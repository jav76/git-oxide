# git-oxide

High-performance, strictly read-only desktop Git analytics, diff, and blame viewer built in Rust with **Slint UI** and **Gitoxide (`gix`)**.

## Overview

`git-oxide` parses `.git` packfiles, commit graphs, and trees directly in Rust without spawning `git` subprocesses. All analytics, diffs, and language breakdowns are keyed off immutable 20-byte commit and blob OIDs and cached in a persistent `redb` `OID Cache`.

### Key Capabilities

- **Two-Phase Progressive Indexing (`gix` + `rayon` + `redb`)**:
  - **Phase 1 Snapshot**: Walks the commit DAG from the active `Ref Target` (`HEAD`, branch, or tag), aggregates contributor counts, builds the 52-week commit heatmap, and computes in-memory blob language statistics via `tokei::LanguageType::parse_from_slice`.
  - **Phase 2 Churn Index**: Streams tree-to-tree diff churn (`+insertions`, `-deletions`, and touched paths) across all CPU cores using `rayon` and persists results in `redb` (`~/.cache/git-oxide/oid_cache_v1.redb`).
- **5-Panel Analytics Dashboard**:
  1. **Language Breakdown**: Stacked proportion bar and table of files, code, comment, and blank lines per language.
  2. **52-Week Commit Activity Heatmap**: Interactive calendar grid with click-to-filter commit drill-down.
  3. **Code Churn & Commit Velocity Timeline**: High-density Weekly/Monthly `+insertions` and `-deletions` bar/line plot rasterized into a `slint::SharedPixelBuffer` via `plotters`.
  4. **Contributor Leaderboard**: Ranked by commit count and line churn; clicking any contributor filters the commit log.
  5. **File Hotspots**: Ranked by historical touch count and total churn; clicking any file jumps straight to its `Git Blame` view.
- **Virtualized Unified & Side-by-Side Diffs (`imara-diff`)**:
  - Computes diffs on demand using the Histogram algorithm and renders both **Unified** and **Side-by-Side (Split)** modes in a single virtualized Slint `ListView` with old and new line numbers.
- **Full-Blob AST Syntax Highlighting (`tree-sitter-highlight`)**:
  - Parses full old and new blobs into syntax trees before aligning diff or blame lines, merges adjacent same-color tokens into `Coalesced Token Spans`, and enforces a 1 MB blob and 500-character line guardrail (`ADR-0002`).
- **Repository File Browser & Git Blame (`gix-blame`)**:
  - Browse the repository tree at any branch or tag, toggle between **Git Blame** and **Source Only** views, and click any blame gutter SHA to jump directly to the originating commit diff.

## Workspace Layout

| Path | Description |
| :--- | :--- |
| `crates/engine` | Pure-Rust Git analytics, `redb` `OID Cache`, `imara-diff`, `gix-blame`, `tokei`, and `tree-sitter-highlight` library (`git-oxide-engine`) |
| `crates/app` | Slint UI desktop binary (`git-oxide`), `plotters` chart rasterizer, and background thread controller |
| `crates/app/ui` | Declarative `.slint` components and views |
| `docs/adr` | Architecture Decision Records (`0001` through `0003`) |
| `GLOSSARY.md` | Canonical domain terminology |
| `.agents/` | Repository-specific coding, logging, and architectural review rules |

## Building & Running

```bash
# Run against a target repository (or omit path to inspect the current directory)
cargo run --release -p git-oxide -- /path/to/repo
```

### Keyboard Shortcuts

- `Ctrl+1`: Switch to **Analytics** view
- `Ctrl+2`: Switch to **Commits & Diffs** view
- `Ctrl+3`: Switch to **Files & Blame** view
- `Ctrl+O`: Open repository folder picker

## Verification Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
