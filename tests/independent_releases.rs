use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::tempdir;

fn write_python_package(root: &Path, rel: &str, name: &str, version: &str, extra_toml: &str) {
    let dir = if rel == "." {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    fs::create_dir_all(dir.join("src")).expect("package dir");
    let mut body = format!("[project]\nname = \"{name}\"\nversion = \"{version}\"\n");
    if !extra_toml.is_empty() {
        body.push('\n');
        body.push_str(extra_toml);
        body.push('\n');
    }
    fs::write(dir.join("pyproject.toml"), body).expect("pyproject");
    fs::write(dir.join("src/mod.py"), format!("# {name} {version}\n")).expect("source");
}

fn write_relx(root: &Path, mode: &str, extra: &str) {
    fs::write(
        root.join("relx.toml"),
        format!(
            r#"[project]
ecosystem = "python"

[release]
branch = "main"
tag_prefix = "v"

[changelog]
contributors = false

[monorepo]
enabled = true
release_mode = "{mode}"
packages = [".", "packages/a", "packages/b"]

[github]
owner = "acme"
repo = "demo"
{extra}
"#
        ),
    )
    .expect("relx.toml");
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "tag.gpgsign")
        .env("GIT_CONFIG_VALUE_1", "false")
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(repo: &Path) {
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.name", "Relx Test"]);
    git(repo, &["config", "user.email", "relx@example.com"]);
}

fn commit_all(repo: &Path, message: &str) {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message]);
}

fn relx(repo: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_relx"));
    command.args(args).current_dir(repo);
    command
}

fn relx_ok(repo: &Path, args: &[&str]) -> String {
    let output = relx(repo, args).output().expect("relx");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "relx {args:?} failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
}

fn relx_err(repo: &Path, args: &[&str]) -> String {
    let output = relx(repo, args).output().expect("relx");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "relx {args:?} unexpectedly succeeded\nstdout:\n{stdout}"
    );
    format!("{stdout}{stderr}")
}

fn read_version(repo: &Path, rel: &str) -> String {
    let path = if rel == "." {
        repo.join("pyproject.toml")
    } else {
        repo.join(rel).join("pyproject.toml")
    };
    let raw = fs::read_to_string(path).expect("pyproject");
    raw.lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|line| line.strip_suffix('"'))
        .expect("version")
        .to_string()
}

fn seed_independent_workspace(repo: &Path, mode: &str) {
    write_python_package(repo, ".", "rootpkg", "1.0.0", "");
    write_python_package(repo, "packages/a", "package-a", "1.0.0", "");
    write_python_package(repo, "packages/b", "package-b", "2.0.0", "");
    write_relx(repo, mode, "");
    init_repo(repo);
    commit_all(repo, "chore: initial workspace");
    git(repo, &["tag", "package-a/v1.0.0"]);
    git(repo, &["tag", "package-b/v2.0.0"]);
    git(repo, &["tag", "rootpkg/v1.0.0"]);
}

#[test]
fn later_package_release_does_not_hide_earlier_unreleased_changes() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    seed_independent_workspace(repo, "release_set");

    fs::write(repo.join("packages/b/src/mod.py"), "# fix b\n").unwrap();
    commit_all(repo, "fix(B): change B");
    fs::write(repo.join("packages/a/src/mod.py"), "# fix a\n").unwrap();
    commit_all(repo, "fix(A): change A");

    relx_ok(repo, &["release", "prepare", "--package", "package-a"]);
    assert_eq!(read_version(repo, "packages/a"), "1.0.1");
    assert_eq!(read_version(repo, "packages/b"), "2.0.0");
    commit_all(repo, "chore(release): package-a 1.0.1");
    git(repo, &["tag", "package-a/v1.0.1"]);

    let plan = relx_ok(
        repo,
        &["release", "plan", "--json", "--package", "package-b"],
    );
    let parsed: Value = serde_json::from_str(&plan).expect("json");
    let packages = parsed["packages"].as_array().expect("packages");
    let b = packages
        .iter()
        .find(|package| package["name"] == "package-b")
        .expect("package-b");
    assert_eq!(b["selected"], true);
    assert_eq!(b["next_version"], "2.0.1");
    assert_eq!(b["release_tag"], "package-b/v2.0.1");
    let changes = b["changes"].as_array().expect("changes");
    assert!(
        changes.iter().any(|change| change["message"]
            .as_str()
            .unwrap_or_default()
            .contains("fix(B): change B")),
        "{changes:?}"
    );
}

