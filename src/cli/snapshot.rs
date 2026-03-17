use std::{fs, path::Path, process::Command, time::SystemTime};

use anyhow::{Context, Result};
use console::style;
use openssl::sha::sha256;

use crate::{
    analysis, changelog, channels, cli::Cli, config::Config, git::GitRepository, github, progress,
};

enum BuildResult {
    Passed,
    Failed,
    Skipped,
}

enum ValidationResult {
    Passed,
    Failed,
    Skipped,
}

struct ArtifactInfo {
    name: String,
    size_bytes: u64,
    sha256: String,
}

pub fn run(cli: &Cli) -> Result<()> {
    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config = Config::load(&cli.config_path())?;

    let sp = progress::spinner("Analyzing commits…");
    let analysis = analysis::analyze(&repo, &config);
    sp.finish_and_clear();
    let mut analysis = analysis?;
    let current_branch = repo.current_branch()?;
    channels::apply_channel_to_analysis(&repo, &config, &mut analysis, &current_branch, None)?;

    let next_version = analysis
        .next_version
        .as_ref()
        .context("nothing to release — no version bump detected")?;
    let snapshot_version = format!("{}.dev1+snapshot", next_version.base());

    let snapshot_dir = Path::new(".relx/snapshot");
    fs::create_dir_all(snapshot_dir).context("failed to create .relx/snapshot")?;

    // Generate changelog entry
    let emoji = &config.changelog.first_contribution_emoji;
    let release_notes =
        changelog::render_release_notes(&snapshot_version, "snapshot", &analysis.changelog, emoji);
    fs::write(snapshot_dir.join("CHANGELOG_ENTRY.md"), &release_notes)
        .context("failed to write CHANGELOG_ENTRY.md")?;

    // Generate release PR body
    let pr_plan = github::build_release_pr_plan(&config, &analysis, &current_branch)?;
    fs::write(snapshot_dir.join("RELEASE_PR_BODY.md"), &pr_plan.body)
        .context("failed to write RELEASE_PR_BODY.md")?;

    // Try to build
    let dist_dir = snapshot_dir.join("dist");
    fs::create_dir_all(&dist_dir)?;
    let build_result = try_build(&dist_dir);
    let validation_result = try_validate(&dist_dir);

    // Collect dist artifacts
    let artifacts = collect_artifacts(&dist_dir);

    // Write manifest
    let manifest = build_manifest(
        &snapshot_version,
        &artifacts,
        &build_result,
        &validation_result,
        &release_notes,
        &pr_plan.body,
    );
    fs::write(
        snapshot_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )
    .context("failed to write manifest.json")?;

    // Print summary
    println!();
    println!("{}", style("Snapshot complete").green().bold());
    println!();
    println!(" Version: {}", style(&snapshot_version).cyan());
    println!(" Output:  .relx/snapshot/");
    println!("   CHANGELOG_ENTRY.md");
    println!("   RELEASE_PR_BODY.md");
    println!("   manifest.json");
    if !artifacts.is_empty() {
        println!("   dist/");
        for a in &artifacts {
            println!("     {}", a.name);
        }
    }
    println!();

    Ok(())
}

fn try_build(dist_dir: &Path) -> BuildResult {
    match Command::new("uv")
        .args(["build", "--out-dir"])
        .arg(dist_dir)
        .output()
    {
        Ok(output) if output.status.success() => BuildResult::Passed,
        Ok(_) => BuildResult::Failed,
        Err(_) => BuildResult::Skipped,
    }
}

fn collect_artifacts(dist_dir: &Path) -> Vec<ArtifactInfo> {
    let entries = match fs::read_dir(dist_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut artifacts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let size_bytes = path.metadata().map(|m| m.len()).unwrap_or(0);
            let sha256 = fs::read(&path)
                .map(|bytes| hex_digest(&sha256(&bytes)))
                .unwrap_or_default();
            artifacts.push(ArtifactInfo {
                name,
                size_bytes,
                sha256,
            });
        }
    }
    artifacts.sort_by(|a, b| a.name.cmp(&b.name));
    artifacts
}

fn build_manifest(
    version: &str,
    artifacts: &[ArtifactInfo],
    build: &BuildResult,
    validation: &ValidationResult,
    changelog_entry: &str,
    release_pr_body: &str,
) -> serde_json::Value {
    serde_json::json!({
        "version": version,
        "snapshot": true,
        "timestamp": timestamp_now(),
        "artifacts": artifacts.iter().map(|a| {
            serde_json::json!({
                "name": a.name,
                "size_bytes": a.size_bytes,
                "sha256": a.sha256,
            })
        }).collect::<Vec<_>>(),
        "changelog_entry": changelog_entry,
        "release_pr_body": release_pr_body,
        "checks": {
            "uv_build": match build {
                BuildResult::Passed => "passed",
                BuildResult::Failed => "failed",
                BuildResult::Skipped => "skipped",
            },
            "twine_check": match validation {
                ValidationResult::Passed => "passed",
                ValidationResult::Failed => "failed",
                ValidationResult::Skipped => "skipped",
            },
        }
    })
}

fn try_validate(dist_dir: &Path) -> ValidationResult {
    let entries = match fs::read_dir(dist_dir) {
        Ok(entries) => entries,
        Err(_) => return ValidationResult::Skipped,
    };

    let files: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    if files.is_empty() {
        return ValidationResult::Skipped;
    }

    let mut command = Command::new("twine");
    command.arg("check");
    command.args(&files);
    match command.output() {
        Ok(output) if output.status.success() => ValidationResult::Passed,
        Ok(_) => ValidationResult::Failed,
        Err(_) => ValidationResult::Skipped,
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn timestamp_now() -> String {
    if let Ok(output) = Command::new("git")
        .args(["show", "-s", "--format=%cI", "HEAD"])
        .output()
        && output.status.success()
    {
        let ts = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !ts.is_empty() {
            return ts;
        }
    }

    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s-since-epoch", duration.as_secs())
}
