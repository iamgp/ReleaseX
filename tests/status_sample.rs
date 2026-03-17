use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn status_dry_run_reports_bump_for_sample_repo() {
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
initial_version = "0.1.0"

[[version_files]]
path = "pyproject.toml"
key = "project.version"

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

    fs::write(repo_path.join("feature.txt"), "search support\n").expect("write feature");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "feat: add search support"],
    );

    fs::write(repo_path.join("fix.txt"), "trim parser\n").expect("write fix");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "fix: trim whitespace in parser"],
    );

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
    assert!(stdout.contains("Proposed bump: minor"), "{stdout}");
    assert!(stdout.contains("Next version: 0.2.0"), "{stdout}");
    assert!(stdout.contains("Added:"), "{stdout}");
    assert!(stdout.contains("Fixed:"), "{stdout}");
    assert!(stdout.contains("Dry run: no files changed"), "{stdout}");
}

#[test]
fn status_reports_monorepo_selected_package_set() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::create_dir_all(repo_path.join("packages/core/src/core")).expect("create core");
    fs::create_dir_all(repo_path.join("packages/cli/src/cli")).expect("create cli");
    fs::write(
        repo_path.join("packages/core/pyproject.toml"),
        "[project]\nname = \"core\"\nversion = \"0.1.0\"\n",
    )
    .expect("write core pyproject");
    fs::write(
        repo_path.join("packages/core/src/core/__init__.py"),
        "__version__ = \"0.1.0\"\n",
    )
    .expect("write core init");
    fs::write(
        repo_path.join("packages/cli/pyproject.toml"),
        "[project]\nname = \"cli\"\nversion = \"0.5.0\"\n",
    )
    .expect("write cli pyproject");
    fs::write(
        repo_path.join("packages/cli/src/cli/__init__.py"),
        "__version__ = \"0.5.0\"\n",
    )
    .expect("write cli init");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[release]
branch = "main"
tag_prefix = "v"

[versioning]
initial_version = "0.1.0"

[monorepo]
enabled = true
release_mode = "unified"
"#,
    )
    .expect("write config");

    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(repo_path, &["git", "tag", "v0.1.0"]);

    fs::write(
        repo_path.join("packages/core/src/core/feature.py"),
        "print('feature')\n",
    )
    .expect("write core feature");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "feat: add core feature"],
    );

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
    assert!(stdout.contains("Release mode: unified"), "{stdout}");
    assert!(
        stdout.contains("Package discovery: auto-discovered package pyproject.toml files"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Selected package set: 1 package(s)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("core [packages/core] current=0.1.0 next=0.2.0 bump=minor"),
        "{stdout}"
    );
    assert!(
        stdout.contains("cli [packages/cli] current=0.5.0 next=unchanged bump=none"),
        "{stdout}"
    );
    assert!(
        stdout.contains("changed files: packages/core/src/core/feature.py"),
        "{stdout}"
    );
}

fn run(repo_path: &std::path::Path, args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(repo_path)
        .status()
        .expect("command should run");
    assert!(status.success(), "command failed: {args:?}");
}