#[test]
fn second_release_of_same_package_starts_from_its_own_tag() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    seed_independent_workspace(repo, "per_package");

    fs::write(repo.join("packages/a/src/mod.py"), "# first\n").unwrap();
    commit_all(repo, "feat: improve a");
    relx_ok(repo, &["release", "prepare", "--package", "package-a"]);
    assert_eq!(read_version(repo, "packages/a"), "1.1.0");
    commit_all(repo, "chore(release): package-a 1.1.0");
    git(repo, &["tag", "package-a/v1.1.0"]);

    fs::write(repo.join("packages/a/src/mod.py"), "# second\n").unwrap();
    commit_all(repo, "fix: follow-up a");
    let plan = relx_ok(
        repo,
        &["release", "plan", "--json", "--package", "package-a"],
    );
    let parsed: Value = serde_json::from_str(&plan).expect("json");
    let a = parsed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "package-a")
        .unwrap();
    assert_eq!(a["current_version"], "1.1.0");
    assert_eq!(a["next_version"], "1.1.1");
    let messages: Vec<_> = a["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("fix: follow-up a"))
    );
    assert!(
        messages
            .iter()
            .all(|message| !message.contains("feat: improve a"))
    );
}

#[test]
fn releasing_one_package_leaves_pending_package_untouched_in_both_modes() {
    for mode in ["release_set", "per_package"] {
        let dir = tempdir().expect("tempdir");
        let repo = dir.path();
        seed_independent_workspace(repo, mode);
        fs::write(repo.join("packages/b/src/mod.py"), "# pending b\n").unwrap();
        commit_all(repo, "feat: pending b");
        fs::write(repo.join("packages/a/src/mod.py"), "# release a\n").unwrap();
        commit_all(repo, "fix: release a");

        relx_ok(repo, &["release", "prepare", "--package", "package-a"]);
        assert_eq!(read_version(repo, "packages/a"), "1.0.1");
        assert_eq!(read_version(repo, "packages/b"), "2.0.0");
        let b_source = fs::read_to_string(repo.join("packages/b/src/mod.py")).unwrap();
        assert_eq!(b_source, "# pending b\n");
        let status = Command::new("git")
            .args(["status", "--short"])
            .current_dir(repo)
            .output()
            .expect("status");
        let diff = String::from_utf8_lossy(&status.stdout);
        assert!(
            !diff.contains("packages/b/"),
            "mode {mode} changed package-b files: {diff}"
        );
    }
}

#[test]
fn compatible_root_patch_does_not_rewrite_provider_range() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    write_python_package(repo, ".", "core", "1.0.0", "");
    write_python_package(
        repo,
        "packages/provider",
        "provider",
        "3.0.0",
        "dependencies = [\"core>=1.0,<2.0\"]",
    );
    fs::write(
        repo.join("relx.toml"),
        r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/provider"]
[workspace]
cascade_bumps = true
[github]
owner = "acme"
repo = "demo"
"#,
    )
    .unwrap();
    init_repo(repo);
    commit_all(repo, "chore: seed");
    git(repo, &["tag", "core/v1.0.0"]);
    git(repo, &["tag", "provider/v3.0.0"]);
    fs::write(repo.join("src/mod.py"), "# core patch\n").unwrap();
    commit_all(repo, "fix: core compatible patch");

    relx_ok(repo, &["release", "prepare", "--package", "core"]);
    assert_eq!(read_version(repo, "."), "1.0.1");
    assert_eq!(read_version(repo, "packages/provider"), "3.0.0");
    let provider = fs::read_to_string(repo.join("packages/provider/pyproject.toml")).unwrap();
    assert!(provider.contains("core>=1.0,<2.0"), "{provider}");
}

