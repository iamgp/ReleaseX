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

[[release.replacements]]
files = ["registry/support/*.json"]
search = '"name": "demo", "version": "0.1.0"'
replace = '"name": "demo", "version": "0.2.0"'
expected_matches = 1

[changelog.sections]
feat = "Added"
fix = "Fixed"
"#,
    )
    .expect("write config");
    fs::create_dir_all(repo_path.join("registry/support")).expect("create registry");
    fs::write(
        repo_path.join("registry/support/v1.json"),
        "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
    )
    .expect("write registry");
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
    assert!(
        stdout.contains(
            "Would replace 1 literal match(es) for package `demo` in `registry/support/v1.json`"
        ),
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
    run(repo_path, &["git", "tag", "v0.1.0"]);

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
    run(repo_path, &["git", "tag", "v0.1.0"]);

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

fn run(repo_path: &std::path::Path, args: &[&str]) {
    let mut command_args = args[1..].to_vec();
    if args.first() == Some(&"git") && args.get(1) == Some(&"tag") {
        command_args = vec!["-c", "tag.gpgSign=false", "tag"];
        command_args.extend_from_slice(&args[2..]);
    }

    let status = Command::new(args[0])
        .args(command_args)
        .current_dir(repo_path)
        .status()
        .expect("command should run");
    assert!(status.success(), "command failed: {args:?}");
}
