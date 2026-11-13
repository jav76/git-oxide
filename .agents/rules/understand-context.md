---
trigger: always_on
---

# Understand-Anything & Architectural Context Rules

- **Prioritize Knowledge Graph & Domain Context**: Before performing manual codebase research, multiple file scans, or answering natural-language questions about system architecture, components, or data flows, check if `graphify-out/graph.json`, `.ua/knowledge-graph.json`, or `.ua/domain-graph.json` exists in the workspace root.
- **Architectural Layers & Crate Seam**: Enforce the strict two-crate boundary:
  1. `crates/engine` (`git-oxide-engine`): Pure-Rust Git traversal (`gix`), `redb` OID Cache, `tokei` blob language analysis, `imara-diff` Unified/Split diffing, `gix` blame, and `tree-sitter-highlight` AST span coalescing. Must never depend on `slint` or UI crates.
  2. `crates/app` (`git-oxide`): Slint UI views, `plotters` `SharedPixelBuffer` chart rendering, and background thread orchestration (`slint::invoke_from_event_loop`).
- **Domain Glossary**: Consult [GLOSSARY.md](file:///home/jaret/Documents/GitHub/git-oxide/GLOSSARY.md) before introducing new domain concepts or altering core indexing, caching, or highlighting pipelines.
