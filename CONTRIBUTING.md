# Contributing to `git-oxide`

Thank you for your interest in contributing to `git-oxide`!

---

## Quickstart for Contributors

### 1. Prerequisites
- **Rust Toolchain** (`1.85+`, Rust 2024 edition) with `rustfmt` and `clippy` components installed.
- **C/C++ Compiler Toolchain** (`gcc` or `clang`) for compiling bundled `tree-sitter` language grammars.
- **Linux System Libraries**: `pkg-config` and `libfontconfig1-dev` (used by Slint's desktop renderer).

### 2. Clone & Build
```bash
git clone https://github.com/<your-username>/git-oxide.git
cd git-oxide
cargo build --workspace
```

### 3. Verify Formatting, Lints, and Tests
```bash
# Check formatting
cargo fmt --all -- --check

# Run strict workspace and pedantic Clippy checks
cargo clippy --workspace --all-targets -- -D warnings

# Run engine and app test suites
cargo test --workspace
```

### 4. Running Locally
```bash
# Inspect the current repository or pass a path to any local Git repository
cargo run --release -- /path/to/repository
```

---

## Coding Standards

`git-oxide` enforces strict Rust and Slint conventions (see [.editorconfig](.editorconfig), [rustfmt.toml](rustfmt.toml), [clippy.toml](clippy.toml), and [.agents/rules/code-style.md](.agents/rules/code-style.md)):
- **Crate Boundary**: `crates/engine` (`git-oxide-engine`) must remain pure Rust with zero Slint or GUI dependencies. All Slint model conversions (`VecModel`, `ModelRc`, `SharedString`, `SharedPixelBuffer`) belong in `crates/app`.
- **Error Handling**: Use typed `thiserror` errors (`EngineError`) in `crates/engine` and `anyhow::Result` with `.context(...)` at top-level application entry points. Never `.unwrap()` fallible I/O or Git object lookups in production paths.
- **Naming**: `snake_case` for functions, methods, modules, and variables; `PascalCase` for types, traits, and enums; `SCREAMING_SNAKE_CASE` for constants and `redb` `TableDefinition`s.
- **UI Thread Discipline**: Never run `gix` traversals, `imara-diff` hunks, `tokei` scans, or `tree-sitter-highlight` passes on the Slint UI thread. Dispatch work to background threads and marshal updates via `slint::invoke_from_event_loop`.
- **Documentation & Domain Terms**: Use the canonical terms defined in [GLOSSARY.md](GLOSSARY.md) and document irreversible architectural changes in `docs/adr/`.

---

## Submitting a Pull Request

1. Ensure `cargo fmt --all -- --check` passes with zero diffs.
2. Ensure `cargo clippy --workspace --all-targets -- -D warnings` passes with zero warnings.
3. Ensure `cargo test --workspace` passes.
4. Update [GLOSSARY.md](GLOSSARY.md) or `docs/adr/` if domain concepts or architectural boundaries change.
5. Include a clear PR summary, test evidence, and merge danger assessment (two-way vs. one-way door and blast radius).
