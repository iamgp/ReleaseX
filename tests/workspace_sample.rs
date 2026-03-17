use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

#[test]
fn workspace_reports_cargo_members() {
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
        "[package]\nname = \"core\"\nversion = \"1.2.3\"\nedition = \"2024\"\n",
    )
    .expect("write core Cargo.toml");
    fs::write(
        repo_path.join("crates/cli/Cargo.toml"),
        "[package]\nname = \"cli\"\nversion = \"1.2.3\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
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
        "[project]\necosystem = \"rust\"\n[release]\nbranch = \"main\"\ntag_prefix = \"v\"\n[[version_files]]\npath = \"Cargo.toml\"\nkey = \"package.version\"\n",
    )
    .expect("write config");

    let output = Command::new(env!("CARGO_BIN_EXE_relx"))
        .arg("workspace")
        .current_dir(repo_path)
        .output()
        .expect("run relx workspace");

    assert!(
        output.status.success(),
        "workspace failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Workspace root: Cargo.toml"), "{stdout}");
    assert!(
        stdout.contains("Discovery: cargo workspace (workspace.members)"),
        "{stdout}"
    );
    assert!(stdout.contains("crates/core (core 1.2.3)"), "{stdout}");
    assert!(stdout.contains("crates/cli (cli 1.2.3)"), "{stdout}");
    assert!(stdout.contains("depends on core"), "{stdout}");
}

fn run(repo_path: &Path, args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(repo_path)
        .status()
        .expect("command should run");
    assert!(status.success(), "command failed: {args:?}");
}
