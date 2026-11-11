use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use git_oxide_engine::cache::OidCache;
use git_oxide_engine::indexer::{run_phase1, run_phase2_churn};
use git_oxide_engine::models::{
    BlameLineRow, ChangedFileStat, ChangedFileStatus, ChurnBatchProgress, CodeTokenSpan, DiffLineKind, DiffLineRow,
    HEATMAP_WEEKS, Phase1Snapshot, TreeEntryItem,
};
use git_oxide_engine::syntax::{
    COLOR_DEFAULT, MAX_HIGHLIGHT_BLOB_BYTES, MAX_HIGHLIGHT_LINE_CHARS, highlight_blob_lines,
};
use git_oxide_engine::{
    load_commit_changed_files, load_commit_file_diff, load_ref_file_blame, load_ref_file_source, load_ref_tree_entries,
};
use tempfile::TempDir;

fn run_git(repo_dir: &Path, args: &[&str], env_vars: &[(&str, &str)]) {
    let mut cmd: Command = Command::new("git");
    cmd.current_dir(repo_dir).args(args);
    for (key, val) in env_vars {
        cmd.env(key, val);
    }
    let output = cmd.output().expect("git binary must be available in test environment");
    assert!(output.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&output.stderr));
}

fn create_fixture_repository(repo_dir: &Path) {
    run_git(repo_dir, &["init", "-b", "main"], &[]);
    run_git(repo_dir, &["config", "user.name", "Alice Dev"], &[]);
    run_git(repo_dir, &["config", "user.email", "alice@example.com"], &[]);

    let src_dir = repo_dir.join("src");
    let scripts_dir = repo_dir.join("scripts");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&scripts_dir).unwrap();

    fs::write(src_dir.join("lib.rs"), "// Initial library module\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n")
        .unwrap();
    fs::write(
        scripts_dir.join("helper.py"),
        "# Helper script\ndef greet(name: str) -> str:\n    return f\"hello {name}\"\n",
    )
    .unwrap();

    run_git(repo_dir, &["add", "."], &[]);
    run_git(
        repo_dir,
        &["commit", "-m", "feat: initial rust and python modules"],
        &[("GIT_AUTHOR_DATE", "2026-10-01T12:00:00Z"), ("GIT_COMMITTER_DATE", "2026-10-01T12:00:00Z")],
    );

    // Second commit by Bob modifying src/lib.rs and adding config.json
    fs::write(
        src_dir.join("lib.rs"),
        "// Initial library module\npub fn add(a: i32, b: i32) -> i32 {\n    a.saturating_add(b)\n}\n\npub fn multiply(a: i32, b: i32) -> i32 {\n    a * b\n}\n",
    )
    .unwrap();
    fs::write(repo_dir.join("config.json"), "{\n  \"name\": \"git-oxide\",\n  \"fast\": true\n}\n").unwrap();

    run_git(repo_dir, &["add", "."], &[]);
    run_git(
        repo_dir,
        &[
            "-c",
            "user.name=Bob Maintainer",
            "-c",
            "user.email=bob@example.com",
            "commit",
            "-m",
            "feat: use saturating_add and add multiply",
        ],
        &[("GIT_AUTHOR_DATE", "2026-10-08T15:30:00Z"), ("GIT_COMMITTER_DATE", "2026-10-08T15:30:00Z")],
    );

    // Create a secondary branch tag
    run_git(repo_dir, &["branch", "feature/analytics"], &[]);
}

