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

#[test]
fn status_reports_cargo_workspace_with_cascade_bumps() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::create_dir_all(repo_path.join("crates/core/src")).expect("create core crate");
    fs::create_dir_all(repo_path.join("crates/cli/src")).expect("create cli crate");
    fs::write(
        repo_path.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .expect("write workspace Cargo.toml");
    fs::write(
        repo_path.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write core Cargo.toml");
    fs::write(
        repo_path.join("crates/cli/Cargo.toml"),
        "[package]\nname = \"cli\"\nversion = \"0.5.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    )
    .expect("write cli Cargo.toml");
    fs::write(
        repo_path.join("crates/core/src/lib.rs"),
        "pub fn core() {}\n",
    )
    .expect("write core lib");
    fs::write(repo_path.join("crates/cli/src/lib.rs"), "pub fn cli() {}\n").expect("write cli lib");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[project]
ecosystem = "rust"

[release]
branch = "main"
tag_prefix = "v"

[versioning]
initial_version = "0.1.0"

[monorepo]
enabled = true
release_mode = "per_package"

[workspace]
cascade_bumps = true
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
        repo_path.join("crates/core/src/lib.rs"),
        "pub fn core() {}\npub fn feature() {}\n",
    )
    .expect("update core lib");
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
    assert!(
        stdout.contains("Package discovery: cargo workspace (workspace.members)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("core [crates/core] current=0.1.0 next=0.2.0 bump=minor"),
        "{stdout}"
    );
    assert!(
        stdout.contains("cli [crates/cli] current=0.5.0 next=0.5.1 bump=patch"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reason=cascade bump: depends on a package with a version bump"),
        "{stdout}"
    );
}

#[test]
fn status_reports_go_workspace_with_cascade_bumps() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::create_dir_all(repo_path.join("services/api")).expect("create api module");
    fs::create_dir_all(repo_path.join("services/worker")).expect("create worker module");
    fs::write(
        repo_path.join("go.work"),
        "go 1.24.0\n\nuse (\n    ./services/api\n    ./services/worker\n)\n",
    )
    .expect("write go.work");
    fs::write(
        repo_path.join("services/api/go.mod"),
        "module github.com/acme/api\n\ngo 1.24.0\n",
    )
    .expect("write api go.mod");
    fs::write(
        repo_path.join("services/worker/go.mod"),
        "module github.com/acme/worker\n\ngo 1.24.0\n\nrequire github.com/acme/api v0.4.0\n",
    )
    .expect("write worker go.mod");
    fs::write(repo_path.join("services/api/VERSION"), "0.4.0\n").expect("write api version");
    fs::write(repo_path.join("services/worker/VERSION"), "1.1.0\n").expect("write worker version");
    fs::write(
        repo_path.join("services/api/main.go"),
        "package main\n\nfunc main() {}\n",
    )
    .expect("write api main");
    fs::write(
        repo_path.join("services/worker/main.go"),
        "package main\n\nfunc main() {}\n",
    )
    .expect("write worker main");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[project]
ecosystem = "go"

[release]
branch = "main"
tag_prefix = "v"

[versioning]
initial_version = "0.1.0"

[monorepo]
enabled = true
release_mode = "per_package"

[workspace]
cascade_bumps = true
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
        repo_path.join("services/api/main.go"),
        "package main\n\nfunc main() {}\n\nfunc feature() {}\n",
    )
    .expect("update api main");
    run(repo_path, &["git", "add", "."]);
    run(repo_path, &["git", "commit", "-m", "feat: add api feature"]);

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
    assert!(
        stdout.contains("Package discovery: go workspace (go.work use)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("api [services/api] current=0.4.0 next=0.5.0 bump=minor"),
        "{stdout}"
    );
    assert!(
        stdout.contains("worker [services/worker] current=1.1.0 next=1.1.1 bump=patch"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reason=cascade bump: depends on a package with a version bump"),
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
