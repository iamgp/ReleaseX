use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn release_pr_dry_run_reports_github_mutations() {
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
            "https://github.com/acme/demo.git",
        ],
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
    run(
        repo_path,
        &["git", "-c", "tag.gpgSign=false", "tag", "v0.1.0"],
    );

    fs::write(repo_path.join("feature.txt"), "search support\n").expect("write feature");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "feat: add search support"],
    );

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
    assert!(
        stdout.contains("Would create or update PR `chore(release): v0.2.0` in acme/demo"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Would apply labels: autorelease: pending"),
        "{stdout}"
    );
}

#[test]
fn release_tag_dry_run_reports_tag_and_release() {
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
            "git@github.com:acme/demo.git",
        ],
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

[changelog.sections]
feat = "Added"
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(
        repo_path,
        &["git", "-c", "tag.gpgSign=false", "tag", "v0.1.0"],
    );

    fs::write(repo_path.join("feature.txt"), "search support\n").expect("write feature");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "feat: add search support"],
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
        stdout.contains("Would create and push tag `v0.2.0` to acme/demo"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Would create or update GitHub Release `v0.2.0`"),
        "{stdout}"
    );
}

#[test]
fn release_pr_dry_run_reports_monorepo_package_set() {
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
            "https://github.com/acme/demo.git",
        ],
    );

    fs::create_dir_all(repo_path.join("packages/core/src/core")).expect("create core");
    fs::create_dir_all(repo_path.join("packages/cli/src/cli")).expect("create cli");
    fs::write(
        repo_path.join("packages/core/pyproject.toml"),
        "[project]\nname = \"core\"\nversion = \"1.1.0\"\n",
    )
    .expect("write core pyproject");
    fs::write(
        repo_path.join("packages/core/src/core/__init__.py"),
        "__version__ = \"1.1.0\"\n",
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

[monorepo]
enabled = true
release_mode = "per_package"
packages = ["packages/core", "packages/cli"]

[github]
owner = "acme"
repo = "demo"
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(
        repo_path,
        &["git", "-c", "tag.gpgSign=false", "tag", "v0.1.0"],
    );

    fs::write(
        repo_path.join("packages/core/src/core/feature.py"),
        "print('feature')\n",
    )
    .expect("write feature");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "feat: add core feature"],
    );

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
        stdout.contains("Would create or update per_package release PR set covering: core 1.2.0"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Would create or update PR `chore(release): 1 packages package release set` in acme/demo"),
        "{stdout}"
    );
}

#[test]
fn release_pending_exits_zero_when_release_is_pending() {
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
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(
        repo_path,
        &["git", "-c", "tag.gpgSign=false", "tag", "v0.1.0"],
    );

    fs::write(repo_path.join("feature.txt"), "search support\n").expect("write feature");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "feat: add search support"],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "pending"])
        .current_dir(repo_path)
        .output()
        .expect("run relx release pending");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("release pending: 0.2.0"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn release_pending_exits_one_when_no_release_is_pending() {
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
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(
        repo_path,
        &["git", "-c", "tag.gpgSign=false", "tag", "v0.1.0"],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "pending"])
        .current_dir(repo_path)
        .output()
        .expect("run relx release pending");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no release pending"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn release_pending_json_reports_pending_state() {
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
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: initial release"],
    );
    run(
        repo_path,
        &["git", "-c", "tag.gpgSign=false", "tag", "v0.1.0"],
    );

    fs::write(repo_path.join("fix.txt"), "bugfix\n").expect("write fix");
    run(repo_path, &["git", "add", "."]);
    run(repo_path, &["git", "commit", "-m", "fix: patch it"]);

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "pending", "--json"])
        .current_dir(repo_path)
        .output()
        .expect("run relx release pending --json");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"pending\": true"), "{stdout}");
    assert!(stdout.contains("\"next_version\": \"0.1.1\""), "{stdout}");
}

fn run(repo_path: &std::path::Path, args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(repo_path)
        .status()
        .expect("command should run");
    assert!(status.success(), "command failed: {args:?}");
}
