use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use gix::ObjectId;
use gix::objs::TreeRefIter;
use gix::objs::tree::EntryKind;
use imara_diff::Algorithm;
use imara_diff::intern::InternedInput;

use crate::error::EngineError;
use crate::models::{
    ChangedFileStat, ChangedFileStatus, CodeTokenSpan, DiffLineKind, DiffLineRow, FileChurnEntry, RgbaColor,
};
use crate::syntax::{COLOR_COMMENT, COLOR_DEFAULT, highlight_blob_lines};

/// Number of unchanged context lines shown before and after each diff hunk.
pub const DIFF_CONTEXT_LINES: usize = 3;

/// Maximum blob size in bytes (4 MB) to run line-level diffing on during churn or view generation.
pub const MAX_DIFF_BLOB_BYTES: usize = 4_194_304;

/// Hunk header color (`#61AFEF`).
pub const COLOR_HUNK_HEADER: RgbaColor = RgbaColor::rgb(97, 175, 239);

/// Raw tree entry change discovered by Merkle-tree comparison.
#[derive(Debug, Clone)]
pub struct RawTreeChange {
    pub path: String,
    pub old_path: Option<String>,
    pub old_blob_oid: Option<ObjectId>,
    pub new_blob_oid: Option<ObjectId>,
}

/// Parses a 40-character hexadecimal SHA-1 string into a `gix::ObjectId`.
pub fn parse_oid_hex(oid_hex: &str) -> Result<ObjectId, EngineError> {
    ObjectId::from_hex(oid_hex.trim().as_bytes()).map_err(|_| EngineError::InvalidObjectId(oid_hex.to_owned()))
}

/// Resolves the tree `ObjectId` and first parent commit `ObjectId` (if any) for a commit.
pub fn resolve_commit_and_parent_trees(
    repo: &gix::Repository,
    commit_oid: ObjectId,
) -> Result<(Option<ObjectId>, ObjectId), EngineError> {
    let commit = repo
        .find_object(commit_oid)
        .map_err(|error| EngineError::GitOperation(format!("failed to find commit {commit_oid}: {error}")))?
        .try_into_commit()
        .map_err(|error| EngineError::GitOperation(format!("object {commit_oid} is not a commit: {error}")))?;

    let new_tree_oid: ObjectId = commit
        .tree_id()
        .map_err(|error| EngineError::GitOperation(format!("failed to read commit tree id: {error}")))?
        .detach();

    let first_parent_oid: Option<ObjectId> = commit.parent_ids().next().map(gix::Id::detach);
    let old_tree_oid: Option<ObjectId> = if let Some(parent_oid) = first_parent_oid {
        if let Ok(parent_obj) = repo.find_object(parent_oid) {
            let parent_commit = parent_obj
                .try_into_commit()
                .map_err(|error| EngineError::GitOperation(format!("parent {parent_oid} is not a commit: {error}")))?;
            let tree_oid: ObjectId = parent_commit
                .tree_id()
                .map_err(|error| EngineError::GitOperation(format!("failed to read parent tree id: {error}")))?
                .detach();
            Some(tree_oid)
        } else {
            None
        }
    } else {
        None
    };

    Ok((old_tree_oid, new_tree_oid))
}

/// Compares two Git trees recursively, skipping identical subtree OIDs in O(1), and pairs renamed files.
pub fn collect_tree_changes(
    repo: &gix::Repository,
    old_tree_oid: Option<ObjectId>,
    new_tree_oid: Option<ObjectId>,
) -> Result<Vec<RawTreeChange>, EngineError> {
    let mut changes: Vec<RawTreeChange> = Vec::new();
    diff_trees_recursive(repo, "", old_tree_oid, new_tree_oid, &mut changes)?;
    Ok(coalesce_renamed_changes(changes))
}

