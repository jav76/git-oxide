use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use chrono::{DateTime, Utc};
use gix::ObjectId;
use gix::objs::tree::EntryKind;
use tokei::{Config as TokeiConfig, LanguageType};

use crate::diff::{find_blob_in_tree_by_path, is_blob_entry, parse_oid_hex, read_blob_bytes, read_tree_entries_map};
use crate::error::EngineError;
use crate::models::{BlameLineRow, CodeTokenSpan, RgbaColor, TreeEntryItem};
use crate::syntax::{COLOR_COMMENT, COLOR_DEFAULT, highlight_blob_lines};

/// Length of abbreviated commit SHA strings displayed in UI tables and blame gutters.
pub const SHORT_OID_LEN: usize = 7;

/// Classifies a file path into a human-readable programming language name using `tokei`.
pub fn classify_language_name(file_path: &str) -> String {
    let config: TokeiConfig = TokeiConfig::default();
    LanguageType::from_path(Path::new(file_path), &config)
        .map_or_else(|| "Other".to_owned(), |lang| lang.name().to_owned())
}

/// Assigns a deterministic brand color for a programming language name.
pub fn language_badge_color(language: &str) -> RgbaColor {
    match language {
        "Rust" => RgbaColor::rgb(222, 165, 132),
        "TypeScript" | "TSX" => RgbaColor::rgb(49, 120, 198),
        "JavaScript" | "JSX" => RgbaColor::rgb(241, 224, 90),
        "Python" => RgbaColor::rgb(53, 114, 165),
        "Go" => RgbaColor::rgb(0, 173, 216),
        "C" => RgbaColor::rgb(85, 85, 85),
        "C++" | "C Header" | "C++ Header" => RgbaColor::rgb(243, 75, 125),
        "C#" => RgbaColor::rgb(23, 134, 0),
        "HTML" => RgbaColor::rgb(227, 76, 38),
        "CSS" | "Sass" => RgbaColor::rgb(86, 61, 124),
        "JSON" => RgbaColor::rgb(64, 160, 112),
        "TOML" | "YAML" => RgbaColor::rgb(156, 66, 33),
        "Markdown" => RgbaColor::rgb(8, 63, 161),
        "Bourne Shell" | "Bash" | "Zsh" => RgbaColor::rgb(137, 224, 81),
        _ => RgbaColor::rgb(136, 146, 176),
    }
}

/// Formats a Unix epoch timestamp in seconds into a `YYYY-MM-DD` UTC date string.
pub fn format_date_iso(timestamp_secs: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp_secs, 0)
        .map_or_else(|| "1970-01-01".to_owned(), |dt| dt.format("%Y-%m-%d").to_string())
}

/// Lists all tracked blob files in the commit tree at `commit_oid_hex` in sorted path order.
pub fn list_tree_entries(repo: &gix::Repository, commit_oid_hex: &str) -> Result<Vec<TreeEntryItem>, EngineError> {
    let commit_oid: ObjectId = parse_oid_hex(commit_oid_hex)?;
    let commit = repo
        .find_object(commit_oid)
        .map_err(|error| EngineError::GitOperation(format!("failed to find commit {commit_oid}: {error}")))?
        .try_into_commit()
        .map_err(|error| EngineError::GitOperation(format!("object {commit_oid} is not a commit: {error}")))?;
    let root_tree_oid: ObjectId = commit
        .tree_id()
        .map_err(|error| EngineError::GitOperation(format!("failed to read root tree id: {error}")))?
        .detach();

    let mut items: Vec<TreeEntryItem> = Vec::new();
    collect_tree_items_recursive(repo, root_tree_oid, "", 0, &mut items)?;
    Ok(items)
}