#[test]
fn syntax_highlighter_coalesces_adjacent_tokens_and_enforces_guardrails() {
    let rust_code: &str = "pub fn compute(value: usize) -> usize {\n    value + 42\n}\n";
    let lines: Vec<Vec<CodeTokenSpan>> = highlight_blob_lines("src/lib.rs", rust_code);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].len() > 1, "expected AST syntax highlighter to produce multiple colored spans on Rust signature");

    // Verify adjacent spans never share the exact same color
    for line in &lines {
        for pair in line.windows(2) {
            assert_ne!(pair[0].color, pair[1].color, "adjacent spans with identical colors must be coalesced");
        }
    }

    // Verify 500-character line guardrail collapses long lines into a single default-colored span
    let long_line: String = format!("let x: usize = {};", "1 + ".repeat((MAX_HIGHLIGHT_LINE_CHARS / 4) + 20));
    let long_highlighted: Vec<Vec<CodeTokenSpan>> = highlight_blob_lines("src/lib.rs", &long_line);
    assert_eq!(long_highlighted.len(), 1);
    assert_eq!(long_highlighted[0].len(), 1);
    assert_eq!(long_highlighted[0][0].color, COLOR_DEFAULT);

    // Verify 1 MB blob guardrail bypasses AST highlighting
    let huge_blob: String = "fn a() {}\n".repeat((MAX_HIGHLIGHT_BLOB_BYTES / 10) + 10);
    let huge_lines: Vec<Vec<CodeTokenSpan>> = highlight_blob_lines("src/lib.rs", &huge_blob);
    assert!(!huge_lines.is_empty());
    assert_eq!(huge_lines[0].len(), 1);
    assert_eq!(huge_lines[0][0].color, COLOR_DEFAULT);
}

