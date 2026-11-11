---
trigger: always_on
---

# git-oxide Code Style & Modern Rust / Slint Standards

Adhere to the following conventions when authoring, modifying, or refactoring Rust and Slint code across the `git-oxide` workspace.

---

## 1. Type Declarations & Explicit Annotations
- **Explicit Types on Non-Obvious Bindings**: Always annotate the explicit type on `let` bindings whenever the type is not immediately obvious from the right-hand side constructor or literal (`clippy::redundant_type_annotations` is intentionally allowed in workspace lints):
  ```rust
  // Correct
  let commit_count: usize = commits.len();
  let mut rows: Vec<DiffLineRow> = Vec::with_capacity(line_count);
  let stats: CommitChurnStats = compute_commit_churn(&repository, commit_oid)?;

  // Avoid when type is not obvious on the RHS
  let stats = compute_commit_churn(&repository, commit_oid)?;
  ```

---

## 2. Imports, Modules & Visibility
- **Import Grouping**: Group `use` declarations in three distinct blocks separated by a blank line:
  1. `std` imports
  2. External crate imports (`anyhow`, `gix`, `imara_diff`, `redb`, `slint`, `tokei`, `tree_sitter_highlight`)
  3. Internal `crate` / `super` imports
- **No Wildcard Imports**: Avoid glob imports (`use foo::*;`) except `use super::*;` inside `#[cfg(test)] mod tests`.
- **Explicit Visibility**: Keep module items private by default; expose crate-internal APIs with `pub(crate)` and public engine contracts with `pub`.

---

## 3. Line Width, Layout & Formatting
- **Max Line Width**: Target line widths under 120 characters (`max_width = 120` in `rustfmt.toml` and `.editorconfig`).
- **Indentation**: 4 spaces for `.rs` and `.slint` files; 2 spaces for `.toml`, `.json`, and `.yml` files; Unix `LF` line endings everywhere.
- **Small Single-Purpose Functions**: Keep functions focused. If a function requires section divider comments, split it into named helper functions.

---

## 4. Naming & Constants (No Magic Numbers)
- **Spelled-Out Identifiers**: Use descriptive names (`repository`, `commit_oid`, `window`, `error`, `highlight_config`) rather than single-letter abbreviations (`r`, `c`, `w`, `e`). Short names are acceptable only in trivial single-line iterator closures.
- **Constants & Magic Numbers**: All constants (`const` and `static`) must use `SCREAMING_SNAKE_CASE` (e.g., `pub const MAX_HIGHLIGHT_BLOB_BYTES: usize = 1_048_576;`, `const CHURN_BATCH_SIZE: usize = 250;`). Avoid magic numbers for buffer capacities, AST size/line caps, batch intervals, RGBA color channels, or chart padding.
- **Domain Terminology**: Use the canonical terms from `GLOSSARY.md` (`Ref Target`, `Phase 1 Snapshot`, `Phase 2 Churn Index`, `OID Cache`, `Coalesced Token Span`, `Unified/Split Row`, `File Hotspot`).

---

## 5. Error Handling & Safety
- **Engine vs. App Errors**:
  - `crates/engine` returns `Result<T, EngineError>` using a typed `thiserror::Error` enum.
  - `crates/app` uses `anyhow::Result<T>` and attaches `.context("lowercase description of failed action")` at every fallible boundary (no trailing period).
- **No `unwrap()` or `todo!()`**:
  - Never use `.unwrap()`, `todo!()`, or `unimplemented!()` in non-test code.
  - `.expect("reason")` is permitted only for true structural invariants and the message must state why the invariant holds.
- **Lint Suppression**: Never use a bare `#[allow(...)]`. When a clippy lint must be suppressed locally, use `#[expect(lint_name, reason = "why this is safe and intentional")]`.

---

## 6. Slint UI Conventions (`crates/app/ui`)
- **Component Structure**: 4-space indentation, `kebab-case` property and callback names, one exported component per `.slint` file.
- **Explicit Property Direction**: Always declare `in`, `out`, or `in-out` on component properties (defaulting to `in`; use `in-out` only when mutated by both Rust and the UI).
- **Declarative Markup Only**: Keep `.slint` files free of business logic. Filtering, sorting, diff alignment, and chart rasterization belong in Rust; the UI raises verb-named callbacks (`select-commit`, `select-file`, `toggle-diff-mode`, `filter-by-author`).
- **Logical Pixels & Accessibility**: Express all dimensions in logical pixels (`px`) and include `accessible-role` and `accessible-label` on custom interactive controls.

---

## 7. Comments & Prose
- **Why, Not What**: Only add comments where the intent or algorithmic constraint is not obvious from the code (such as full-blob AST alignment before hunk slicing or `redb` batch transaction sizing).
- **No Em-Dashes**: Never use em-dashes in comments, docstrings, markdown docs, or UI strings. Use hyphens, colons, parentheses, or separate sentences.