fn coalesce_renamed_changes(changes: Vec<RawTreeChange>) -> Vec<RawTreeChange> {
    let mut deleted: Vec<RawTreeChange> = Vec::new();
    let mut added: Vec<RawTreeChange> = Vec::new();
    let mut modified: Vec<RawTreeChange> = Vec::new();

    for change in changes {
        match (change.old_blob_oid, change.new_blob_oid) {
            (Some(_), None) => deleted.push(change),
            (None, Some(_)) => added.push(change),
            _ => modified.push(change),
        }
    }

    if deleted.is_empty() || added.is_empty() {
        modified.extend(deleted);
        modified.extend(added);
        modified.sort_by(|a, b| a.path.cmp(&b.path));
        return modified;
    }

    let mut used_deleted: Vec<bool> = vec![false; deleted.len()];
    let mut used_added: Vec<bool> = vec![false; added.len()];

    // Pass 1: exact blob OID match between a deleted path and an added path.
    for (add_idx, add_entry) in added.iter().enumerate() {
        let Some(new_oid) = add_entry.new_blob_oid else {
            continue;
        };
        if let Some((del_idx, del_entry)) =
            deleted.iter().enumerate().find(|(idx, del)| !used_deleted[*idx] && del.old_blob_oid == Some(new_oid))
        {
            used_deleted[del_idx] = true;
            used_added[add_idx] = true;
            modified.push(RawTreeChange {
                path: add_entry.path.clone(),
                old_path: Some(del_entry.path.clone()),
                old_blob_oid: del_entry.old_blob_oid,
                new_blob_oid: add_entry.new_blob_oid,
            });
        }
    }

    // Pass 2: matching file basename when a file is moved across directories and edited in the same commit.
    for (add_idx, add_entry) in added.iter().enumerate() {
        if used_added[add_idx] {
            continue;
        }
        let Some(add_name) = Path::new(&add_entry.path).file_name() else {
            continue;
        };
        if let Some((del_idx, del_entry)) = deleted
            .iter()
            .enumerate()
            .find(|(idx, del)| !used_deleted[*idx] && Path::new(&del.path).file_name() == Some(add_name))
        {
            used_deleted[del_idx] = true;
            used_added[add_idx] = true;
            modified.push(RawTreeChange {
                path: add_entry.path.clone(),
                old_path: Some(del_entry.path.clone()),
                old_blob_oid: del_entry.old_blob_oid,
                new_blob_oid: add_entry.new_blob_oid,
            });
        }
    }

    for (idx, del_entry) in deleted.into_iter().enumerate() {
        if !used_deleted[idx] {
            modified.push(del_entry);
        }
    }
    for (idx, add_entry) in added.into_iter().enumerate() {
        if !used_added[idx] {
            modified.push(add_entry);
        }
    }

    modified.sort_by(|a, b| a.path.cmp(&b.path));
    modified
}

fn format_child_path(prefix: &str, name_bytes: &[u8]) -> String {
    let name: std::borrow::Cow<'_, str> = String::from_utf8_lossy(name_bytes);
    if prefix.is_empty() { name.into_owned() } else { format!("{prefix}/{name}") }
}