#[test]
fn end_to_end_progressive_indexing_diff_blame_and_redb_cache_hits() {
    let temp_dir: TempDir = TempDir::new().unwrap();
    let repo_dir = temp_dir.path().join("sample-repo");
    fs::create_dir_all(&repo_dir).unwrap();
    create_fixture_repository(&repo_dir);

    let cache_path = temp_dir.path().join("cache.redb");
    let cache: OidCache = OidCache::open_at(&cache_path).unwrap();
    let cancel_flag: AtomicBool = AtomicBool::new(false);

    // Phase 1 initial scan (cold cache)
    let snapshot1: Phase1Snapshot = run_phase1(&repo_dir, None, &cache, &cancel_flag).unwrap();
    assert_eq!(snapshot1.total_commits, 2);
    assert_eq!(snapshot1.indexed_churn_commits, 0);
    assert_eq!(snapshot1.heatmap_weeks.len(), HEATMAP_WEEKS);
    assert_eq!(snapshot1.contributors.len(), 2);
    assert!(snapshot1.refs.iter().any(|r| r.display_name == "main"), "expected main branch in repository refs");
    assert!(
        snapshot1.refs.iter().any(|r| r.display_name == "feature/analytics"),
        "expected feature/analytics branch in repository refs"
    );

    // Verify language breakdown via in-memory tokei blob parsing
    assert!(
        snapshot1.languages.iter().any(|l| l.language == "Rust" && l.code >= 5),
        "expected Rust language stats in Phase 1 snapshot"
    );
    assert!(
        snapshot1.languages.iter().any(|l| l.language == "Python" && l.code >= 2),
        "expected Python language stats in Phase 1 snapshot"
    );

    // Phase 2 background churn index
    let mut final_progress: Option<ChurnBatchProgress> = None;
    run_phase2_churn(&repo_dir, &snapshot1, &cache, &cancel_flag, |progress| {
        final_progress = Some(progress);
    })
    .unwrap();

    let progress: ChurnBatchProgress = final_progress.expect("Phase 2 must emit progress");
    assert!(progress.is_complete);
    assert_eq!(progress.indexed_commits, 2);
    assert!(!progress.hotspots.is_empty());
    assert_eq!(progress.hotspots[0].path, "src/lib.rs");
    assert_eq!(progress.hotspots[0].touches, 2);
    assert!(!progress.weekly_churn.is_empty());
    assert!(!progress.monthly_churn.is_empty());

    // Re-run Phase 1 and confirm 100% redb OID Cache hit
    let snapshot2: Phase1Snapshot = run_phase1(&repo_dir, None, &cache, &cancel_flag).unwrap();
    assert_eq!(snapshot2.indexed_churn_commits, 2);
    assert!(snapshot2.commits.iter().all(|c| c.churn_indexed));

    // Verify commit changed files and Unified/Split AST-highlighted diff
    let head_commit_oid: &str = &snapshot2.commits[0].oid_hex;
    let changed_files: Vec<ChangedFileStat> = load_commit_changed_files(&repo_dir, head_commit_oid).unwrap();
    assert_eq!(changed_files.len(), 2);

    let diff_rows: Vec<DiffLineRow> = load_commit_file_diff(&repo_dir, head_commit_oid, "src/lib.rs").unwrap();
    assert!(!diff_rows.is_empty());
    assert!(diff_rows.iter().any(|r| r.kind == DiffLineKind::HunkHeader));
    assert!(diff_rows.iter().any(|r| r.kind == DiffLineKind::Removed));
    assert!(diff_rows.iter().any(|r| r.kind == DiffLineKind::Added));

    // Verify Unified Removed rows do not carry a new_line_no and Unified Added rows do not carry an old_line_no
    for row in &diff_rows {
        match row.kind {
            DiffLineKind::Removed => {
                assert!(row.old_line_no.is_some());
                assert!(row.new_line_no.is_none());
            }
            DiffLineKind::Added => {
                assert!(row.old_line_no.is_none());
                assert!(row.new_line_no.is_some());
            }
            DiffLineKind::Context => {
                assert!(row.old_line_no.is_some());
                assert!(row.new_line_no.is_some());
                assert_eq!(row.old_line_no, row.left_line_no);
                assert_eq!(row.new_line_no, row.right_line_no);
            }
            _ => {}
        }
    }

    // Verify tree browser, source view, and gix blame view
    let tree_entries: Vec<TreeEntryItem> = load_ref_tree_entries(&repo_dir, head_commit_oid).unwrap();
    assert_eq!(tree_entries.len(), 3);

    let source_rows: Vec<BlameLineRow> = load_ref_file_source(&repo_dir, head_commit_oid, "src/lib.rs").unwrap();
    assert_eq!(source_rows.len(), 8);

    let blame_rows: Vec<BlameLineRow> = load_ref_file_blame(&repo_dir, head_commit_oid, "src/lib.rs").unwrap();
    assert_eq!(blame_rows.len(), 8);
    assert_eq!(blame_rows[0].author_name, "Alice Dev");
    assert_eq!(blame_rows[2].author_name, "Bob Maintainer");

    // Verify rename detection and diff across renamed file
    let tools_dir = repo_dir.join("tools");
    fs::create_dir_all(&tools_dir).unwrap();
    run_git(&repo_dir, &["mv", "scripts/helper.py", "tools/helper.py"], &[]);
    fs::write(
        tools_dir.join("helper.py"),
        "# Helper script\ndef greet(name: str) -> str:\n    return f\"hi {name}!\"\n",
    )
    .unwrap();
    run_git(&repo_dir, &["add", "."], &[]);
    run_git(
        &repo_dir,
        &["commit", "-m", "refactor: move helper.py to tools and update greeting"],
        &[("GIT_AUTHOR_DATE", "2026-10-12T10:00:00Z"), ("GIT_COMMITTER_DATE", "2026-10-12T10:00:00Z")],
    );

    let snapshot3: Phase1Snapshot = run_phase1(&repo_dir, Some("main"), &cache, &cancel_flag).unwrap();
    assert_eq!(snapshot3.total_commits, 3);
    let rename_commit_oid: &str = &snapshot3.commits[0].oid_hex;
    let rename_changed: Vec<ChangedFileStat> = load_commit_changed_files(&repo_dir, rename_commit_oid).unwrap();
    assert_eq!(rename_changed.len(), 1);
    assert_eq!(rename_changed[0].status, ChangedFileStatus::Renamed);
    assert_eq!(rename_changed[0].old_path.as_deref(), Some("scripts/helper.py"));
    assert_eq!(rename_changed[0].path, "tools/helper.py");
    assert_eq!(rename_changed[0].insertions, 1);
    assert_eq!(rename_changed[0].deletions, 1);

    let rename_diff: Vec<DiffLineRow> = load_commit_file_diff(&repo_dir, rename_commit_oid, "tools/helper.py").unwrap();
    assert!(rename_diff.iter().any(|r| r.kind == DiffLineKind::Removed));
    assert!(rename_diff.iter().any(|r| r.kind == DiffLineKind::Added));
}
