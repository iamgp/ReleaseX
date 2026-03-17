use anyhow::Result;

use crate::{
    analysis::ReleaseAnalysis,
    config::{ChannelConfig, Config},
    cratesio, ecosystem,
    git::GitRepository,
    pypi,
    version::Version,
};

#[derive(Debug, Clone)]
pub struct ResolvedChannel {
    pub branch: String,
    pub publish: bool,
    pub prerelease: Option<String>,
    pub version_range: Option<String>,
}

pub fn release_base_branch(config: &Config, current_branch: &str) -> String {
    resolve_channel(config, current_branch, None)
        .map(|channel| channel.branch.clone())
        .unwrap_or_else(|| config.release.branch.clone())
}

pub fn resolve_channel<'a>(
    config: &'a Config,
    current_branch: &str,
    channel_arg: Option<&str>,
) -> Option<&'a ChannelConfig> {
    let wanted = channel_arg.unwrap_or(current_branch);
    config.channels.iter().find(|channel| {
        channel.branch == wanted
            || channel.prerelease.as_deref() == Some(wanted)
            || matches!(
                (channel.prerelease.as_deref(), wanted),
                (Some("a"), "alpha") | (Some("b"), "beta") | (Some("rc"), "rc")
            )
    })
}

pub fn apply_channel_to_analysis(
    repo: &GitRepository,
    config: &Config,
    analysis: &mut ReleaseAnalysis,
    current_branch: &str,
    channel_arg: Option<&str>,
) -> Result<Option<ResolvedChannel>> {
    let Some(channel) = resolve_channel(config, current_branch, channel_arg) else {
        return Ok(None);
    };

    if let Some(kind) = &channel.prerelease {
        let base = analysis
            .next_version
            .clone()
            .unwrap_or_else(|| analysis.current_version.bump_patch());
        let next = next_prerelease_for_package(repo, ".", &base, kind)?;
        analysis.next_version = Some(next);

        for package in &mut analysis.package_plan.packages {
            if !package.selected {
                continue;
            }
            let pkg_base = package
                .next_version
                .clone()
                .unwrap_or_else(|| package.current_version.bump_patch());
            package.next_version = Some(next_prerelease_for_package(
                repo,
                &package.root,
                &pkg_base,
                kind,
            )?);
        }
    }

    enforce_channel_range(analysis, channel)?;

    Ok(Some(ResolvedChannel {
        branch: channel.branch.clone(),
        publish: channel.publish,
        prerelease: channel.prerelease.clone(),
        version_range: channel.version_range.clone(),
    }))
}

fn next_prerelease_for_package(
    repo: &GitRepository,
    package_root: &str,
    base: &Version,
    kind: &str,
) -> Result<Version> {
    let ecosystem = ecosystem::detect(repo.path(), None);
    match ecosystem {
        crate::config::Ecosystem::Python => match pypi::project_name(repo.path(), package_root) {
            Some(project_name) => match pypi::next_prerelease_version(&project_name, base, kind) {
                Ok(version) => Ok(version),
                Err(_) => base.bump_pre(kind),
            },
            None => base.bump_pre(kind),
        },
        crate::config::Ecosystem::Rust => {
            let crate_name = crate::analysis::detect_project_name(repo.path(), package_root);
            match crate_name {
                Some(name) => match cratesio::latest_published_version(&name) {
                    Ok(Some(version))
                        if version.major == base.major
                            && version.minor == base.minor
                            && version.patch == base.patch
                            && matches!(version.suffix, Some(crate::version::Suffix::Pre(_))) =>
                    {
                        version.bump_pre(kind)
                    }
                    _ => base.bump_pre(kind),
                },
                None => base.bump_pre(kind),
            }
        }
        crate::config::Ecosystem::Go => base.bump_pre(kind),
    }
}

pub fn version_in_range(version: &Version, range: &str) -> bool {
    range.split(',').all(|raw| {
        let clause = raw.trim();
        if let Some(min) = clause.strip_prefix(">=") {
            return min
                .parse::<Version>()
                .map(|v| version >= &v)
                .unwrap_or(false);
        }
        if let Some(max) = clause.strip_prefix("<=") {
            return max
                .parse::<Version>()
                .map(|v| version <= &v)
                .unwrap_or(false);
        }
        if let Some(min) = clause.strip_prefix('>') {
            return min
                .parse::<Version>()
                .map(|v| version > &v)
                .unwrap_or(false);
        }
        if let Some(max) = clause.strip_prefix('<') {
            return max
                .parse::<Version>()
                .map(|v| version < &v)
                .unwrap_or(false);
        }
        if let Some(exact) = clause.strip_prefix("==") {
            return exact
                .parse::<Version>()
                .map(|v| version == &v)
                .unwrap_or(false);
        }
        false
    })
}

fn enforce_channel_range(analysis: &ReleaseAnalysis, channel: &ChannelConfig) -> Result<()> {
    let Some(range) = channel.version_range.as_deref() else {
        return Ok(());
    };

    for package in analysis
        .package_plan
        .packages
        .iter()
        .filter(|pkg| pkg.selected)
    {
        if let Some(next) = &package.next_version
            && !version_in_range(next, range)
        {
            anyhow::bail!(
                "next version {} for package {} is outside configured channel range {}",
                next,
                package.name,
                range
            );
        }
    }

    if let Some(next) = &analysis.next_version
        && !version_in_range(next, range)
    {
        anyhow::bail!(
            "next version {} is outside configured channel range {}",
            next,
            range
        );
    }

    Ok(())
}