fn parse_borrowed_tree_entries(tree_bytes: &[u8]) -> Vec<(&[u8], EntryKind, ObjectId)> {
    let mut entries: Vec<(&[u8], EntryKind, ObjectId)> = Vec::new();
    for entry_ref in TreeRefIter::from_bytes(tree_bytes).flatten() {
        entries.push((entry_ref.filename, entry_ref.mode.kind(), entry_ref.oid.to_owned()));
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
    entries
}

fn diff_trees_recursive(
    repo: &gix::Repository,
    prefix: &str,
    old_tree_oid: Option<ObjectId>,
    new_tree_oid: Option<ObjectId>,
    changes: &mut Vec<RawTreeChange>,
) -> Result<(), EngineError> {
    if old_tree_oid == new_tree_oid {
        return Ok(());
    }

    let old_bytes: Vec<u8> = read_blob_bytes(repo, old_tree_oid)?;
    let new_bytes: Vec<u8> = read_blob_bytes(repo, new_tree_oid)?;

    let old_entries: Vec<(&[u8], EntryKind, ObjectId)> = parse_borrowed_tree_entries(&old_bytes);
    let new_entries: Vec<(&[u8], EntryKind, ObjectId)> = parse_borrowed_tree_entries(&new_bytes);

    let mut old_idx: usize = 0;
    let mut new_idx: usize = 0;

    while old_idx < old_entries.len() && new_idx < new_entries.len() {
        let (old_name, old_kind, old_oid) = old_entries[old_idx];
        let (new_name, new_kind, new_oid) = new_entries[new_idx];

        match old_name.cmp(new_name) {
            std::cmp::Ordering::Less => {
                let full_path: String = format_child_path(prefix, old_name);
                if old_kind == EntryKind::Tree {
                    diff_trees_recursive(repo, &full_path, Some(old_oid), None, changes)?;
                } else if is_blob_entry(old_kind) {
                    changes.push(RawTreeChange {
                        path: full_path,
                        old_path: None,
                        old_blob_oid: Some(old_oid),
                        new_blob_oid: None,
                    });
                }
                old_idx += 1;
            }
            std::cmp::Ordering::Greater => {
                let full_path: String = format_child_path(prefix, new_name);
                if new_kind == EntryKind::Tree {
                    diff_trees_recursive(repo, &full_path, None, Some(new_oid), changes)?;
                } else if is_blob_entry(new_kind) {
                    changes.push(RawTreeChange {
                        path: full_path,
                        old_path: None,
                        old_blob_oid: None,
                        new_blob_oid: Some(new_oid),
                    });
                }
                new_idx += 1;
            }
            std::cmp::Ordering::Equal => {
                old_idx += 1;
                new_idx += 1;
                if old_oid == new_oid && old_kind == new_kind {
                    continue;
                }

                let full_path: String = format_child_path(prefix, old_name);
                match (old_kind, new_kind) {
                    (EntryKind::Tree, EntryKind::Tree) => {
                        diff_trees_recursive(repo, &full_path, Some(old_oid), Some(new_oid), changes)?;
                    }
                    (EntryKind::Tree, _) => {
                        diff_trees_recursive(repo, &full_path, Some(old_oid), None, changes)?;
                        if is_blob_entry(new_kind) {
                            changes.push(RawTreeChange {
                                path: full_path,
                                old_path: None,
                                old_blob_oid: None,
                                new_blob_oid: Some(new_oid),
                            });
                        }
                    }
                    (_, EntryKind::Tree) => {
                        if is_blob_entry(old_kind) {
                            changes.push(RawTreeChange {
                                path: full_path.clone(),
                                old_path: None,
                                old_blob_oid: Some(old_oid),
                                new_blob_oid: None,
                            });
                        }
                        diff_trees_recursive(repo, &full_path, None, Some(new_oid), changes)?;
                    }
                    _ => {
                        if is_blob_entry(old_kind) || is_blob_entry(new_kind) {
                            changes.push(RawTreeChange {
                                path: full_path,
                                old_path: None,
                                old_blob_oid: is_blob_entry(old_kind).then_some(old_oid),
                                new_blob_oid: is_blob_entry(new_kind).then_some(new_oid),
                            });
                        }
                    }
                }
            }
        }
    }

    while old_idx < old_entries.len() {
        let (old_name, old_kind, old_oid) = old_entries[old_idx];
        let full_path: String = format_child_path(prefix, old_name);
        if old_kind == EntryKind::Tree {
            diff_trees_recursive(repo, &full_path, Some(old_oid), None, changes)?;
        } else if is_blob_entry(old_kind) {
            changes.push(RawTreeChange {
                path: full_path,
                old_path: None,
                old_blob_oid: Some(old_oid),
                new_blob_oid: None,
            });
        }
        old_idx += 1;
    }

    while new_idx < new_entries.len() {
        let (new_name, new_kind, new_oid) = new_entries[new_idx];
        let full_path: String = format_child_path(prefix, new_name);
        if new_kind == EntryKind::Tree {
            diff_trees_recursive(repo, &full_path, None, Some(new_oid), changes)?;
        } else if is_blob_entry(new_kind) {
            changes.push(RawTreeChange {
                path: full_path,
                old_path: None,
                old_blob_oid: None,
                new_blob_oid: Some(new_oid),
            });
        }
        new_idx += 1;
    }

    Ok(())
}

pub(crate) fn is_blob_entry(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::Blob | EntryKind::BlobExecutable)
}