fn collect_tree_items_recursive(
    repo: &gix::Repository,
    tree_oid: ObjectId,
    prefix: &str,
    depth: u32,
    items: &mut Vec<TreeEntryItem>,
) -> Result<(), EngineError> {
    let entries: BTreeMap<String, (EntryKind, ObjectId)> = read_tree_entries_map(repo, tree_oid)?;
    for (name, (kind, oid)) in entries {
        let full_path: String = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };

        if kind == EntryKind::Tree {
            collect_tree_items_recursive(repo, oid, &full_path, depth + 1, items)?;
        } else if is_blob_entry(kind) {
            let size_bytes: u64 = repo.find_header(oid).map_or(0, |header| header.size());
            let language: String = classify_language_name(&full_path);
            items.push(TreeEntryItem {
                path: full_path,
                file_name: name,
                depth,
                language,
                size_bytes,
                blob_oid_hex: oid.to_hex().to_string(),
            });
        }
    }
    Ok(())
}

/// Reads and AST-highlights a file at `(commit_oid_hex, file_path)` without running blame traversal.
pub fn read_file_source_lines(
    repo: &gix::Repository,
    commit_oid_hex: &str,
    file_path: &str,
) -> Result<Vec<BlameLineRow>, EngineError> {
    let commit_oid: ObjectId = parse_oid_hex(commit_oid_hex)?;
    let commit = repo
        .find_object(commit_oid)
        .map_err(|error| EngineError::GitOperation(format!("failed to find commit {commit_oid}: {error}")))?
        .try_into_commit()
        .map_err(|error| EngineError::GitOperation(format!("object {commit_oid} is not a commit: {error}")))?;
    let root_tree_oid: ObjectId = commit
        .tree_id()
        .map_err(|error| EngineError::GitOperation(format!("failed to read commit tree id: {error}")))?
        .detach();

    let Some(blob_oid) = find_blob_in_tree_by_path(repo, root_tree_oid, file_path)? else {
        return Err(EngineError::PathNotFound(file_path.to_owned()));
    };

    let bytes: Vec<u8> = read_blob_bytes(repo, Some(blob_oid))?;
    let Ok(content) = std::str::from_utf8(&bytes) else {
        return Ok(vec![BlameLineRow {
            line_no: 1,
            commit_oid_hex: commit_oid_hex.to_owned(),
            short_oid: shorten_oid_str(commit_oid_hex),
            author_name: String::new(),
            date_iso: String::new(),
            summary: String::new(),
            is_hunk_start: true,
            spans: vec![CodeTokenSpan { text: "Binary or non-UTF-8 file".to_owned(), color: COLOR_COMMENT }],
        }]);
    };

    let highlighted_lines: Vec<Vec<CodeTokenSpan>> = highlight_blob_lines(file_path, content);
    let short_oid: String = shorten_oid_str(commit_oid_hex);
    let rows: Vec<BlameLineRow> = highlighted_lines
        .into_iter()
        .enumerate()
        .map(|(idx, spans)| BlameLineRow {
            line_no: (idx + 1) as u32,
            commit_oid_hex: commit_oid_hex.to_owned(),
            short_oid: short_oid.clone(),
            author_name: String::new(),
            date_iso: String::new(),
            summary: String::new(),
            is_hunk_start: idx == 0,
            spans,
        })
        .collect();

    Ok(rows)
}

