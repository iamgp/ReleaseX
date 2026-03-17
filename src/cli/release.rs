use anyhow::{Context, Result};

use crate::{
    analysis,
    changelog::PendingChangelog,
    channels,
    cli::{Cli, PreReleaseArgs, PreReleaseKind, ReleaseCommand, ReleaseSubcommand},
    config::Config,
    git::GitRepository,
    github, progress, publish,
    version::{BumpLevel, Version},
};

fn apply_suffix_bump(version: &Version, kind: &PreReleaseKind) -> Result<Version> {
    match kind {
        PreReleaseKind::Alpha => version.bump_pre("a"),
        PreReleaseKind::Beta => version.bump_pre("b"),
        PreReleaseKind::Rc => version.bump_pre("rc"),
        PreReleaseKind::Post => Ok(version.bump_post()),
        PreReleaseKind::Dev => Ok(version.bump_dev()),
    }
}

fn apply_pre_release_override(
    analysis: &mut analysis::ReleaseAnalysis,
    args: &PreReleaseArgs,
) -> Result<()> {
    if args.finalize {
        let finalized = analysis.current_version.finalize();
        analysis.next_version = Some(finalized);
        for package in &mut analysis.package_plan.packages {
            if package.selected {
                package.next_version = Some(package.current_version.finalize());
            }
        }
    } else if let Some(kind) = &args.pre_release {
        let base = match &analysis.next_version {
            Some(v) => v.clone(),
            None => analysis.current_version.bump_patch(),
        };
        analysis.next_version = Some(apply_suffix_bump(&base, kind)?);
        for package in &mut analysis.package_plan.packages {
            if package.selected {
                let pkg_base = match &package.next_version {
                    Some(v) => v.clone(),
                    None => package.current_version.bump_patch(),
                };
                package.next_version = Some(apply_suffix_bump(&pkg_base, kind)?);
            }
        }
    }
    Ok(())
}

fn apply_channel_override(
    repo: &GitRepository,
    config: &Config,
    analysis: &mut analysis::ReleaseAnalysis,
    args: &PreReleaseArgs,
) -> Result<()> {
    let branch = repo
        .current_branch()
        .unwrap_or_else(|_| "unknown".to_string());
    channels::apply_channel_to_analysis(repo, config, analysis, &branch, args.channel.as_deref())?;
    Ok(())
}

/// When `release tag` runs after a release PR has been merged, the version
/// files already contain the bumped version (e.g. 0.2.0) and the latest tag
/// is still the old one (e.g. v0.1.0).  A naive re-analysis would scan the
/// commits since v0.1.0 — which now include the merge commit — and bump
/// *again* to 0.3.0.
///
/// This function detects that situation: the current version in the version
/// files is already newer than the latest tag, so we should tag the current
/// version rather than computing a new bump.
fn adjust_for_merged_release_pr(
    repo: &GitRepository,
    config: &Config,
    analysis: &mut analysis::ReleaseAnalysis,
) -> Result<()> {
    let tag_prefix = &config.release.tag_prefix;
    let latest_tag_version = repo
        .latest_tag()?
        .and_then(|tag| tag.strip_prefix(tag_prefix).map(|s| s.to_string()))
        .and_then(|s| s.parse::<Version>().ok());

    let Some(tag_version) = latest_tag_version else {
        return Ok(());
    };

    // If current version (from files) is already ahead of the latest tag,
    // the release PR has been merged — tag the current version as-is.
    if analysis.current_version > tag_version {
        let version = analysis.current_version.clone();
        analysis.next_version = Some(version.clone());
        analysis.bump = BumpLevel::None;
        analysis.changelog = PendingChangelog::from_commits(
            config,
            &analysis
                .commits
                .iter()
                .filter_map(|c| {
                    crate::conventional_commits::ConventionalCommit::parse_message(&c.message).ok()
                })
                .collect::<Vec<_>>(),
        );
        for package in &mut analysis.package_plan.packages {
            if package.selected {
                package.next_version = Some(version.clone());
                package.bump = BumpLevel::None;
            }
        }
    }

    Ok(())
}

pub fn run(cli: &Cli, command: &ReleaseCommand) -> Result<()> {
    if command.snapshot {
        return super::snapshot::run(cli);
    }

    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config = Config::load(&cli.config)?;

    let mut analysis = if cli.dry_run {
        analysis::analyze(&repo, &config)?
    } else {
        let sp = progress::spinner("Analyzing commits…");
        let result = analysis::analyze(&repo, &config);
        sp.finish_and_clear();
        result?
    };

    match &command.command {
        ReleaseSubcommand::Pr(args) => {
            apply_channel_override(&repo, &config, &mut analysis, args)?;
            apply_pre_release_override(&mut analysis, args)?;
            if cli.dry_run {
                github::print_release_pr_dry_run(&repo, &config, &analysis)?;
            } else if config.monorepo.enabled {
                let sp = progress::spinner("Creating monorepo release PR(s)…");
                let result = github::execute_monorepo_release_pr(&repo, &config, &analysis);
                sp.finish_and_clear();
                result?;
            } else {
                let sp = progress::spinner("Creating release PR…");
                let result = github::execute_release_pr(&repo, &config, &analysis);
                sp.finish_and_clear();
                result?;
            }
        }
        ReleaseSubcommand::Tag(args) => {
            adjust_for_merged_release_pr(&repo, &config, &mut analysis)?;
            apply_channel_override(&repo, &config, &mut analysis, args)?;
            apply_pre_release_override(&mut analysis, args)?;
            if cli.dry_run {
                github::print_release_tag_dry_run(&repo, &config, &analysis)?;
            } else if config.monorepo.enabled {
                let sp = progress::spinner("Tagging monorepo packages…");
                let result = github::execute_monorepo_release_tag(&repo, &config, &analysis);
                sp.finish_and_clear();
                result?;
            } else {
                let sp = progress::spinner("Creating tag and GitHub release…");
                let result = github::execute_release_tag(&repo, &config, &analysis);
                sp.finish_and_clear();
                result?;
            }
        }
        ReleaseSubcommand::Publish => {
            if cli.dry_run {
                publish::print_dry_run(repo.path(), &config)?;
            } else if config.monorepo.enabled {
                let sp = progress::spinner("Publishing monorepo packages…");
                let result = publish::execute_monorepo(repo.path(), &config, &analysis);
                sp.finish_and_clear();
                result?;
            } else {
                let sp = progress::spinner("Publishing…");
                let result = publish::execute(repo.path(), &config);
                sp.finish_and_clear();
                result?;
            }
        }
    }

    Ok(())
}