pub(crate) fn read_tree_entries_map(
    repo: &gix::Repository,
    tree_oid: ObjectId,
) -> Result<BTreeMap<String, (EntryKind, ObjectId)>, EngineError> {
    let object = repo
        .find_object(tree_oid)
        .map_err(|error| EngineError::GitOperation(format!("failed to find tree {tree_oid}: {error}")))?;
    let mut map: BTreeMap<String, (EntryKind, ObjectId)> = BTreeMap::new();
    for entry_result in TreeRefIter::from_bytes(&object.data) {
        let entry = entry_result
            .map_err(|error| EngineError::GitOperation(format!("corrupt tree entry in {tree_oid}: {error}")))?;
        let name: String = String::from_utf8_lossy(entry.filename).into_owned();
        map.insert(name, (entry.mode.kind(), entry.oid.to_owned()));
    }
    Ok(map)
}

pub(crate) fn read_blob_bytes(repo: &gix::Repository, blob_oid: Option<ObjectId>) -> Result<Vec<u8>, EngineError> {
    let Some(oid) = blob_oid else {
        return Ok(Vec::new());
    };
    let object = repo
        .find_object(oid)
        .map_err(|error| EngineError::GitOperation(format!("failed to read blob {oid}: {error}")))?;
    Ok(object.data.clone())
}

fn is_binary_or_oversized(bytes: &[u8]) -> bool {
    if bytes.len() > MAX_DIFF_BLOB_BYTES {
        return true;
    }
    let check_len: usize = bytes.len().min(8_000);
    bytes[..check_len].contains(&0)
}

fn count_lines_fast(bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return 0;
    }
    let segments: u32 = bytes.split(|&b| b == b'\n').count() as u32;
    if bytes.last() == Some(&b'\n') { segments.saturating_sub(1) } else { segments }
}

fn trim_common_prefix_suffix_lines<'a>(old_str: &'a str, new_str: &'a str) -> (&'a str, &'a str) {
    let old_bytes: &[u8] = old_str.as_bytes();
    let new_bytes: &[u8] = new_str.as_bytes();

    // Find common byte prefix and back up to the start of the first differing line.
    let prefix_match: usize = old_bytes.iter().zip(new_bytes.iter()).take_while(|(a, b)| a == b).count();
    let line_start: usize = if prefix_match == 0 {
        0
    } else {
        old_bytes[..prefix_match].iter().rposition(|&b| b == b'\n').map_or(0, |pos| pos + 1)
    };

    let old_rem: &[u8] = &old_bytes[line_start..];
    let new_rem: &[u8] = &new_bytes[line_start..];

    // Find common byte suffix and advance to the end of the last differing line.
    let suffix_match: usize = old_rem.iter().rev().zip(new_rem.iter().rev()).take_while(|(a, b)| a == b).count();
    let suffix_trim: usize = if suffix_match == 0 {
        0
    } else {
        let suffix_slice: &[u8] = &old_rem[(old_rem.len() - suffix_match)..];
        suffix_slice.iter().position(|&b| b == b'\n').map_or(0, |pos| suffix_match - pos - 1)
    };

    let old_end: usize = old_bytes.len().saturating_sub(suffix_trim);
    let new_end: usize = new_bytes.len().saturating_sub(suffix_trim);

    (old_str.get(line_start..old_end).unwrap_or(old_str), new_str.get(line_start..new_end).unwrap_or(new_str))
}

