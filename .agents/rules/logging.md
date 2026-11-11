---
trigger: always_on
---

# git-oxide Logging & Diagnostic Standards

Adhere to the following conventions when authoring, modifying, or refactoring diagnostics and logging across the `git-oxide` workspace.

---

## 1. Core Facade & Forbidden Macros
- **`log` Crate Facade**: All diagnostic output in `crates/engine` and `crates/app` must flow through the standard `log` macros (`log::trace!`, `log::debug!`, `log::info!`, `log::warn!`, `log::error!`).
- **No Direct Standard Output**: Never use `println!`, `eprintln!`, or `dbg!` outside tests. Workspace Clippy lints (`print_stdout = "warn"`, `print_stderr = "warn"`, `dbg_macro = "warn"`) enforce this rule.

---

## 2. Log Once & Contextual Formatting
- **Log Where Handled**: Log recoverable failures once at the boundary where they are handled or degraded, never both logging an error and returning the same error up the call stack.
- **Structured Context**: Include relevant identifiers (repository path, `commit_oid`, `file_path`, elapsed milliseconds, batch count) using inline format arguments:
  ```rust
  log::info!(
      "completed Phase 1 snapshot for {repo_path} at {target_oid} ({commit_count} commits in {elapsed_ms} ms)"
  );
  log::warn!("failed to compute blame for {file_path} at {commit_oid}: {error:#}");
  ```

---

## 3. Log Level Guidelines

| Level | Semantic Meaning & When to Use |
| :--- | :--- |
| **Trace** | Granular per-blob language parsing, AST token span coalescing counts, or `ListView` model slicing. |
| **Debug** | `redb` OID Cache hit/miss ratios, Phase 2 batch commit progress, and chart pixel buffer rasterization dimensions. |
| **Info** | High-level lifecycle milestones (repository opened, Phase 1 Snapshot completed, Phase 2 Churn Index finished, Ref Target switched). |
| **Warn** | Recoverable anomalies (skipped corrupt pack entry, AST highlight fallback on oversized blob, cancelled background worker). |
| **Error** | Unhandled repository open failure, `redb` storage failure, or UI event-loop dispatch error. |

---

## 4. Zero-Overhead Guards for Expensive Diagnostics
- When computing diagnostic summaries that require extra allocations or iteration, guard the block with `log::log_enabled!(log::Level::Debug)` so disabled levels incur zero overhead.