/// Computes line-by-line `gix::blame` attribution paired with `tree-sitter-highlight` AST spans.
pub fn compute_file_blame(
    repo: &gix::Repository,
    commit_oid_hex: &str,
    file_path: &str,
) -> Result<Vec<BlameLineRow>, EngineError> {
    let commit_oid: ObjectId = parse_oid_hex(commit_oid_hex)?;
    let source_rows: Vec<BlameLineRow> = read_file_source_lines(repo, commit_oid_hex, file_path)?;
    if source_rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut resource_cache = repo
        .diff_resource_cache_for_tree_diff()
        .map_err(|error| EngineError::GitOperation(format!("failed to create diff resource cache: {error}")))?;

    let walk = repo
        .rev_walk([commit_oid])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|error| EngineError::GitOperation(format!("failed to start blame revision walk: {error}")))?;

    let traverse_iter = walk.map(|item_res| {
        item_res.map(|info| gix::traverse::commit::Info {
            id: info.id,
            parent_ids: info.parent_ids,
            commit_time: info.commit_time,
        })
    });

    let clean_path: &str = file_path.trim_matches('/');
    let outcome = match gix::blame::file(&repo.objects, traverse_iter, &mut resource_cache, clean_path.into(), None) {
        Ok(outcome) => outcome,
        Err(error) => {
            log::warn!("falling back to head commit attribution for '{clean_path}' at {commit_oid}: {error}");
            return Ok(attach_single_commit_blame(repo, commit_oid, source_rows));
        }
    };

    let mut commit_meta_cache: HashMap<ObjectId, (String, String, String, String)> = HashMap::new();
    let mut rows: Vec<BlameLineRow> = source_rows;

    for entry in &outcome.entries {
        let meta: (String, String, String, String) = commit_meta_cache
            .entry(entry.commit_id)
            .or_insert_with(|| resolve_commit_blame_meta(repo, entry.commit_id))
            .clone();

        let range = entry.range_in_blamed_file();
        for (offset, line_idx) in range.enumerate() {
            if let Some(row) = rows.get_mut(line_idx) {
                row.commit_oid_hex.clone_from(&meta.0);
                row.short_oid.clone_from(&meta.1);
                row.author_name.clone_from(&meta.2);
                row.date_iso.clone_from(&meta.3);
                row.is_hunk_start = offset == 0;
            }
        }
    }

    // Populate any lines not covered by blame entries with the target commit metadata.
    let fallback_meta: (String, String, String, String) = resolve_commit_blame_meta(repo, commit_oid);
    for row in &mut rows {
        if row.author_name.is_empty() {
            row.commit_oid_hex.clone_from(&fallback_meta.0);
            row.short_oid.clone_from(&fallback_meta.1);
            row.author_name.clone_from(&fallback_meta.2);
            row.date_iso.clone_from(&fallback_meta.3);
        }
        if row.spans.is_empty() {
            row.spans.push(CodeTokenSpan { text: String::new(), color: COLOR_DEFAULT });
        }
    }

    Ok(rows)
}

fn attach_single_commit_blame(
    repo: &gix::Repository,
    commit_oid: ObjectId,
    mut rows: Vec<BlameLineRow>,
) -> Vec<BlameLineRow> {
    let meta: (String, String, String, String) = resolve_commit_blame_meta(repo, commit_oid);
    for (idx, row) in rows.iter_mut().enumerate() {
        row.commit_oid_hex.clone_from(&meta.0);
        row.short_oid.clone_from(&meta.1);
        row.author_name.clone_from(&meta.2);
        row.date_iso.clone_from(&meta.3);
        row.is_hunk_start = idx == 0;
    }
    rows
}

fn resolve_commit_blame_meta(repo: &gix::Repository, commit_oid: ObjectId) -> (String, String, String, String) {
    let oid_hex: String = commit_oid.to_hex().to_string();
    let short_oid: String = shorten_oid_str(&oid_hex);

    if let Ok(object) = repo.find_object(commit_oid)
        && let Ok(commit) = object.try_into_commit()
        && let Ok(commit_ref) = commit.decode()
    {
        let author_name: String = String::from_utf8_lossy(commit_ref.author.name).into_owned();
        let timestamp: i64 = commit_ref.author.time.seconds;
        let date_iso: String = format_date_iso(timestamp);
        return (oid_hex, short_oid, author_name, date_iso);
    }

    (oid_hex, short_oid, "Unknown".to_owned(), "1970-01-01".to_owned())
}

pub(crate) fn shorten_oid_str(oid_hex: &str) -> String {
    oid_hex.chars().take(SHORT_OID_LEN).collect()
}