#[test]
fn provider_only_release_ignores_earlier_root_tag() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    write_python_package(repo, ".", "core", "1.0.0", "");
    write_python_package(repo, "packages/provider", "provider", "3.0.0", "");
    fs::write(
        repo.join("relx.toml"),
        r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/provider"]
[github]
owner = "acme"
repo = "demo"
"#,
    )
    .unwrap();
    init_repo(repo);
    commit_all(repo, "chore: seed");
    git(repo, &["tag", "core/v1.0.0"]);
    git(repo, &["tag", "provider/v3.0.0"]);
    fs::write(
        repo.join("packages/provider/src/mod.py"),
        "# provider feat\n",
    )
    .unwrap();
    commit_all(repo, "feat: provider change");
    fs::write(repo.join("src/mod.py"), "# later core\n").unwrap();
    commit_all(repo, "fix: core later");
    relx_ok(repo, &["release", "prepare", "--package", "core"]);
    commit_all(repo, "chore(release): core 1.0.1");
    git(repo, &["tag", "core/v1.0.1"]);

    let plan = relx_ok(
        repo,
        &["release", "plan", "--json", "--package", "provider"],
    );
    let parsed: Value = serde_json::from_str(&plan).unwrap();
    let provider = parsed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "provider")
        .unwrap();
    assert_eq!(provider["next_version"], "3.1.0");
    let root = parsed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "core")
        .unwrap();
    assert_eq!(root["selected"], false);
    assert_eq!(root["current_version"], "1.0.1");
}

#[test]
fn incompatible_dependency_is_visible_and_exclusion_fails() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    write_python_package(repo, ".", "core", "1.0.0", "");
    write_python_package(
        repo,
        "packages/provider",
        "provider",
        "3.0.0",
        "dependencies = [\"core>=1.0,<1.1\"]",
    );
    fs::write(
        repo.join("relx.toml"),
        r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/provider"]
[workspace]
cascade_bumps = false
[github]
owner = "acme"
repo = "demo"
"#,
    )
    .unwrap();
    init_repo(repo);
    commit_all(repo, "chore: seed");
    git(repo, &["tag", "core/v1.0.0"]);
    git(repo, &["tag", "provider/v3.0.0"]);
    fs::write(repo.join("src/mod.py"), "# breaking range\n").unwrap();
    commit_all(repo, "feat: core minor");

    let err = relx_err(repo, &["release", "plan", "--json", "--package", "core"]);
    assert!(
        err.contains("incompatible workspace dependency") && err.contains("provider"),
        "{err}"
    );

    fs::write(
        repo.join("relx.toml"),
        r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/provider"]
[workspace]
cascade_bumps = true
[github]
owner = "acme"
repo = "demo"
"#,
    )
    .unwrap();
    let plan = relx_ok(repo, &["release", "plan", "--json"]);
    let parsed: Value = serde_json::from_str(&plan).unwrap();
    assert!(
        !parsed["required_dependency_changes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let provider = parsed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "provider")
        .unwrap();
    assert_eq!(provider["selected"], true);
}

#[test]
fn prepare_writes_manifest_and_only_selected_package_diff() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    seed_independent_workspace(repo, "release_set");
    fs::write(repo.join("packages/b/src/mod.py"), "# pending\n").unwrap();
    commit_all(repo, "fix: pending b");
    fs::write(repo.join("packages/a/src/mod.py"), "# selected\n").unwrap();
    commit_all(repo, "fix: selected a");

    relx_ok(repo, &["release", "prepare", "--package", "package-a"]);
    let manifest_raw =
        fs::read_to_string(repo.join(".relx/release-manifest.json")).expect("manifest");
    let manifest: Value = serde_json::from_str(&manifest_raw).unwrap();
    assert_eq!(manifest["schema_version"], 2);
    let selected: Vec<_> = manifest["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|package| package["selected"] == true)
        .collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0]["name"], "package-a");
    assert_eq!(selected[0]["path"], "packages/a");
    assert_eq!(selected[0]["current_version"], "1.0.0");
    assert_eq!(selected[0]["next_version"], "1.0.1");
    assert_eq!(selected[0]["release_tag"], "package-a/v1.0.1");
    assert_eq!(selected[0]["baseline"]["reference"], "package-a/v1.0.0");
    assert!(
        manifest["preparation_base"]["commit"]
            .as_str()
            .unwrap()
            .len()
            >= 7
    );

    let status = Command::new("git")
        .args(["status", "--short"])
        .current_dir(repo)
        .output()
        .unwrap();
    let diff = String::from_utf8_lossy(&status.stdout);
    assert!(diff.contains("packages/a/"), "{diff}");
    assert!(
        diff.contains(".relx") || diff.contains("release-manifest.json"),
        "{diff}"
    );
    assert!(!diff.contains("packages/b/"), "{diff}");
}

