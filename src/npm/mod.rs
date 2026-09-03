use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result};

use crate::version::Version;

pub fn project_name(repo_root: &Path, package_root: &str) -> Option<String> {
    let manifest = if package_root == "." {
        repo_root.join("package.json")
    } else {
        repo_root.join(package_root).join("package.json")
    };
    let contents = fs::read_to_string(manifest).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&contents).ok()?;
    parsed.get("name")?.as_str().map(ToString::to_string)
}

fn npm_view(package_name: &str, field: &str) -> Result<String> {
    let output = Command::new("npm")
        .args(["view", package_name, field, "--json"])
        .output()
        .context("failed to run npm view")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("npm view failed for {package_name}: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn latest_published_version(project_name: &str) -> Result<Option<Version>> {
    match npm_view(project_name, "version") {
        Ok(raw) => Ok(raw.trim_matches('"').parse().ok()),
        Err(error) => {
            let message = error.to_string();
            if message.contains("E404") || message.contains("404") {
                return Ok(None);
            }
            Err(error)
        }
    }
}

pub fn has_version(project_name: &str, version: &Version) -> Result<bool> {
    let raw = npm_view(project_name, "versions")?;
    let versions: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(versions.iter().any(|v| v == &version.to_string()))
}