/// Fast line insertion and deletion counter between two blob slices using `imara-diff` Histogram.
pub fn count_blob_diff_churn(old_bytes: &[u8], new_bytes: &[u8]) -> (u32, u32) {
    if is_binary_or_oversized(old_bytes) || is_binary_or_oversized(new_bytes) {
        return (0, 0);
    }

    if old_bytes.is_empty() {
        return (count_lines_fast(new_bytes), 0);
    }
    if new_bytes.is_empty() {
        return (0, count_lines_fast(old_bytes));
    }

    let Ok(old_str) = std::str::from_utf8(old_bytes) else {
        return (0, 0);
    };
    let Ok(new_str) = std::str::from_utf8(new_bytes) else {
        return (0, 0);
    };

    let (old_trimmed, new_trimmed) = trim_common_prefix_suffix_lines(old_str, new_str);
    if old_trimmed.is_empty() && new_trimmed.is_empty() {
        return (0, 0);
    }
    if old_trimmed.is_empty() {
        return (count_lines_fast(new_trimmed.as_bytes()), 0);
    }
    if new_trimmed.is_empty() {
        return (0, count_lines_fast(old_trimmed.as_bytes()));
    }

    let input: InternedInput<&str> = InternedInput::new(old_trimmed, new_trimmed);
    let mut insertions: u32 = 0;
    let mut deletions: u32 = 0;

    imara_diff::diff(Algorithm::Histogram, &input, |before: Range<u32>, after: Range<u32>| {
        deletions = deletions.saturating_add(before.len() as u32);
        insertions = insertions.saturating_add(after.len() as u32);
    });

    (insertions, deletions)
}

/// Computes all changed files and their line additions/deletions for a commit OID.
pub fn list_commit_changed_files(
    repo: &gix::Repository,
    commit_oid_hex: &str,
) -> Result<Vec<ChangedFileStat>, EngineError> {
    let commit_oid: ObjectId = parse_oid_hex(commit_oid_hex)?;
    let (old_tree_oid, new_tree_oid) = resolve_commit_and_parent_trees(repo, commit_oid)?;
    let raw_changes: Vec<RawTreeChange> = collect_tree_changes(repo, old_tree_oid, Some(new_tree_oid))?;

    let mut stats: Vec<ChangedFileStat> = Vec::with_capacity(raw_changes.len());
    for change in raw_changes {
        let status: ChangedFileStatus = if change.old_path.is_some() {
            ChangedFileStatus::Renamed
        } else {
            match (change.old_blob_oid, change.new_blob_oid) {
                (None, Some(_)) => ChangedFileStatus::Added,
                (Some(_), None) => ChangedFileStatus::Deleted,
                _ => ChangedFileStatus::Modified,
            }
        };
        let old_bytes: Vec<u8> = read_blob_bytes(repo, change.old_blob_oid)?;
        let new_bytes: Vec<u8> = read_blob_bytes(repo, change.new_blob_oid)?;
        let (insertions, deletions) = count_blob_diff_churn(&old_bytes, &new_bytes);

        stats.push(ChangedFileStat { path: change.path, old_path: change.old_path, status, insertions, deletions });
    }

    Ok(stats)
}

/// Computes `FileChurnEntry` totals for a single commit (used by the Phase 2 worker).
///
/// Skips merge commits (`parent_count > 1`) so subsystem merges do not double-count line churn.
pub fn compute_commit_churn_entries(
    repo: &gix::Repository,
    commit_oid: ObjectId,
) -> Result<Vec<FileChurnEntry>, EngineError> {
    let commit = repo
        .find_object(commit_oid)
        .map_err(|error| EngineError::GitOperation(format!("failed to find commit {commit_oid}: {error}")))?
        .try_into_commit()
        .map_err(|error| EngineError::GitOperation(format!("object {commit_oid} is not a commit: {error}")))?;

    let mut parents = commit.parent_ids();
    let first_parent: Option<ObjectId> = parents.next().map(gix::Id::detach);
    if parents.next().is_some() {
        return Ok(Vec::new());
    }

    let new_tree_oid: ObjectId = commit
        .tree_id()
        .map_err(|error| EngineError::GitOperation(format!("failed to read commit tree id: {error}")))?
        .detach();

    let old_tree_oid: Option<ObjectId> = if let Some(parent_oid) = first_parent {
        let Ok(parent_obj) = repo.find_object(parent_oid) else {
            // Shallow clone graft boundary: parent commit object is not present locally.
            return Ok(Vec::new());
        };
        let parent_commit = parent_obj
            .try_into_commit()
            .map_err(|error| EngineError::GitOperation(format!("parent {parent_oid} is not a commit: {error}")))?;
        let tree_oid: ObjectId = parent_commit
            .tree_id()
            .map_err(|error| EngineError::GitOperation(format!("failed to read parent tree id: {error}")))?
            .detach();
        Some(tree_oid)
    } else {
        None
    };

    let raw_changes: Vec<RawTreeChange> = collect_tree_changes(repo, old_tree_oid, Some(new_tree_oid))?;

    let mut entries: Vec<FileChurnEntry> = Vec::with_capacity(raw_changes.len());
    for change in raw_changes {
        let old_bytes: Vec<u8> = read_blob_bytes(repo, change.old_blob_oid)?;
        let new_bytes: Vec<u8> = read_blob_bytes(repo, change.new_blob_oid)?;
        let (insertions, deletions) = count_blob_diff_churn(&old_bytes, &new_bytes);
        entries.push(FileChurnEntry { path: change.path, insertions, deletions });
    }

    Ok(entries)
}