#[test]
fn squash_validation_accepts_matching_tree_and_rejects_stale_plan() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    seed_independent_workspace(repo, "release_set");
    fs::write(repo.join("packages/a/src/mod.py"), "# selected\n").unwrap();
    commit_all(repo, "fix: selected a");
    relx_ok(repo, &["release", "prepare", "--package", "package-a"]);
    git(repo, &["add", "-A"]);
    git(
        repo,
        &[
            "commit",
            "-m",
            "squash: unrelated wording for the merge queue",
        ],
    );
    relx_ok(repo, &["release", "verify-plan"]);

    let mut manifest: Value = serde_json::from_str(
        &fs::read_to_string(repo.join(".relx/release-manifest.json")).unwrap(),
    )
    .unwrap();
    manifest["source_digest"] = Value::String("deadbeef".into());
    fs::write(
        repo.join(".relx/release-manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let err = relx_err(repo, &["release", "verify-plan"]);
    assert!(
        err.contains("stale") || err.contains("does not match"),
        "{err}"
    );
}

#[test]
fn legacy_migration_new_packages_and_invalid_baselines() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    write_python_package(repo, ".", "legacy-root", "0.15.1", "");
    write_python_package(repo, "packages/a", "legacy-a", "0.15.1", "");
    fs::write(
        repo.join("relx.toml"),
        r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/a", "packages/new"]
first_release_packages = ["brand-new"]
[[monorepo.legacy_releases]]
tag = "v0.15.1"
packages = ["legacy-root", "legacy-a"]
[github]
owner = "acme"
repo = "demo"
"#,
    )
    .unwrap();
    init_repo(repo);
    commit_all(repo, "chore: shared release");
    git(repo, &["tag", "v0.15.1"]);
    let shared = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    fs::write(
        repo.join("relx.toml"),
        format!(
            r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/a"]
[[monorepo.legacy_releases]]
tag = "v0.15.1"
commit = "{shared}"
packages = ["legacy-root", "legacy-a"]
[github]
owner = "acme"
repo = "demo"
"#
        ),
    )
    .unwrap();
    commit_all(repo, "chore: record migration config");
    fs::write(repo.join("packages/a/src/mod.py"), "# after shared\n").unwrap();
    commit_all(repo, "fix: a after shared tag");
    let plan = relx_ok(
        repo,
        &["release", "plan", "--json", "--package", "legacy-a"],
    );
    let parsed: Value = serde_json::from_str(&plan).unwrap();
    let a = parsed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "legacy-a")
        .unwrap();
    assert_eq!(a["baseline"]["kind"], "legacy_tag");
    assert_eq!(a["next_version"], "0.15.2");

    write_python_package(repo, "packages/new", "brand-new", "0.1.0", "");
    fs::write(
        repo.join("relx.toml"),
        format!(
            r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/a", "packages/new"]
first_release_packages = ["brand-new"]
[[monorepo.legacy_releases]]
tag = "v0.15.1"
commit = "{shared}"
packages = ["legacy-root", "legacy-a"]
[github]
owner = "acme"
repo = "demo"
"#
        ),
    )
    .unwrap();
    commit_all(repo, "feat: add brand-new package");
    let new_plan = relx_ok(
        repo,
        &["release", "plan", "--json", "--package", "brand-new"],
    );
    let parsed: Value = serde_json::from_str(&new_plan).unwrap();
    let new_pkg = parsed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "brand-new")
        .unwrap();
    assert_eq!(new_pkg["baseline"]["kind"], "first_release");

    fs::write(repo.join("unrelated.txt"), "side history\n").unwrap();
    commit_all(repo, "chore: unrelated commit");
    let unrelated = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(repo, &["reset", "--hard", "HEAD^"]);
    git(repo, &["tag", "legacy-a/v9.9.9", &unrelated]);
    let err = relx_err(
        repo,
        &["release", "plan", "--json", "--package", "legacy-a"],
    );
    assert!(
        err.contains("not an ancestor") || err.contains("no valid release baseline"),
        "{err}"
    );
}

