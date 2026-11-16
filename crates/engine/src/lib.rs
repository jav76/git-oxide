//! Core read-only Git analytics, diff, blame, and AST syntax highlighting engine.

pub mod cache;
pub mod diff;
pub mod error;
pub mod indexer;
pub mod models;
pub mod repository;
pub mod syntax;
pub mod tree_blame;

use std::path::Path;

use crate::diff::{compute_file_diff, list_commit_changed_files};
use crate::error::EngineError;
use crate::models::{BlameLineRow, ChangedFileStat, DiffLineRow, TreeEntryItem};
use crate::repository::open_repository;
use crate::tree_blame::{compute_file_blame, list_tree_entries, read_file_source_lines};

/// Opens a repository at `repo_path` and lists changed files for `commit_oid_hex`.
pub fn load_commit_changed_files(repo_path: &Path, commit_oid_hex: &str) -> Result<Vec<ChangedFileStat>, EngineError> {
    let repo: gix::Repository = open_repository(repo_path)?;
    list_commit_changed_files(&repo, commit_oid_hex)
}

/// Opens a repository at `repo_path` and computes Unified/Split AST-highlighted diff rows for a file.
pub fn load_commit_file_diff(
    repo_path: &Path,
    commit_oid_hex: &str,
    file_path: &str,
) -> Result<Vec<DiffLineRow>, EngineError> {
    let repo: gix::Repository = open_repository(repo_path)?;
    compute_file_diff(&repo, commit_oid_hex, file_path)
}

/// Opens a repository at `repo_path` and lists all tracked tree entries at `commit_oid_hex`.
pub fn load_ref_tree_entries(repo_path: &Path, commit_oid_hex: &str) -> Result<Vec<TreeEntryItem>, EngineError> {
    let repo: gix::Repository = open_repository(repo_path)?;
    list_tree_entries(&repo, commit_oid_hex)
}

/// Opens a repository at `repo_path` and computes AST-highlighted source lines without blame.
pub fn load_ref_file_source(
    repo_path: &Path,
    commit_oid_hex: &str,
    file_path: &str,
) -> Result<Vec<BlameLineRow>, EngineError> {
    let repo: gix::Repository = open_repository(repo_path)?;
    read_file_source_lines(&repo, commit_oid_hex, file_path)
}

/// Opens a repository at `repo_path` and computes AST-highlighted blame lines for `file_path`.
pub fn load_ref_file_blame(
    repo_path: &Path,
    commit_oid_hex: &str,
    file_path: &str,
) -> Result<Vec<BlameLineRow>, EngineError> {
    let repo: gix::Repository = open_repository(repo_path)?;
    compute_file_blame(&repo, commit_oid_hex, file_path)
}
