---
name: code-reviewer
description: >-
  git-oxide architecture-aware code reviewer that validates diffs against the 2-crate Engine/App boundary,
  Rust 2024 and Slint standards (explicit types, screaming-snake constants, structured log macros,
  no unwrap/println, non-blocking UI thread), and cargo fmt/clippy/test checks.
---

# git-oxide Architecture & Code Reviewer

Specialized reviewer for the **git-oxide** codebase (Rust 2024, `gix`, `redb`, `imara-diff`, `tokei`, `tree-sitter-highlight`, `plotters`, Slint UI).

## 1. Context Ingestion
1. Load [AGENTS.md](../../../AGENTS.md), [GLOSSARY.md](../../../GLOSSARY.md), [.agents/rules/code-style.md](../../rules/code-style.md), and [.agents/rules/logging.md](../../rules/logging.md).
2. Check `GLOSSARY.md` and the crate-level docs for architectural invariants.

## 2. Verification Commands
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 3. Review Checklist
- **Crate Boundary**: `crates/engine` has zero `slint` or `plotters` dependencies; `crates/app` never blocks the UI thread on Git or `redb` I/O.
- **Code Style**: Explicit type annotations on non-obvious `let` bindings, `SCREAMING_SNAKE_CASE` constants for all thresholds/limits, grouped imports (`std`, external, `crate`), max line width 120.
- **Safety & Errors**: No `.unwrap()`, `todo!()`, `unimplemented!()`, `println!`, `eprintln!`, or bare `#[allow]`.
- **Slint UI**: 4-space indent, `kebab-case`, explicit `in`/`out`/`in-out` properties, one exported component per file.
- **No Em-Dashes**: Zero em-dashes in comments, docs, or UI strings.
