use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use git_oxide_engine::cache::OidCache;
use git_oxide_engine::error::EngineError;
use git_oxide_engine::indexer::run_phase1;
use git_oxide_engine::models::{BlameLineRow, ChangedFileStat, Phase1Snapshot, TreeEntryItem};
use git_oxide_engine::{load_commit_changed_files, load_ref_file_blame, load_ref_tree_entries};
use tempfile::TempDir;

mod common;

use common::{create_fixture_repository, run_git};

const FIXTURE_COMMIT_COUNT: usize = 2;

/// `cache` is declared first so it drops (closing the `redb` file) before `temp_dir` is deleted, which Windows
/// requires.
struct WorktreeFixture {
    cache: OidCache,
    main_dir: PathBuf,
    temp_dir: TempDir,
}

impl WorktreeFixture {
    fn new() -> Self {
        let temp_dir: TempDir = TempDir::new().unwrap();
        let main_dir: PathBuf = temp_dir.path().join("main-repo");
        fs::create_dir_all(&main_dir).unwrap();
        create_fixture_repository(&main_dir);
        let cache: OidCache = OidCache::open_at(&temp_dir.path().join("cache.redb")).unwrap();
        Self { cache, main_dir, temp_dir }
    }

    fn add_worktree(&self, name: &str, extra_args: &[&str]) -> PathBuf {
        let worktree_dir: PathBuf = self.temp_dir.path().join(name);
        let worktree_arg: String = worktree_dir.display().to_string();
        let mut args: Vec<&str> = vec!["worktree", "add"];
        args.extend_from_slice(extra_args);
        args.push(&worktree_arg);
        run_git(&self.main_dir, &args, &[]);
        worktree_dir
    }

    fn phase1(&self, path: &Path) -> Result<Phase1Snapshot, EngineError> {
        run_phase1(path, None, &self.cache, &AtomicBool::new(false))
    }
}

fn same_path(left: &str, right: &Path) -> bool {
    fs::canonicalize(left).unwrap() == fs::canonicalize(right).unwrap()
}

#[test]
fn linked_worktree_on_branch_indexes_and_reports_main_repository() {
    let fixture: WorktreeFixture = WorktreeFixture::new();
    let worktree_dir: PathBuf = fixture.add_worktree("linked-branch", &["-b", "wt-branch"]);

    let snapshot: Phase1Snapshot = fixture.phase1(&worktree_dir).unwrap();
    assert_eq!(snapshot.active_ref_name, "HEAD (wt-branch)");
    assert_eq!(snapshot.commits.len(), FIXTURE_COMMIT_COUNT);
    assert!(same_path(&snapshot.repo_path, &worktree_dir));
    let main_path: &str = snapshot.linked_worktree_of.as_deref().expect("linked worktree must report its main repo");
    assert!(same_path(main_path, &fixture.main_dir), "unexpected main repository path {main_path}");
    assert!(snapshot.refs.iter().any(|item| item.full_ref_name == "refs/heads/main"));

    let head_oid: &str = &snapshot.active_ref_oid_hex;
    let changed: Vec<ChangedFileStat> = load_commit_changed_files(&worktree_dir, head_oid).unwrap();
    assert!(changed.iter().any(|file| file.path == "src/lib.rs"));
    let entries: Vec<TreeEntryItem> = load_ref_tree_entries(&worktree_dir, head_oid).unwrap();
    assert!(entries.iter().any(|entry| entry.path == "config.json"));
    let blame: Vec<BlameLineRow> = load_ref_file_blame(&worktree_dir, head_oid, "src/lib.rs").unwrap();
    assert!(!blame.is_empty());
}

#[test]
fn main_checkout_is_not_reported_as_linked_worktree() {
    let fixture: WorktreeFixture = WorktreeFixture::new();
    fixture.add_worktree("sibling", &["-b", "sibling"]);

    let snapshot: Phase1Snapshot = fixture.phase1(&fixture.main_dir).unwrap();
    assert_eq!(snapshot.linked_worktree_of, None);
}

