use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

#[test]
fn init_generates_repo_aware_config() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "trunk"]);
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

    fs::create_dir_all(repo_path.join("src/demo")).expect("create package dir");
    fs::write(
        repo_path.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.3.0\"\n",
    )
    .expect("write pyproject");
    fs::write(
        repo_path.join("setup.cfg"),
        "[metadata]\nname = demo\nversion = 0.3.0\n",
    )
    .expect("write setup.cfg");
    fs::write(
        repo_path.join("src/demo/__init__.py"),
        "__version__ = \"0.3.0\"\n",
    )
    .expect("write init");

    let init = Command::new(env!("CARGO_BIN_EXE_relx"))
        .arg("init")
        .current_dir(repo_path)
        .output()
        .expect("run relx init");
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let config = fs::read_to_string(repo_path.join("relx.toml")).expect("read config");
    assert!(config.contains("branch = \"trunk\""), "{config}");
    assert!(config.contains("initial_version = \"0.3.0\""), "{config}");
    assert!(config.contains("path = \"pyproject.toml\""), "{config}");
    assert!(config.contains("path = \"setup.cfg\""), "{config}");
    assert!(
        config.contains("path = \"src/demo/__init__.py\""),
        "{config}"
    );
    assert!(
        config.contains("pattern = '__version__ = \"{version}\"'"),
        "{config}"
    );
    assert!(config.contains("owner = \"acme\""), "{config}");
    assert!(config.contains("repo = \"demo\""), "{config}");

    let validate = Command::new(env!("CARGO_BIN_EXE_relx"))
        .arg("validate")
        .current_dir(repo_path)
        .output()
        .expect("run relx validate");
    assert!(
        validate.status.success(),
        "validate failed: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
}

#[test]
fn init_detects_rust_repos() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::write(
        repo_path.join("Cargo.toml"),
        "[package]\nname = \"demo-rust\"\nversion = \"0.4.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");

    let init = Command::new(env!("CARGO_BIN_EXE_relx"))
        .arg("init")
        .current_dir(repo_path)
        .output()
        .expect("run relx init");
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let config = fs::read_to_string(repo_path.join("relx.toml")).expect("read config");
    assert!(config.contains("ecosystem = \"rust\""), "{config}");
    assert!(config.contains("path = \"Cargo.toml\""), "{config}");
    assert!(config.contains("key = \"package.version\""), "{config}");
    assert!(config.contains("initial_version = \"0.4.0\""), "{config}");
}

#[test]
fn init_scaffolds_go_version_file_when_missing() {
    let repo_dir = tempdir().expect("tempdir");
    let repo_path = repo_dir.path();

    run(repo_path, &["git", "init", "-b", "main"]);
    run(repo_path, &["git", "config", "user.name", "Relx Test"]);
    run(
        repo_path,
        &["git", "config", "user.email", "relx@example.com"],
    );

    fs::write(
        repo_path.join("go.mod"),
        "module github.com/acme/demo-go\n\ngo 1.24.0\n",
    )
    .expect("write go.mod");

    let init = Command::new(env!("CARGO_BIN_EXE_relx"))
        .arg("init")
        .current_dir(repo_path)
        .output()
        .expect("run relx init");
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let config = fs::read_to_string(repo_path.join("relx.toml")).expect("read config");
    assert!(config.contains("ecosystem = \"go\""), "{config}");
    assert!(config.contains("path = \"VERSION\""), "{config}");
    assert!(config.contains("pattern = \"{version}\""), "{config}");

    let version_file = fs::read_to_string(repo_path.join("VERSION")).expect("read VERSION");
    assert_eq!(version_file, "0.1.0\n");
}

fn run(repo_path: &Path, args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(repo_path)
        .status()
        .expect("command should run");
    assert!(status.success(), "command failed: {args:?}");
}