/// Computes `Unified/Split Row` diff items with full-blob AST `Coalesced Token Spans` for a file in a commit.
pub fn compute_file_diff(
    repo: &gix::Repository,
    commit_oid_hex: &str,
    file_path: &str,
) -> Result<Vec<DiffLineRow>, EngineError> {
    let commit_oid: ObjectId = parse_oid_hex(commit_oid_hex)?;
    let (old_tree_oid, new_tree_oid) = resolve_commit_and_parent_trees(repo, commit_oid)?;

    let mut old_blob_oid: Option<ObjectId> = match old_tree_oid {
        Some(tree_oid) => find_blob_in_tree_by_path(repo, tree_oid, file_path)?,
        None => None,
    };
    let new_blob_oid: Option<ObjectId> = find_blob_in_tree_by_path(repo, new_tree_oid, file_path)?;

    // If the file was renamed in this commit, locate its previous blob OID via rename pairing.
    if old_blob_oid.is_none()
        && new_blob_oid.is_some()
        && let Ok(changes) = collect_tree_changes(repo, old_tree_oid, Some(new_tree_oid))
        && let Some(renamed) = changes.into_iter().find(|c| c.path == file_path && c.old_path.is_some())
    {
        old_blob_oid = renamed.old_blob_oid;
    }

    let old_bytes: Vec<u8> = read_blob_bytes(repo, old_blob_oid)?;
    let new_bytes: Vec<u8> = read_blob_bytes(repo, new_blob_oid)?;

    Ok(diff_blobs_with_syntax(file_path, &old_bytes, &new_bytes))
}

pub(crate) fn find_blob_in_tree_by_path(
    repo: &gix::Repository,
    root_tree_oid: ObjectId,
    file_path: &str,
) -> Result<Option<ObjectId>, EngineError> {
    let clean_path: &str = file_path.trim_matches('/');
    if clean_path.is_empty() {
        return Ok(None);
    }

    let segments: Vec<&str> = clean_path.split('/').collect();
    let mut current_tree_oid: ObjectId = root_tree_oid;

    for (idx, segment) in segments.iter().enumerate() {
        let entries: BTreeMap<String, (EntryKind, ObjectId)> = read_tree_entries_map(repo, current_tree_oid)?;
        let Some((kind, oid)) = entries.get(*segment).copied() else {
            return Ok(None);
        };
        if idx + 1 == segments.len() {
            return Ok(is_blob_entry(kind).then_some(oid));
        }
        if kind != EntryKind::Tree {
            return Ok(None);
        }
        current_tree_oid = oid;
    }

    Ok(None)
}

