use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::version::Version;

#[derive(Debug, Clone, Deserialize)]
struct PypiResponse {
    info: PypiInfo,
    #[serde(default)]
    releases: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct PypiInfo {
    version: String,
}

pub fn project_name(repo_root: &Path, package_root: &str) -> Option<String> {
    let pyproject = if package_root == "." {
        repo_root.join("pyproject.toml")
    } else {
        repo_root.join(package_root).join("pyproject.toml")
    };
    let contents = fs::read_to_string(pyproject).ok()?;
    let parsed = contents.parse::<toml::Table>().ok()?;
    parsed
        .get("project")?
        .as_table()?
        .get("name")?
        .as_str()
        .map(ToString::to_string)
}

pub fn latest_published_version(project_name: &str) -> Result<Option<Version>> {
    let response = fetch_project(project_name)?;
    Ok(response.info.version.parse().ok())
}

pub fn has_version(project_name: &str, version: &Version) -> Result<bool> {
    let response = fetch_project(project_name)?;
    Ok(response.releases.contains_key(&version.to_string()))
}

pub fn next_prerelease_version(
    project_name: &str,
    base_version: &Version,
    kind: &str,
) -> Result<Version> {
    let response = fetch_project(project_name)?;
    let prefix = format!("{}{}", base_version.base(), kind);
    let mut max_n = 0;
    for release in response.releases.keys() {
        if !release.starts_with(&prefix) {
            continue;
        }
        if let Ok(version) = release.parse::<Version>() {
            let current = match version.suffix {
                Some(crate::version::Suffix::Pre(crate::version::PreRelease::Alpha(n)))
                    if kind == "a" =>
                {
                    n
                }
                Some(crate::version::Suffix::Pre(crate::version::PreRelease::Beta(n)))
                    if kind == "b" =>
                {
                    n
                }
                Some(crate::version::Suffix::Pre(crate::version::PreRelease::Rc(n)))
                    if kind == "rc" =>
                {
                    n
                }
                _ => continue,
            };
            max_n = max_n.max(current);
        }
    }

    let mut next = base_version.base();
    for _ in 0..=max_n {
        next = next.bump_pre(kind)?;
    }
    Ok(next)
}

fn fetch_project(project_name: &str) -> Result<PypiResponse> {
    fetch_project_from_base("https://pypi.org", project_name)
}

fn fetch_project_from_base(base_url: &str, project_name: &str) -> Result<PypiResponse> {
    let url = format!(
        "{}/pypi/{}/json",
        base_url.trim_end_matches('/'),
        project_name
    );
    let response = ureq::get(&url)
        .set("User-Agent", "relx")
        .call()
        .with_context(|| format!("failed to fetch PyPI metadata for {project_name}"))?;
    response
        .into_json()
        .with_context(|| format!("failed to parse PyPI metadata for {project_name}"))
}