#[test]
fn prerelease_tag_does_not_consume_stable_history() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    seed_independent_workspace(repo, "release_set");
    fs::write(repo.join("packages/a/src/mod.py"), "# beta\n").unwrap();
    commit_all(repo, "feat: a beta work");
    git(repo, &["tag", "package-a/v1.1.0b1"]);
    fs::write(repo.join("packages/a/src/mod.py"), "# stable\n").unwrap();
    commit_all(repo, "fix: a stable follow-up");
    let plan = relx_ok(
        repo,
        &["release", "plan", "--json", "--package", "package-a"],
    );
    let parsed: Value = serde_json::from_str(&plan).unwrap();
    let a = parsed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "package-a")
        .unwrap();
    assert_eq!(a["baseline"]["reference"], "package-a/v1.0.0");
    assert_eq!(a["next_version"], "1.1.0");
}

#[test]
fn tagging_is_idempotent_for_unchanged_content() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    seed_independent_workspace(repo, "release_set");
    fs::write(repo.join("packages/a/src/mod.py"), "# change\n").unwrap();
    commit_all(repo, "fix: a");
    relx_ok(repo, &["release", "prepare", "--package", "package-a"]);
    commit_all(repo, "chore(release): package-a 1.0.1");
    git(repo, &["tag", "package-a/v1.0.1"]);
    let err = relx_err(
        repo,
        &["release", "plan", "--json", "--package", "package-a"],
    );
    assert!(
        err.contains("no releasable package set is pending"),
        "{err}"
    );
}

#[test]
fn malformed_legacy_commit_is_rejected() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path();
    write_python_package(repo, "packages/a", "legacy-a", "0.1.0", "");
    fs::write(
        repo.join("relx.toml"),
        r#"[project]
ecosystem = "python"
[changelog]
contributors = false
[monorepo]
enabled = true
release_mode = "per_package"
packages = ["packages/a"]
[[monorepo.legacy_releases]]
tag = "v0.1.0"
commit = "ffffffffffffffffffffffffffffffffffffffff"
packages = ["legacy-a"]
[github]
owner = "acme"
repo = "demo"
"#,
    )
    .unwrap();
    init_repo(repo);
    commit_all(repo, "chore: seed");
    git(repo, &["tag", "v0.1.0"]);
    let err = relx_err(repo, &["release", "plan", "--json"]);
    assert!(
        err.contains("legacy baseline") || err.contains("configured commit"),
        "{err}"
    );
}
