use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn release_publish_dry_run_reports_uv_provider_and_target() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::create_dir_all(repo_path.join("dist")).expect("create dist");
    fs::write(
        repo_path.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.2.0\"\n",
    )
    .expect("write pyproject");
    fs::write(repo_path.join("dist/demo-0.2.0.tar.gz"), "sdist").expect("write sdist");
    fs::write(repo_path.join("dist/demo-0.2.0-py3-none-any.whl"), "wheel").expect("write wheel");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[release]
branch = "main"
tag_prefix = "v"

[[version_files]]
path = "pyproject.toml"
key = "project.version"

[publish]
enabled = true
provider = "uv"
repository = "testpypi"
repository_url = "https://test.pypi.org/legacy/"
trusted_publishing = true
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: prepare publish"],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "publish", "--dry-run"])
        .current_dir(repo_path)
        .output()
        .expect("run relx release publish");

    assert!(
        output.status.success(),
        "release publish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Provider: uv"), "{stdout}");
    assert!(
        stdout.contains("Target repository: testpypi (https://test.pypi.org/legacy/)"),
        "{stdout}"
    );
    assert!(stdout.contains("Trusted publishing: enabled"), "{stdout}");
    assert!(stdout.contains("Command: uv publish"), "{stdout}");
}

#[test]
fn release_publish_dry_run_reports_twine_command_and_env() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::create_dir_all(repo_path.join("dist")).expect("create dist");
    fs::write(
        repo_path.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.2.0\"\n",
    )
    .expect("write pyproject");
    fs::write(repo_path.join("dist/demo-0.2.0.tar.gz"), "sdist").expect("write sdist");
    fs::write(
        repo_path.join("relx.toml"),
        r#"[release]
branch = "main"
tag_prefix = "v"

[[version_files]]
path = "pyproject.toml"
key = "project.version"

[publish]
enabled = true
provider = "twine"
repository = "internal"
repository_url = "https://packages.example.com/upload"
token_env = "PYPI_TOKEN"
"#,
    )
    .expect("write config");
    run(repo_path, &["git", "add", "."]);
    run(
        repo_path,
        &["git", "commit", "-m", "chore: prepare publish"],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .args(["release", "publish", "--dry-run"])
        .env("PYPI_TOKEN", "secret")
        .current_dir(repo_path)
        .output()
        .expect("run relx release publish");

    assert!(
        output.status.success(),
        "release publish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Provider: twine"), "{stdout}");
    assert!(
        stdout.contains("Target repository: internal (https://packages.example.com/upload)"),
        "{stdout}"
    );
    assert!(stdout.contains("Environment:"), "{stdout}");
    assert!(stdout.contains("TWINE_TOKEN=<set>"), "{stdout}");
    assert!(
        stdout.contains("Command: twine upload --non-interactive --repository-url https://packages.example.com/upload"),
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
