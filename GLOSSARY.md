# Glossary

- **Ref Target**: The immutable 20-byte commit OID resolved from `HEAD`, a local/remote branch, or a tag against which all repository analytics, commit history, and tree browsing are currently scoped.
  - _Avoid_: current branch, active checkout, working head.
- **Phase 1 Snapshot**: The fast initial repository scan (commit DAG traversal, author commit counts, 52-week commit heatmap, and target-tree language breakdown) that completes in milliseconds without diffing historical parent trees.
  - _Avoid_: quick scan, initial load, metadata pass.
- **Phase 2 Churn Index**: The background multi-threaded (`rayon`) tree-to-tree diff pass that computes insertions, deletions, and touched file paths for every reachable commit OID and persists them in `redb`.
  - _Avoid_: deep scan, full index, background diffing.
- **OID Cache**: The persistent embedded `redb` database keyed exclusively by immutable 20-byte commit OIDs (`COMMIT_CHURN_TABLE`) and blob OIDs (`BLOB_LANG_TABLE`).
  - _Avoid_: repo database, stat cache, local storage.
- **Coalesced Token Span**: A contiguous slice of a single source line sharing an identical foreground RGBA color after `tree-sitter-highlight` AST evaluation, merged in Rust to bound the number of Slint `Text` elements per virtualized row.
  - _Avoid_: syntax token, styled text chunk, highlight segment.
- **Unified/Split Row**: A single row model element in the diff `ListView` carrying both left (old) and right (new) line numbers, diff kinds, and Coalesced Token Spans so one virtualized list renders both Unified and Side-by-Side diff layouts.
  - _Avoid_: diff line, hunk line, dual row.
- **File Hotspot**: A repository file path ranked by cumulative historical commit touch count and total line churn (`insertions + deletions`) across the reachable commit graph.
  - _Avoid_: churned file, busy file, top file.
