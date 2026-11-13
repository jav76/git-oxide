# Agent guide

Rules for any coding agent (Claude Code, Copilot, Antigravity, Codex) working in this repository.
Tooling enforces most of the formatting; this file and `.agents/rules/` cover what tools cannot.
When a rule here conflicts with a tool, fix the tool config instead of working around it.

## Commands

Run these before calling a change done:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- `cargo run -p git-oxide -- <path-to-repo>` launches the desktop Git analytics viewer against `<path-to-repo>` (defaults to the current directory).
- Formatting comes from `rustfmt.toml`, `.editorconfig`, and `.gitattributes` (LF everywhere). Use stable rustfmt only, with no nightly-only options.
- Lints are configured once in the root `Cargo.toml` under `[workspace.lints]`. Every crate opts in with `[lints] workspace = true`.

## Repository map

| Path | What lives there |
| :--- | :--- |
| `crates/engine` | Pure-Rust Git analytics, `redb` OID Cache, `imara-diff`, `gix-blame`, `tokei`, and `tree-sitter-highlight` engine (zero UI dependencies) |
| `crates/app` | Slint UI binary (`git-oxide`), `plotters` chart rasterizer, and background controller |
| `crates/app/ui` | Declarative Slint markup (`.slint`), compiled by `crates/app/build.rs` |
| `.agents/rules` | Always-on repository coding, logging, and architectural context rules |
| `.agents/agents` | Repository-specific code review agent configuration |
| `GLOSSARY.md` | Domain terms (`Ref Target`, `Phase 1 Snapshot`, `Phase 2 Churn Index`, `OID Cache`, `Coalesced Token Span`, `Unified/Split Row`, `File Hotspot`) |

## Rust

**Language and toolchain**
- Edition 2024, MSRV from `rust-version` in the root `Cargo.toml`.
- Prefer modern syntax: `let ... else`, let chains, inline format args (`"{name}"`), `is_none_or`, and `LazyLock`.

**Errors**
- `crates/engine` exposes a typed `EngineError` (`thiserror`) so callers can match on domain failures.
- `crates/app` uses `anyhow::Result` with `.context("lowercase description of failed action")` at every fallible boundary (no trailing period).
- No `unwrap()` outside tests. `expect("...")` is allowed only for true invariants, and the message must state the invariant.
- No `todo!()` or `unimplemented!()` in committed code.
- Log recoverable failures once, where they are handled, through the `log` macros (`log::warn!`, `log::error!`, `log::info!`, `log::debug!`, `log::trace!`). Never use `println!` or `eprintln!` (workspace clippy lints forbid both) and never log and also return the same error.

**Style**
- Annotate the explicit type on `let` bindings whose type is not immediately obvious from the right-hand side (`clippy::redundant_type_annotations` is allowed for this house style).
- Spell variable and parameter names out (`commit_oid`, `repository`, `window`, `error`, not `c`, `r`, `w`, `e`). Short names are fine for simple iterator closures.
- Declare all non-obvious numeric values (buffer sizes, AST size/line limits, batch counts, palette channels, chart dimensions) as descriptive `SCREAMING_SNAKE_CASE` constants (`const`). Avoid magic numbers.
- Group imports as `std`, then external crates, then `crate`/`super`, with a blank line between groups. Avoid glob imports except `use super::*;` in test modules.
- Keep functions small and single-purpose. Target max line width of 120 characters.
- Silence a lint locally only with `#[expect(lint, reason = "...")]`, never a bare `#[allow]`.
- Comments explain why, not what. Do not add comments that restate the code.

**Structure & Threading**
- `crates/engine` must never depend on `slint`, `plotters`, or windowing libraries.
- Functions in `crates/app` that must run on the UI thread say so in their module or item docs. Hand work back to the UI thread with `slint::invoke_from_event_loop` or `slint::Weak::upgrade_in_event_loop`. Never block the UI thread on Git pack I/O, tree diffs, or `redb` transactions; spawn named background threads (`git-oxide-<purpose>`).

## Slint (`crates/app/ui`)

- 4-space indentation, `kebab-case` names, one exported component per `.slint` file.
- Declare property direction explicitly (`in`, `out`, `in-out`). Default to `in`. Use `in-out` only when both Rust and the UI write it.
- Keep markup declarative. Git traversal, diffing, sorting, and chart math belong in Rust; the UI raises callbacks named as verbs (`select-commit`, `select-file`, `filter-by-author`).
- Sizes use logical pixels (`px`).
- Custom interactive elements set `accessible-role` and `accessible-label`.

## Docs, commits, and PRs

- Keep `README.md` and `GLOSSARY.md` in sync with behavior and domain terminology.
- Hard-to-reverse decisions are recorded as `docs/adr/NNNN-short-title.md`, which is gitignored: design decisions stay out of the public repository. Never force-add files under `docs/adr`.
- Write concise, practical prose. Never use em-dashes: use hyphens, colons, parentheses, or separate sentences.
- Commit messages use Conventional Commits (`feat(engine): ...`, `feat(app): ...`, `fix(engine): ...`, `docs: ...`), imperative mood, under 72 characters.
- Never add AI attribution lines (`Co-Authored-By`, `Generated with`) to commits or PRs.
