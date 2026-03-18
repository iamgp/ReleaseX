use std::{fs, process::Command};

use tempfile::tempdir;

fn run(repo_path: &std::path::Path, args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(repo_path)
        .status()
        .expect("command should run");
    assert!(status.success(), "command failed: {args:?}");
}

#[test]
fn status_no_releasable_commits_shows_no_bump() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::write(
        repo_path.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .expect("write pyproject");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[release]
branch = "main"
tag_prefix = "v"

[versioning]
strategy = "conventional_commits"

[[version_files]]
path = "pyproject.toml"
key = "project.version"
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(repo_path, &["git", "tag", "v0.1.0"]);

    fs::write(repo_path.join("readme.md"), "updated readme\n").expect("write readme");
    run(repo_path, &["git", "add", "."]);
    run(repo_path, &["git", "commit", "-m", "updated readme"]);

    fs::write(repo_path.join("notes.txt"), "misc cleanup\n").expect("write notes");
    run(repo_path, &["git", "add", "."]);
    run(repo_path, &["git", "commit", "-m", "misc cleanup"]);

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["status", "--dry-run"])
        .current_dir(repo_path)
        .output()
        .expect("run relx status");

    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Current version: 0.1.0"), "{stdout}");
    assert!(stdout.contains("Proposed bump: none"), "{stdout}");
    assert!(stdout.contains("Next version: unchanged"), "{stdout}");
}

#[test]
fn status_breaking_change_reports_major_bump() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::write(
        repo_path.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"1.0.0\"\n",
    )
    .expect("write pyproject");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[release]
branch = "main"
tag_prefix = "v"

[versioning]
strategy = "conventional_commits"

[[version_files]]
path = "pyproject.toml"
key = "project.version"
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(repo_path, &["git", "tag", "v1.0.0"]);

    fs::write(repo_path.join("api.txt"), "redesigned API\n").expect("write api");
    run(repo_path, &["git", "add", "."]);
    run(repo_path, &["git", "commit", "-m", "feat!: redesign API"]);

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["status", "--dry-run"])
        .current_dir(repo_path)
        .output()
        .expect("run relx status");

    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Current version: 1.0.0"), "{stdout}");
    assert!(stdout.contains("Proposed bump: major"), "{stdout}");
    assert!(stdout.contains("Next version: 2.0.0"), "{stdout}");
}

#[test]
fn release_pr_dry_run_with_rust_ecosystem() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );
    run(
        repo_path,
        &[
            "git",
            "remote",
            "add",
            "origin",
            "https://github.com/acme/demo-rust.git",
        ],
    );

    fs::write(
        repo_path.join("Cargo.toml"),
        "[package]\nname = \"demo-rust\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[project]
ecosystem = "rust"

[release]
branch = "main"
tag_prefix = "v"

[[version_files]]
path = "Cargo.toml"
key = "package.version"

[changelog.sections]
feat = "Added"
fix = "Fixed"
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(repo_path, &["git", "tag", "v0.1.0"]);

    fs::write(repo_path.join("feature.rs"), "fn feature() {}\n").expect("write feature");
    run(repo_path, &["git", "add", "."]);
    run(repo_path, &["git", "commit", "-m", "feat: add new feature"]);

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "pr", "--dry-run"])
        .current_dir(repo_path)
        .output()
        .expect("run relx release pr");

    assert!(
        output.status.success(),
        "release pr failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Would push release branch `relx/release/v0.2.0`"),
        "{stdout}"
    );
    assert!(stdout.contains("Would create or update PR"), "{stdout}");
}

#[test]
fn release_tag_dry_run_with_go_ecosystem() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );
    run(
        repo_path,
        &[
            "git",
            "remote",
            "add",
            "origin",
            "git@github.com:acme/demo-go.git",
        ],
    );

    fs::write(
        repo_path.join("go.mod"),
        "module github.com/acme/demo-go\n\ngo 1.24.0\n",
    )
    .expect("write go.mod");
    fs::write(repo_path.join("VERSION"), "0.1.0\n").expect("write VERSION");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[project]
ecosystem = "go"

[release]
branch = "main"
tag_prefix = "v"

[[version_files]]
path = "VERSION"
pattern = "{version}"

[changelog.sections]
feat = "Added"
fix = "Fixed"
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(repo_path, &["git", "tag", "v0.1.0"]);

    fs::write(repo_path.join("bugfix.go"), "package main\n").expect("write bugfix");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "fix: resolve edge case"],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "tag", "--dry-run"])
        .current_dir(repo_path)
        .output()
        .expect("run relx release tag");

    assert!(
        output.status.success(),
        "release tag failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Would create and push tag `v0.1.1` to acme/demo-go"),
        "{stdout}"
    );
}

#[test]
fn init_refuses_to_overwrite_existing_config() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::write(
        repo_path.join("relx.toml"),
        "[release]\nbranch = \"main\"\n",
    )
    .expect("write existing config");

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .arg("init")
        .current_dir(repo_path)
        .output()
        .expect("run relx init");

    assert!(
        !output.status.success(),
        "init should fail when config already exists"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config already exists"),
        "expected 'config already exists' error, got: {stderr}"
    );
}

#[test]
fn validate_rejects_missing_version_files() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::write(
        repo_path.join("relx.toml"),
        r#"[release]
branch = "main"
tag_prefix = "v"
"#,
    )
    .expect("write config");

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .arg("validate")
        .current_dir(repo_path)
        .output()
        .expect("run relx validate");

    assert!(
        !output.status.success(),
        "validate should fail when no version_files are defined"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("version_files"),
        "expected version_files error, got: {stderr}"
    );
}

#[test]
fn publish_disabled_by_default_rejects_publish_command() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::write(
        repo_path.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .expect("write pyproject");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[release]
branch = "main"
tag_prefix = "v"

[[version_files]]
path = "pyproject.toml"
key = "project.version"

[publish]
enabled = false
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(repo_path, &["git", "commit", "-m", "chore: initial commit"]);

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "publish"])
        .current_dir(repo_path)
        .output()
        .expect("run relx release publish");

    assert!(
        !output.status.success(),
        "publish should fail when disabled"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("publish flow is disabled"),
        "expected 'publish flow is disabled' error, got: {stderr}"
    );
}