/// Aligns two file blobs with `imara-diff` and zips them with full-blob `tree-sitter-highlight` AST spans.
pub fn diff_blobs_with_syntax(file_path: &str, old_bytes: &[u8], new_bytes: &[u8]) -> Vec<DiffLineRow> {
    if is_binary_or_oversized(old_bytes) || is_binary_or_oversized(new_bytes) {
        return vec![single_notice_row("Binary or oversized file (diff omitted)")];
    }

    let Ok(old_str) = std::str::from_utf8(old_bytes) else {
        return vec![single_notice_row("Non-UTF-8 file content (diff omitted)")];
    };
    let Ok(new_str) = std::str::from_utf8(new_bytes) else {
        return vec![single_notice_row("Non-UTF-8 file content (diff omitted)")];
    };

    let old_highlighted: Vec<Vec<CodeTokenSpan>> = highlight_blob_lines(file_path, old_str);
    let new_highlighted: Vec<Vec<CodeTokenSpan>> = highlight_blob_lines(file_path, new_str);

    let input: InternedInput<&str> = InternedInput::new(old_str, new_str);
    let mut raw_hunks: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    imara_diff::diff(Algorithm::Histogram, &input, |before: Range<u32>, after: Range<u32>| {
        raw_hunks.push(((before.start as usize)..(before.end as usize), (after.start as usize)..(after.end as usize)));
    });

    if raw_hunks.is_empty() {
        return Vec::new();
    }

    let grouped_hunks: Vec<Vec<(Range<usize>, Range<usize>)>> = group_nearby_hunks(&raw_hunks, DIFF_CONTEXT_LINES);
    let mut rows: Vec<DiffLineRow> = Vec::new();

    for group in grouped_hunks {
        let first_change: &(Range<usize>, Range<usize>) = &group[0];
        let last_change: &(Range<usize>, Range<usize>) = &group[group.len() - 1];

        let hunk_old_start: usize = first_change.0.start.saturating_sub(DIFF_CONTEXT_LINES);
        let hunk_old_end: usize = (last_change.0.end + DIFF_CONTEXT_LINES).min(old_highlighted.len());
        let hunk_new_start: usize = first_change.1.start.saturating_sub(DIFF_CONTEXT_LINES);
        let hunk_new_end: usize = (last_change.1.end + DIFF_CONTEXT_LINES).min(new_highlighted.len());

        let header_text: String = format!(
            "@@ -{},{} +{},{} @@",
            hunk_old_start + 1,
            hunk_old_end.saturating_sub(hunk_old_start),
            hunk_new_start + 1,
            hunk_new_end.saturating_sub(hunk_new_start)
        );
        let header_span: Vec<CodeTokenSpan> = vec![CodeTokenSpan { text: header_text, color: COLOR_HUNK_HEADER }];
        rows.push(DiffLineRow {
            old_line_no: None,
            new_line_no: None,
            kind: DiffLineKind::HunkHeader,
            unified_spans: header_span.clone(),
            left_line_no: None,
            left_kind: DiffLineKind::HunkHeader,
            left_spans: header_span.clone(),
            right_line_no: None,
            right_kind: DiffLineKind::HunkHeader,
            right_spans: header_span,
        });

        let mut cursor_old: usize = hunk_old_start;
        let mut cursor_new: usize = hunk_new_start;

        for (change_old, change_new) in group {
            while cursor_old < change_old.start && cursor_new < change_new.start {
                let old_spans: Vec<CodeTokenSpan> =
                    old_highlighted.get(cursor_old).cloned().unwrap_or_else(empty_line_spans);
                let new_spans: Vec<CodeTokenSpan> =
                    new_highlighted.get(cursor_new).cloned().unwrap_or_else(empty_line_spans);
                let old_no: Option<u32> = Some((cursor_old + 1) as u32);
                let new_no: Option<u32> = Some((cursor_new + 1) as u32);

                rows.push(DiffLineRow {
                    old_line_no: old_no,
                    new_line_no: new_no,
                    kind: DiffLineKind::Context,
                    unified_spans: new_spans.clone(),
                    left_line_no: old_no,
                    left_kind: DiffLineKind::Context,
                    left_spans: old_spans,
                    right_line_no: new_no,
                    right_kind: DiffLineKind::Context,
                    right_spans: new_spans,
                });
                cursor_old += 1;
                cursor_new += 1;
            }

            let removed_count: usize = change_old.len();
            let added_count: usize = change_new.len();
            let total_unified: usize = removed_count + added_count;

            for unified_idx in 0..total_unified {
                let (old_line_no, new_line_no, kind, unified_spans) = if unified_idx < removed_count {
                    let old_idx: usize = change_old.start + unified_idx;
                    let spans: Vec<CodeTokenSpan> =
                        old_highlighted.get(old_idx).cloned().unwrap_or_else(empty_line_spans);
                    (Some((old_idx + 1) as u32), None, DiffLineKind::Removed, spans)
                } else {
                    let new_idx: usize = change_new.start + (unified_idx - removed_count);
                    let spans: Vec<CodeTokenSpan> =
                        new_highlighted.get(new_idx).cloned().unwrap_or_else(empty_line_spans);
                    (None, Some((new_idx + 1) as u32), DiffLineKind::Added, spans)
                };

                let (left_line_no, left_kind, left_spans) = if unified_idx < removed_count {
                    let old_idx: usize = change_old.start + unified_idx;
                    let spans: Vec<CodeTokenSpan> =
                        old_highlighted.get(old_idx).cloned().unwrap_or_else(empty_line_spans);
                    (Some((old_idx + 1) as u32), DiffLineKind::Removed, spans)
                } else {
                    (None, DiffLineKind::EmptyPlaceholder, empty_line_spans())
                };

                let (right_line_no, right_kind, right_spans) = if unified_idx < added_count {
                    let new_idx: usize = change_new.start + unified_idx;
                    let spans: Vec<CodeTokenSpan> =
                        new_highlighted.get(new_idx).cloned().unwrap_or_else(empty_line_spans);
                    (Some((new_idx + 1) as u32), DiffLineKind::Added, spans)
                } else {
                    (None, DiffLineKind::EmptyPlaceholder, empty_line_spans())
                };

                rows.push(DiffLineRow {
                    old_line_no,
                    new_line_no,
                    kind,
                    unified_spans,
                    left_line_no,
                    left_kind,
                    left_spans,
                    right_line_no,
                    right_kind,
                    right_spans,
                });
            }

            cursor_old = change_old.end;
            cursor_new = change_new.end;
        }

        while cursor_old < hunk_old_end && cursor_new < hunk_new_end {
            let old_spans: Vec<CodeTokenSpan> =
                old_highlighted.get(cursor_old).cloned().unwrap_or_else(empty_line_spans);
            let new_spans: Vec<CodeTokenSpan> =
                new_highlighted.get(cursor_new).cloned().unwrap_or_else(empty_line_spans);
            let old_no: Option<u32> = Some((cursor_old + 1) as u32);
            let new_no: Option<u32> = Some((cursor_new + 1) as u32);

            rows.push(DiffLineRow {
                old_line_no: old_no,
                new_line_no: new_no,
                kind: DiffLineKind::Context,
                unified_spans: new_spans.clone(),
                left_line_no: old_no,
                left_kind: DiffLineKind::Context,
                left_spans: old_spans,
                right_line_no: new_no,
                right_kind: DiffLineKind::Context,
                right_spans: new_spans,
            });
            cursor_old += 1;
            cursor_new += 1;
        }
    }

    rows
}

