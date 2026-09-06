use anyhow::{Context, Result, bail};

use crate::{
    config::{Config, VersionFileConfig},
    git::GitRepository,
    version::{Suffix, Version},
    version_files,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageBaseline {
    pub kind: String,
    pub reference: Option<String>,
    pub commit: Option<String>,
}

impl Default for PackageBaseline {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            reference: None,
            commit: None,
        }
    }
}

impl PackageBaseline {
    pub fn first_release() -> Self {
        Self {
            kind: "first_release".to_string(),
            reference: None,
            commit: None,
        }
    }
}

pub fn uses_independent_package_identity(config: &Config) -> bool {
    matches!(
        config.monorepo.release_mode.as_str(),
        "release_set" | "per_package"
    ) && config.monorepo.enabled
}

pub fn normalize_tag_package_name(name: &str) -> String {
    name.trim_start_matches('@').to_string()
}

pub fn package_release_tag(package_name: &str, version: &Version) -> String {
    format!("{}/v{}", normalize_tag_package_name(package_name), version)
}

pub fn parse_package_tag(tag: &str) -> Option<(String, Version)> {
    let index = tag.rfind("/v")?;
    let name = &tag[..index];
    if name.is_empty() {
        return None;
    }
    let version = tag[index + 2..].parse().ok()?;
    Some((name.to_string(), version))
}

pub fn is_package_identity_tag(tag: &str) -> bool {
    parse_package_tag(tag).is_some()
}

pub fn resolve_package_baseline(
    repo: &GitRepository,
    config: &Config,
    package_name: &str,
    version_files: &[VersionFileConfig],
    prerelease_kind: Option<&str>,
    known_package_names: &[String],
) -> Result<PackageBaseline> {
    if !uses_independent_package_identity(config) {
        return shared_tag_baseline(repo, config);
    }

    let tag_names = repo.list_tags()?;
    let package_tags = collect_package_tags(&tag_names, package_name);
    if !package_tags.is_empty() {
        return select_package_tag_baseline(
            repo,
            package_name,
            version_files,
            prerelease_kind,
            &package_tags,
        );
    }

    if config
        .monorepo
        .first_release_packages
        .iter()
        .any(|name| name == package_name)
    {
        return Ok(PackageBaseline::first_release());
    }

    if let Some(legacy) = config
        .monorepo
        .legacy_releases
        .iter()
        .rev()
        .find(|entry| entry.packages.iter().any(|name| name == package_name))
    {
        return resolve_legacy_baseline(repo, legacy, package_name, version_files);
    }

    if config.monorepo.legacy_releases.is_empty()
        && !tag_names.iter().any(|tag| {
            parse_package_tag(tag).is_some_and(|(name, _)| {
                known_package_names
                    .iter()
                    .any(|pkg| normalize_tag_package_name(pkg) == name)
            })
        })
    {
        return shared_tag_baseline(repo, config);
    }

    bail!(
        "package `{package_name}` has no release identity tag. Add `{package_name}` to a [[monorepo.legacy_releases]] entry (if a shared tag published it) or to [monorepo].first_release_packages (if it has never been released). Missing history is not inferred from unrelated shared tags."
    );
}

fn collect_package_tags(tags: &[String], package_name: &str) -> Vec<(String, Version)> {
    let normalized = normalize_tag_package_name(package_name);
    tags.iter()
        .filter_map(|tag| {
            let (name, version) = parse_package_tag(tag)?;
            (name == normalized).then_some((tag.clone(), version))
        })
        .collect()
}

