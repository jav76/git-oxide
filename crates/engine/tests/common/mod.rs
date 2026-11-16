//! Shared git fixture helpers for engine integration tests.

use std::fs;
use std::path::Path;
use std::process::Command;

pub fn run_git(repo_dir: &Path, args: &[&str], env_vars: &[(&str, &str)]) {
    let mut cmd: Command = Command::new("git");
    cmd.current_dir(repo_dir).args(args);
    for (key, val) in env_vars {
        cmd.env(key, val);
    }
    let output = cmd.output().expect("git binary must be available in test environment");
    assert!(output.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&output.stderr));
}

pub fn create_fixture_repository(repo_dir: &Path) {
    run_git(repo_dir, &["init", "-b", "main"], &[]);
    run_git(repo_dir, &["config", "user.name", "Alice Dev"], &[]);
    run_git(repo_dir, &["config", "user.email", "alice@example.com"], &[]);

    let src_dir = repo_dir.join("src");
    let scripts_dir = repo_dir.join("scripts");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&scripts_dir).unwrap();

    fs::write(src_dir.join("lib.rs"), "// Initial library module\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n")
        .unwrap();
    fs::write(
        scripts_dir.join("helper.py"),
        "# Helper script\ndef greet(name: str) -> str:\n    return f\"hello {name}\"\n",
    )
    .unwrap();

    run_git(repo_dir, &["add", "."], &[]);
    run_git(
        repo_dir,
        &["commit", "-m", "feat: initial rust and python modules"],
        &[("GIT_AUTHOR_DATE", "2026-10-01T12:00:00Z"), ("GIT_COMMITTER_DATE", "2026-10-01T12:00:00Z")],
    );

    // Second commit by Bob modifying src/lib.rs and adding config.json
    fs::write(
        src_dir.join("lib.rs"),
        "// Initial library module\npub fn add(a: i32, b: i32) -> i32 {\n    a.saturating_add(b)\n}\n\npub fn multiply(a: i32, b: i32) -> i32 {\n    a * b\n}\n",
    )
    .unwrap();
    fs::write(repo_dir.join("config.json"), "{\n  \"name\": \"git-oxide\",\n  \"fast\": true\n}\n").unwrap();

    run_git(repo_dir, &["add", "."], &[]);
    run_git(
        repo_dir,
        &[
            "-c",
            "user.name=Bob Maintainer",
            "-c",
            "user.email=bob@example.com",
            "commit",
            "-m",
            "feat: use saturating_add and add multiply",
        ],
        &[("GIT_AUTHOR_DATE", "2026-10-08T15:30:00Z"), ("GIT_COMMITTER_DATE", "2026-10-08T15:30:00Z")],
    );

    // Create a secondary branch tag
    run_git(repo_dir, &["branch", "feature/analytics"], &[]);
}
