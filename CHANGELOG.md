# Changelog

All notable changes to `git-oxide` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-11

### Added

- Two-crate Cargo workspace (`crates/engine` as `git-oxide-engine` and `crates/app` as `git-oxide`).
- Two-phase progressive indexing pipeline (`run_phase1` commit-graph walk + in-memory `tokei` language breakdown, and `run_phase2_churn` multi-threaded `rayon` tree-diff churn indexer).
- Persistent `redb` `OID Cache` (`COMMIT_CHURN_TABLE` and `BLOB_LANG_TABLE`) keyed by 20-byte SHA-1 commit and blob OIDs.
- Full-blob AST syntax highlighting (`tree-sitter-highlight`) across 15 bundled grammars with adjacent same-color token span coalescing and 1 MB blob / 500-character line guardrails (`ADR-0002`).
- On-demand `imara-diff` (`Algorithm::Histogram`) commit inspection supporting both Unified and Side-by-Side (Split) diff views plus file rename detection.
- Repository tree browser and `gix::blame::file` line attribution view paired with AST-highlighted source spans.
- Slint desktop UI with `TopBar`, `ActivityRail`, 5-panel `AnalyticsView` (including `plotters` `SharedPixelBuffer` churn/velocity chart and interactive 52-week heatmap), `CommitsDiffView`, and `FilesBlameView`.