fn group_nearby_hunks(
    hunks: &[(Range<usize>, Range<usize>)],
    context_lines: usize,
) -> Vec<Vec<(Range<usize>, Range<usize>)>> {
    let mut groups: Vec<Vec<(Range<usize>, Range<usize>)>> = Vec::new();
    let max_gap: usize = context_lines * 2;

    for hunk in hunks {
        if let Some(current_group) = groups.last_mut()
            && let Some(prev) = current_group.last()
            && hunk.0.start.saturating_sub(prev.0.end) <= max_gap
        {
            current_group.push(hunk.clone());
            continue;
        }
        groups.push(vec![hunk.clone()]);
    }

    groups
}

fn empty_line_spans() -> Vec<CodeTokenSpan> {
    vec![CodeTokenSpan { text: String::new(), color: COLOR_DEFAULT }]
}

fn single_notice_row(notice: &str) -> DiffLineRow {
    let spans: Vec<CodeTokenSpan> = vec![CodeTokenSpan { text: notice.to_owned(), color: COLOR_COMMENT }];
    DiffLineRow {
        old_line_no: None,
        new_line_no: None,
        kind: DiffLineKind::HunkHeader,
        unified_spans: spans.clone(),
        left_line_no: None,
        left_kind: DiffLineKind::HunkHeader,
        left_spans: spans.clone(),
        right_line_no: None,
        right_kind: DiffLineKind::HunkHeader,
        right_spans: spans,
    }
}