#[test]
fn detached_worktree_uses_its_own_head() {
    let fixture: WorktreeFixture = WorktreeFixture::new();
    let worktree_dir: PathBuf = fixture.add_worktree("detached", &["--detach"]);
    run_git(&worktree_dir, &["checkout", "--detach", "HEAD~1"], &[]);

    let snapshot: Phase1Snapshot = fixture.phase1(&worktree_dir).unwrap();
    assert_eq!(snapshot.active_ref_name, "HEAD (detached)");
    assert_eq!(snapshot.commits.len(), FIXTURE_COMMIT_COUNT - 1);
}

#[test]
fn worktree_opens_from_a_subdirectory() {
    let fixture: WorktreeFixture = WorktreeFixture::new();
    let worktree_dir: PathBuf = fixture.add_worktree("nested-open", &["-b", "nested"]);

    let snapshot: Phase1Snapshot = fixture.phase1(&worktree_dir.join("src")).unwrap();
    assert!(same_path(&snapshot.repo_path, &worktree_dir));
    assert!(snapshot.linked_worktree_of.is_some());
}

#[test]
fn worktree_with_per_worktree_config_extension_indexes() {
    let fixture: WorktreeFixture = WorktreeFixture::new();
    run_git(&fixture.main_dir, &["config", "extensions.worktreeConfig", "true"], &[]);
    let worktree_dir: PathBuf = fixture.add_worktree("worktree-config", &["-b", "with-config"]);
    run_git(&worktree_dir, &["config", "--worktree", "user.name", "Worktree Only"], &[]);

    let snapshot: Phase1Snapshot = fixture.phase1(&worktree_dir).unwrap();
    assert_eq!(snapshot.active_ref_name, "HEAD (with-config)");
    assert_eq!(snapshot.commits.len(), FIXTURE_COMMIT_COUNT);
}

#[test]
fn worktree_whose_admin_directory_was_removed_fails_to_open_with_details() {
    let fixture: WorktreeFixture = WorktreeFixture::new();
    let worktree_dir: PathBuf = fixture.add_worktree("orphaned", &["-b", "orphaned"]);
    fs::remove_dir_all(fixture.main_dir.join(".git").join("worktrees").join("orphaned")).unwrap();

    let error: EngineError = fixture.phase1(&worktree_dir).unwrap_err();
    let EngineError::RepositoryOpen { message, .. } = &error else {
        panic!("expected RepositoryOpen, got {error:?}");
    };
    assert!(!message.is_empty());
}

#[test]
fn repository_without_commits_reports_unborn_branch() {
    let temp_dir: TempDir = TempDir::new().unwrap();
    let repo_dir: PathBuf = temp_dir.path().join("empty-repo");
    fs::create_dir_all(&repo_dir).unwrap();
    run_git(&repo_dir, &["init", "-b", "trunk"], &[]);
    let cache: OidCache = OidCache::open_at(&temp_dir.path().join("cache.redb")).unwrap();

    let error: EngineError = run_phase1(&repo_dir, None, &cache, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(&error, EngineError::UnbornHead { branch } if branch == "trunk"), "got {error:?}");
}

#[test]
fn unreadable_objects_report_the_underlying_cause_instead_of_an_empty_repository() {
    let fixture: WorktreeFixture = WorktreeFixture::new();
    let worktree_dir: PathBuf = fixture.add_worktree("missing-objects", &["-b", "missing"]);
    let objects_dir: PathBuf = fixture.main_dir.join(".git").join("objects");
    fs::remove_dir_all(&objects_dir).unwrap();
    fs::create_dir_all(objects_dir.join("pack")).unwrap();

    let error: EngineError = fixture.phase1(&worktree_dir).unwrap_err();
    let EngineError::ObjectRead(message) = &error else {
        panic!("expected ObjectRead, got {error:?}");
    };
    assert!(message.contains("HEAD"), "message should name the ref that failed: {message}");
    assert!(message.contains(": "), "message should include the underlying cause: {message}");
}
