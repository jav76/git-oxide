# Changelog

All notable changes to `git-oxide` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Linked worktree support is now covered by integration tests (branch, detached `HEAD`, subdirectory open, `extensions.worktreeConfig`). The top bar shows a `worktree of <main repo>` badge when a linked worktree is open.
- Click an error in the top bar status to see the full message in a popup where the text can be selected and copied.
- CI runs the engine test suite on Windows.

### Fixed

- Errors now include their full cause chain instead of only the outermost message. The misleading "repository has no commits at HEAD" error is replaced by the actual object database failure, or by "branch '<name>' has no commits yet" for a truly empty repository.
- Failures while listing refs are reported instead of silently producing an empty ref list.
- Phase 1 retries once after a short delay when git objects cannot be read, which happens on Windows while another git process repacks or locks pack files.
- Failures in the Phase 2 Churn Index, changed-file list, diff, file tree, and blame loaders are logged and shown in the status bar instead of being dropped.

## [0.1.0-alpha.1] - 2026-11-13

First public pre-release. Expect rough edges: the `OID Cache` schema, UI layout, and packaging may change before `0.1.0`.

### Added

- Two-crate Cargo workspace (`crates/engine` as `git-oxide-engine` and `crates/app` as `git-oxide`).
- Two-phase progressive indexing pipeline (`run_phase1` commit-graph walk + in-memory `tokei` language breakdown, and `run_phase2_churn` multi-threaded `rayon` tree-diff churn indexer).
- Persistent `redb` `OID Cache` (`COMMIT_CHURN_TABLE` and `BLOB_LANG_TABLE`) keyed by 20-byte SHA-1 commit and blob OIDs.
- Full-blob AST syntax highlighting (`tree-sitter-highlight`) across 15 bundled grammars with adjacent same-color token span coalescing and 1 MB blob / 500-character line guardrails.
- On-demand `imara-diff` (`Algorithm::Histogram`) commit inspection supporting both Unified and Side-by-Side (Split) diff views plus file rename detection.
- Repository tree browser and `gix::blame::file` line attribution view paired with AST-highlighted source spans.
- Slint desktop UI with `TopBar`, `ActivityRail`, 5-panel `AnalyticsView` (including `plotters` `SharedPixelBuffer` churn/velocity chart and interactive 52-week heatmap), `CommitsDiffView`, and `FilesBlameView`.
- Collapsible Language, Heatmap, Churn, and Leaderboard regions in the Analytics view, with a collapse/expand-all toggle.
- Cross-platform `Release` workflow built in one `debian:bookworm` container: Linux `amd64`/`arm64` `.deb`, `.rpm`, and `.tar.gz`, Windows `x64` `.msi` and `.zip`, Windows `arm64` `.zip`, and a `SHA256SUMS` manifest.
- Linux desktop entry and scalable icon (`io.github.jav76.GitOxide`), a `cargo-binstall` manifest, and `scripts/bump-version.sh` for version bumps.

### Performance

- Phase 2 churn indexing processes commits in batches of 2,000 with 64-commit temporally adjacent `rayon` sub-chunks and 32 MiB thread-local `gix` object caches, keeping very large histories (for example, the Linux kernel) responsive.

### Known limitations

- Linux binaries need glibc 2.35 or newer (Debian 12+, Ubuntu 22.04+, Fedora 36+).
- Binaries are unsigned. Windows SmartScreen may warn on first launch of the `.msi` or `.exe`.
- Windows `arm64` ships as a portable `.zip` only (no `.msi`).

[Unreleased]: https://github.com/jav76/git-oxide/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/jav76/git-oxide/releases/tag/v0.1.0-alpha.1
