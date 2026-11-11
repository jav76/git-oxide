# git-oxide Agent Guidelines & Standards

This document defines core conventions and operational rules for AI coding assistants working within the **git-oxide** repository.

---

## 1. Quick Reference & Commands

- **Format Check**: `cargo fmt --all -- --check`
- **Auto-Format**: `cargo fmt --all`
- **Lint Check**: `cargo clippy --workspace --all-targets -- -D warnings`
- **Test Suite**: `cargo test --workspace`
- **Release Build**: `cargo build --workspace --release`

---

## 2. Agent Rule Files

Detailed rules and operational standards are located in `.agents/`:
- **Code Style & Rust/Slint Standards**: [.agents/rules/code-style.md](file:///home/jaret/Documents/GitHub/git-oxide/.agents/rules/code-style.md)
- **Logging & Diagnostic Standards**: [.agents/rules/logging.md](file:///home/jaret/Documents/GitHub/git-oxide/.agents/rules/logging.md)
- **Knowledge Graph & Architecture Context**: [.agents/rules/understand-context.md](file:///home/jaret/Documents/GitHub/git-oxide/.agents/rules/understand-context.md)
- **Architectural Code Reviewer Agent**: [.agents/agents/code-reviewer.md](file:///home/jaret/Documents/GitHub/git-oxide/.agents/agents/code-reviewer.md)
- **Code Reviewer Skill**: [.agents/skills/code-reviewer/SKILL.md](file:///home/jaret/Documents/GitHub/git-oxide/.agents/skills/code-reviewer/SKILL.md)
- **Architecture Designer Skill**: [.agents/skills/architecture-designer/SKILL.md](file:///home/jaret/Documents/GitHub/git-oxide/.agents/skills/architecture-designer/SKILL.md)

---

## 3. Core Architectural Boundaries

1. **Core Analytics & Git Engine (`crates/engine`)**: Pure-Rust library (`git-oxide-engine`) handling `gix` commit-graph traversal, `redb` OID caching (`commit_oid` and `blob_oid`), `tokei` in-memory blob language statistics, `imara-diff` Unified/Split diff construction, `gix` file blame, and `tree-sitter-highlight` AST span coalescing. Zero GUI dependencies.
2. **Desktop Presentation & Controller (`crates/app`)**: Slint UI desktop binary (`git-oxide`), `plotters` time-series rasterizer (`SharedPixelBuffer`), and non-blocking background worker orchestration (`slint::invoke_from_event_loop`).

---

## 4. Key Invariants & Non-Negotiables

1. **Explicit Type Annotations**: Annotate types on `let` bindings whenever the type is not immediately obvious from the right-hand side.
2. **No Magic Numbers**: All buffer sizes, thresholds, AST guardrails, batch sizes, and palette colors must be declared as `SCREAMING_SNAKE_CASE` constants (`const`).
3. **No `unwrap()` Outside Tests**: Use typed `EngineError` in `crates/engine` and `anyhow::Context` in `crates/app`.
4. **No Direct Console Output**: Never use `println!`, `eprintln!`, or `dbg!`; route all diagnostics through `log` macros.
5. **Non-Blocking UI Thread**: Never perform Git pack I/O, tree diffing, or `redb` queries synchronously on the Slint event loop thread.
6. **No Em-Dashes**: Never use em-dashes in code comments, documentation, commit messages, or UI strings.
