# Contributing to `git-oxide`

Thank you for your interest in contributing to `git-oxide`!

---

## Quickstart for Contributors

### 1. Prerequisites
- **Rust Toolchain** (`1.92+`, Rust 2024 edition) with `rustfmt` and `clippy` components installed.
- **C/C++ Compiler Toolchain** (`gcc` or `clang`) for compiling bundled `tree-sitter` language grammars.
- **Linux System Libraries**: `pkg-config` and `libfontconfig1-dev` (used by Slint's desktop renderer).

### 2. Clone & Build
```bash
git clone https://github.com/jav76/git-oxide.git
cd git-oxide
cargo build --workspace
```

### 3. Verify Formatting, Lints, and Tests
```bash
# Check formatting
cargo fmt --all -- --check

# Run strict workspace and pedantic Clippy checks
cargo clippy --workspace --all-targets --locked -- -D warnings

# Run engine and app test suites
cargo test --workspace --locked

# Verify documentation builds with zero broken links
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
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

## Release & Self-Hosted Runner Workflow

`git-oxide` follows [Semantic Versioning](https://semver.org/) with `[workspace.package].version` in `Cargo.toml` as the single source of truth:

1. **Prepare a release**:
   ```bash
   ./scripts/bump-version.sh 0.2.0
   ```
   Update the newly created `## [0.2.0]` section in `CHANGELOG.md` and commit both files to `main`.
2. **Tag and trigger release builds**:
   ```bash
   git tag v0.2.0
   git push origin main v0.2.0
   ```
   The `Release` workflow builds inside `debian:bookworm` (`linux/amd64`, `linux/arm64` `.deb`/`.rpm`/`.tar.gz`, plus Windows `x64` `.msi`/`.zip` and `arm64` `.zip`), attaches `SHA256SUMS`, and publishes the GitHub Release atomically.
3. **Self-hosted runner setup**:
   ```bash
   ./scripts/setup-runner.sh
   ```
   Registers a `[self-hosted, Linux, X64, git-oxide]` runner and configures `vars.RUNNER_LABELS` (if unset, workflows fall back to `ubuntu-latest`).

---

## Submitting a Pull Request

1. Ensure `cargo fmt --all -- --check` passes with zero diffs.
2. Ensure `cargo clippy --workspace --all-targets --locked -- -D warnings` passes with zero warnings.
3. Ensure `cargo test --workspace --locked` passes.
4. Ensure `CHANGELOG.md` has a matching section whenever `Cargo.toml` version changes.
5. Include a clear PR summary, test evidence, and merge danger assessment (two-way vs. one-way door and blast radius).