fn select_package_tag_baseline(
    repo: &GitRepository,
    package_name: &str,
    version_files: &[VersionFileConfig],
    prerelease_kind: Option<&str>,
    tags: &[(String, Version)],
) -> Result<PackageBaseline> {
    let mut rejected = Vec::new();
    let mut candidates = Vec::new();
    for (tag, version) in tags {
        if !version_matches_channel(version, prerelease_kind) {
            rejected.push(format!(
                "{tag} is on a different release channel than the current request"
            ));
            continue;
        }
        if !repo.is_ancestor(tag, "HEAD")? {
            rejected.push(format!("{tag} is not an ancestor of HEAD"));
            continue;
        }
        let commit = repo.rev_parse(tag)?;
        match version_at_commit(repo, &commit, version_files)? {
            Some(recorded) if recorded == *version => {}
            Some(recorded) => {
                rejected.push(format!(
                    "{tag} records package version {recorded} at {commit}, not {version}"
                ));
                continue;
            }
            None => {
                rejected.push(format!(
                    "{tag} does not contain package `{package_name}` at {commit}"
                ));
                continue;
            }
        }
        candidates.push((tag.clone(), commit, version.clone()));
    }

    if candidates.is_empty() {
        bail!(
            "no valid release baseline for package `{package_name}`: {}",
            rejected.join("; ")
        );
    }

    let mut maximal = Vec::new();
    for candidate in &candidates {
        let is_dominated = candidates.iter().any(|other| {
            other.0 != candidate.0 && repo.is_ancestor(&candidate.0, &other.0).unwrap_or(false)
        });
        if !is_dominated {
            maximal.push(candidate.clone());
        }
    }

    if maximal.len() != 1 {
        bail!(
            "ambiguous release baselines for package `{package_name}`: {}",
            maximal
                .iter()
                .map(|(tag, commit, _)| format!("{tag} ({commit})"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let (tag, commit, _) = &maximal[0];
    Ok(PackageBaseline {
        kind: "package_tag".to_string(),
        reference: Some(tag.clone()),
        commit: Some(commit.clone()),
    })
}

fn resolve_legacy_baseline(
    repo: &GitRepository,
    legacy: &crate::config::LegacyReleaseConfig,
    package_name: &str,
    version_files: &[VersionFileConfig],
) -> Result<PackageBaseline> {
    if !repo.commit_exists(&legacy.tag) && repo.rev_parse(&legacy.tag).is_err() {
        bail!(
            "legacy baseline tag `{}` for package `{package_name}` was not found",
            legacy.tag
        );
    }
    let commit = repo.rev_parse(&legacy.tag)?;
    if let Some(expected) = legacy.commit.as_deref() {
        let expected = expected.trim();
        if !commit.starts_with(expected) && !expected.starts_with(&commit) {
            bail!(
                "legacy baseline tag `{}` resolves to {commit}, not the configured commit {expected}",
                legacy.tag
            );
        }
    }
    if !repo.is_ancestor(&legacy.tag, "HEAD")? {
        bail!(
            "legacy baseline tag `{}` for package `{package_name}` is not an ancestor of HEAD",
            legacy.tag
        );
    }
    if version_at_commit(repo, &commit, version_files)?.is_none() {
        bail!(
            "legacy baseline tag `{}` does not contain package `{package_name}` at {commit}",
            legacy.tag
        );
    }
    Ok(PackageBaseline {
        kind: "legacy_tag".to_string(),
        reference: Some(legacy.tag.clone()),
        commit: Some(commit),
    })
}

fn shared_tag_baseline(repo: &GitRepository, config: &Config) -> Result<PackageBaseline> {
    let Some(tag) = latest_shared_tag(repo, &config.release.tag_prefix)? else {
        return Ok(PackageBaseline::first_release());
    };
    if !repo.is_ancestor(&tag, "HEAD")? {
        bail!("shared release tag `{tag}` is not an ancestor of HEAD");
    }
    Ok(PackageBaseline {
        kind: "shared_tag".to_string(),
        reference: Some(tag.clone()),
        commit: Some(repo.rev_parse(&tag)?),
    })
}

fn latest_shared_tag(repo: &GitRepository, prefix: &str) -> Result<Option<String>> {
    if let Some(tag) = repo.latest_tag()?
        && !is_package_identity_tag(&tag)
    {
        return Ok(Some(tag));
    }

    let mut shared = Vec::new();
    for tag in repo.list_tags()? {
        if is_package_identity_tag(&tag) {
            continue;
        }
        let Some(version) = tag
            .strip_prefix(prefix)
            .and_then(|value| value.parse::<Version>().ok())
        else {
            continue;
        };
        if repo.is_ancestor(&tag, "HEAD").unwrap_or(false) {
            shared.push((tag, version));
        }
    }
    shared.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(shared.pop().map(|(tag, _)| tag))
}

fn version_matches_channel(version: &Version, prerelease_kind: Option<&str>) -> bool {
    match prerelease_kind {
        None => !matches!(version.suffix, Some(Suffix::Pre(_))),
        Some(kind) => match &version.suffix {
            None => true,
            Some(Suffix::Pre(pre)) => match kind {
                "a" | "alpha" => matches!(pre, crate::version::PreRelease::Alpha(_)),
                "b" | "beta" => matches!(pre, crate::version::PreRelease::Beta(_)),
                "rc" => matches!(pre, crate::version::PreRelease::Rc(_)),
                _ => false,
            },
            Some(_) => false,
        },
    }
}

pub fn version_at_commit(
    repo: &GitRepository,
    commit: &str,
    version_files: &[VersionFileConfig],
) -> Result<Option<Version>> {
    for version_file in version_files {
        let Some(contents) = repo.file_at_commit(commit, &version_file.path)? else {
            continue;
        };
        let value = if let Some(key) = &version_file.key {
            version_files::read_key_from_contents(&version_file.path, &contents, key)?
        } else if let Some(pattern) = &version_file.pattern {
            version_files::read_pattern_from_contents(&contents, pattern)?
        } else {
            None
        };
        if let Some(value) = value {
            return Ok(Some(value.parse().with_context(|| {
                format!(
                    "invalid version `{value}` in {} at {commit}",
                    version_file.path
                )
            })?));
        }
    }
    Ok(None)
}

pub fn is_bookkeeping_path(path: &str, config: &Config) -> bool {
    let plan_file = config.release.plan_file.trim();
    path == ".relx"
        || path.starts_with(".relx/")
        || path == plan_file
        || path == config.release.changelog_file
        || matches!(
            path,
            "uv.lock"
                | "Cargo.lock"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "go.sum"
                | "CHANGELOG.md"
        )
}

#[cfg(test)]
mod tests {
    use super::{package_release_tag, parse_package_tag, version_matches_channel};
    use crate::version::Version;

    #[test]
    fn parses_preferred_package_tag_convention() {
        let (name, version) = parse_package_tag("phlo-polaris/v0.15.3").expect("tag");
        assert_eq!(name, "phlo-polaris");
        assert_eq!(version.to_string(), "0.15.3");
        assert_eq!(
            package_release_tag("phlo-polaris", &version),
            "phlo-polaris/v0.15.3"
        );
    }

    #[test]
    fn stable_channel_ignores_prerelease_tags() {
        let beta: Version = "1.1.0b1".parse().unwrap();
        let stable: Version = "1.0.0".parse().unwrap();
        assert!(!version_matches_channel(&beta, None));
        assert!(version_matches_channel(&stable, None));
        assert!(version_matches_channel(&beta, Some("b")));
        assert!(version_matches_channel(&stable, Some("b")));
    }
}
