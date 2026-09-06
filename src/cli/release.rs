use anyhow::{Context, Result};

use crate::{
    analysis::{self, AnalyzeOptions},
    changelog::PendingChangelog,
    channels,
    cli::{Cli, PreReleaseArgs, PreReleaseKind, ReleaseCommand, ReleaseSubcommand},
    config::{Config, Ecosystem},
    git::GitRepository,
    github, progress,
    promotion::{PreviewOptions, ReleaseOptions},
    publish,
    version::{BumpLevel, Suffix, Version},
};

fn analyze_cli(
    repo: &GitRepository,
    config: &Config,
    packages: &[String],
    channel: Option<&str>,
) -> Result<analysis::ReleaseAnalysis> {
    let branch = repo
        .current_branch()
        .unwrap_or_else(|_| "unknown".to_string());
    let prerelease_kind = channels::resolve_channel(config, &branch, channel)
        .and_then(|channel| channel.prerelease.clone());
    analysis::analyze_with(
        repo,
        config,
        &AnalyzeOptions {
            packages: packages.to_vec(),
            prerelease_kind,
        },
    )
}

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
    config: &Config,
    analysis: &mut analysis::ReleaseAnalysis,
    args: &PreReleaseArgs,
) -> Result<()> {
    if args.finalize {
        select_prerelease_packages_for_finalize(config, analysis);
        let finalized = analysis.current_version.finalize();
        analysis.next_version = Some(finalized);
        for package in &mut analysis.package_plan.packages {
            if package.selected {
                package.next_version = Some(package.current_version.finalize());
            }
        }
    } else if let Some(kind) = &args.pre_release {
        select_prerelease_workspace_root(config, analysis);
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

fn parse_stable_next_version(value: &str) -> Result<Version> {
    let version: Version = value
        .parse()
        .with_context(|| format!("invalid --next-version `{value}`"))?;
    if version.suffix.is_some() {
        anyhow::bail!("--next-version must be a stable version, not `{version}`");
    }
    Ok(version)
}

fn apply_next_version_override(
    analysis: &mut analysis::ReleaseAnalysis,
    args: &PreReleaseArgs,
) -> Result<()> {
    let Some(value) = args.next_version.as_deref() else {
        return Ok(());
    };
    let version = parse_stable_next_version(value)?;

    if version <= analysis.current_version {
        anyhow::bail!(
            "--next-version {version} must be newer than current version {}",
            analysis.current_version
        );
    }
    for package in analysis
        .package_plan
        .packages
        .iter()
        .filter(|pkg| pkg.selected)
    {
        if version <= package.current_version {
            anyhow::bail!(
                "--next-version {version} must be newer than current version {} for package {}",
                package.current_version,
                package.name
            );
        }
    }

    analysis.next_version = Some(version.clone());
    for package in analysis
        .package_plan
        .packages
        .iter_mut()
        .filter(|pkg| pkg.selected)
    {
        package.next_version = Some(version.clone());
    }
    Ok(())
}

fn validate_next_version_channel(
    config: &Config,
    repo: &GitRepository,
    args: &PreReleaseArgs,
) -> Result<()> {
    if args.next_version.is_none() {
        return Ok(());
    }
    let branch = repo
        .current_branch()
        .unwrap_or_else(|_| "unknown".to_string());
    if let Some(channel) = channels::resolve_channel(config, &branch, args.channel.as_deref())
        && channel.prerelease.is_some()
    {
        anyhow::bail!("--next-version cannot be used with a prerelease channel");
    }
    Ok(())
}

fn validate_tag_next_version(
    analysis: &analysis::ReleaseAnalysis,
    args: &PreReleaseArgs,
) -> Result<()> {
    let Some(value) = args.next_version.as_deref() else {
        return Ok(());
    };
    let version = parse_stable_next_version(value)?;
    if version != analysis.current_version {
        anyhow::bail!(
            "--next-version {version} does not match prepared source version {}; release tag derives its version from source",
            analysis.current_version
        );
    }
    for package in analysis
        .package_plan
        .packages
        .iter()
        .filter(|pkg| pkg.selected)
    {
        if package.current_version != version {
            anyhow::bail!(
                "--next-version {version} does not match prepared source version {} for package {}",
                package.current_version,
                package.name
            );
        }
    }
    Ok(())
}

fn select_prerelease_workspace_root(config: &Config, analysis: &mut analysis::ReleaseAnalysis) {
    if !python_prerelease_workspace_enabled(config, analysis) {
        return;
    }

    let Some(root_package) = analysis
        .package_plan
        .packages
        .iter_mut()
        .find(|package| package.root == ".")
    else {
        return;
    };

    if root_package.selected {
        return;
    }

    root_package.selected = true;
    root_package.bump = BumpLevel::Patch;
    root_package.next_version = Some(root_package.current_version.bump_patch());
    root_package.selection_reason = "root prerelease".to_string();
}

fn select_prerelease_packages_for_finalize(
    config: &Config,
    analysis: &mut analysis::ReleaseAnalysis,
) {
    if !python_prerelease_workspace_enabled(config, analysis) {
        return;
    }

    for package in &mut analysis.package_plan.packages {
        if !version_is_prerelease(&package.current_version) {
            continue;
        }

        package.selected = true;
        package.bump = BumpLevel::None;
        package.next_version = Some(package.current_version.finalize());
        package.selection_reason = "finalize prerelease package".to_string();
    }
}

fn python_prerelease_workspace_enabled(
    config: &Config,
    analysis: &analysis::ReleaseAnalysis,
) -> bool {
    config.prerelease.enabled
        && config.project.ecosystem == Some(Ecosystem::Python)
        && config.monorepo.enabled
        && analysis.package_plan.release_mode == "release_set"
}

fn version_is_prerelease(version: &Version) -> bool {
    matches!(version.suffix, Some(Suffix::Pre(_)))
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
    if channels::resolve_channel(config, &branch, args.channel.as_deref())
        .and_then(|channel| channel.prerelease.as_ref())
        .is_some()
    {
        select_prerelease_workspace_root(config, analysis);
    }
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
        // No tags yet: if the version files are already ahead of the initial
        // version, the first release PR has been merged — tag the current
        // version as-is instead of bumping the full history again.
        let initial: Version = config.versioning.initial_version.parse().with_context(|| {
            format!(
                "versioning.initial_version `{}` is not a valid version",
                config.versioning.initial_version
            )
        })?;
        if analysis.current_version > initial {
            snap_to_current_version(config, analysis);
        }
        return Ok(());
    };

    // If current version (from files) is already ahead of the latest tag,
    // the release PR has been merged — tag the current version as-is.
    if analysis.current_version > tag_version {
        snap_to_current_version(config, analysis);
    }

    Ok(())
}

fn snap_to_current_version(config: &Config, analysis: &mut analysis::ReleaseAnalysis) {
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

fn analyze_for_publish(repo: &GitRepository, config: &Config) -> Result<analysis::ReleaseAnalysis> {
    let analysis = analysis::analyze(repo, config)?;
    if !(config.monorepo.enabled && analysis.package_plan.release_mode != "unified") {
        return Ok(analysis);
    }

    if !analysis.package_plan.selected_packages().is_empty() {
        return Ok(analysis);
    }

    let Some(previous_tag) = repo.previous_tag_before_head()? else {
        return Ok(analysis);
    };

    analysis::analyze_since(repo, config, &previous_tag)
}

pub fn run(cli: &Cli, command: &ReleaseCommand) -> Result<()> {
    if command.snapshot {
        return super::snapshot::run(cli);
    }

    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config = Config::load(&cli.config_path())?;

    match &command.command {
        ReleaseSubcommand::Plan(args) => {
            let mut analysis = analyze_cli(&repo, &config, &args.package, None)?;
            let release_args = PreReleaseArgs {
                channel: None,
                next_version: None,
                pre_release: None,
                finalize: false,
                package: args.package.clone(),
            };
            apply_channel_override(&repo, &config, &mut analysis, &release_args)?;
            let base = channels::release_base_branch(&config, &repo.current_branch()?);
            if analysis.package_plan.selected_packages().is_empty() {
                anyhow::bail!("no releasable package set is pending from the current commit set");
            }
            let manifest = crate::manifest::ReleaseManifest::from_analysis(
                &repo,
                &config,
                &analysis,
                &base,
                repo.path(),
            )?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            } else {
                println!("Release plan (schema {}):", manifest.schema_version);
                println!(
                    "Preparation base: {} @ {}",
                    manifest.preparation_base.ref_name, manifest.preparation_base.commit
                );
                for package in manifest.packages.iter().filter(|package| package.selected) {
                    println!(
                        "{} [{}]: {} -> {} (tag {}, baseline {})",
                        package.name,
                        package.path,
                        package.current_version,
                        package.next_version.as_deref().unwrap_or("unchanged"),
                        package.release_tag,
                        package
                            .baseline
                            .reference
                            .as_deref()
                            .unwrap_or(&package.baseline.kind)
                    );
                    println!("  {}", package.selection_reason);
                }
                for change in &manifest.required_dependency_changes {
                    println!(
                        "Required dependency change: {} must update {} to include {}",
                        change.package, change.dependency, change.required_version
                    );
                }
            }
        }
        ReleaseSubcommand::Prepare(args) => {
            let mut analysis = analyze_cli(
                &repo,
                &config,
                &args.release.package,
                args.release.channel.as_deref(),
            )?;
            apply_channel_override(&repo, &config, &mut analysis, &args.release)?;
            apply_pre_release_override(&config, &mut analysis, &args.release)?;
            if args.check {
                github::prepare_release_workspace_check(&repo, &config, &analysis)?;
            } else {
                github::prepare_release_workspace(&repo, &config, &analysis)?;
            }
        }
        ReleaseSubcommand::Pr(args) => {
            let mut analysis = if cli.dry_run {
                analyze_cli(&repo, &config, &args.package, args.channel.as_deref())?
            } else {
                let sp = progress::spinner("Analyzing commits…");
                let result = analyze_cli(&repo, &config, &args.package, args.channel.as_deref());
                sp.finish_and_clear();
                result?
            };
            validate_next_version_channel(&config, &repo, args)?;
            apply_next_version_override(&mut analysis, args)?;
            apply_channel_override(&repo, &config, &mut analysis, args)?;
            apply_pre_release_override(&config, &mut analysis, args)?;
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
            let mut analysis = if cli.dry_run {
                analyze_cli(&repo, &config, &args.package, args.channel.as_deref())?
            } else {
                let sp = progress::spinner("Analyzing commits…");
                let result = analyze_cli(&repo, &config, &args.package, args.channel.as_deref());
                sp.finish_and_clear();
                result?
            };
            if crate::baseline::uses_independent_package_identity(&config) {
                github::apply_manifest_to_analysis(&repo, &config, &mut analysis)?;
            } else {
                adjust_for_merged_release_pr(&repo, &config, &mut analysis)?;
            }
            validate_next_version_channel(&config, &repo, args)?;
            validate_tag_next_version(&analysis, args)?;
            apply_channel_override(&repo, &config, &mut analysis, args)?;
            apply_pre_release_override(&config, &mut analysis, args)?;
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
        ReleaseSubcommand::Publish(args) => {
            let analysis = if cli.dry_run {
                analysis::analyze(&repo, &config)?
            } else {
                let sp = progress::spinner("Analyzing commits…");
                let result = analyze_for_publish(&repo, &config);
                sp.finish_and_clear();
                result?
            };
            if cli.dry_run {
                publish::print_dry_run(repo.path(), &config, args.skip_published)?;
            } else if config.monorepo.enabled && analysis.package_plan.release_mode != "unified" {
                let sp = progress::spinner("Publishing monorepo packages…");
                let result =
                    publish::execute_monorepo(repo.path(), &config, &analysis, args.skip_published);
                sp.finish_and_clear();
                result?;
            } else {
                let sp = progress::spinner("Publishing…");
                let result = publish::execute(repo.path(), &config, args.skip_published);
                sp.finish_and_clear();
                result?;
            }
        }
        ReleaseSubcommand::PreviewPr(args) => {
            let options = PreviewOptions {
                pr_number: args.pr,
                head_branch: args.head.clone(),
                base_branch: args.base.clone(),
                json: args.json,
            };
            if cli.dry_run {
                crate::promotion::execute_preview(&repo, &config, &options, true)?;
            } else {
                let sp = progress::spinner("Previewing promotion release…");
                let result = crate::promotion::execute_preview(&repo, &config, &options, false);
                sp.finish_and_clear();
                result?;
            }
        }
        ReleaseSubcommand::Release(args) => {
            let options = ReleaseOptions {
                pr_number: args.pr,
                json: args.json,
            };
            if cli.dry_run {
                crate::promotion::execute_release(&repo, &config, &options, true)?;
            } else {
                let sp = progress::spinner("Cutting release…");
                let result = crate::promotion::execute_release(&repo, &config, &options, false);
                sp.finish_and_clear();
                result?;
            }
        }
        ReleaseSubcommand::ForwardPort(_) => {
            if cli.dry_run {
                crate::promotion::execute_forward_port(&repo, &config, true)?;
            } else {
                let sp = progress::spinner("Forward-porting production…");
                let result = crate::promotion::execute_forward_port(&repo, &config, false);
                sp.finish_and_clear();
                result?;
            }
        }
        ReleaseSubcommand::VerifyPlan(_) => {
            github::verify_release_manifest(&repo, &config)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, process::Command};

    use tempfile::tempdir;

    use super::{
        adjust_for_merged_release_pr, analyze_for_publish, apply_next_version_override,
        apply_pre_release_override, validate_next_version_channel, validate_tag_next_version,
    };
    use crate::analysis::{PackagePlan, PackageReleaseAnalysis, ReleaseAnalysis};
    use crate::changelog::PendingChangelog;
    use crate::cli::{PreReleaseArgs, PreReleaseKind};
    use crate::config::Config;
    use crate::git::GitRepository;
    use crate::version::{BumpLevel, PreRelease, Suffix, Version};

    #[test]
    fn merged_first_release_pr_tags_current_version_without_tags() {
        let repo_dir = tempdir().expect("tempdir");
        let repo_path = repo_dir.path();

        git(repo_path, &["init", "-b", "main"]);
        git(repo_path, &["config", "user.name", "Relx Test"]);
        git(repo_path, &["config", "user.email", "relx@example.com"]);
        git(repo_path, &["config", "commit.gpgsign", "false"]);
        git(repo_path, &["config", "tag.gpgsign", "false"]);
        fs::write(
            repo_path.join("package.json"),
            "{\"name\": \"demo\", \"version\": \"0.2.0\"}",
        )
        .expect("write manifest");
        fs::write(
            repo_path.join("relx.toml"),
            "[versioning]\ninitial_version = \"0.1.0\"\n\n[[version_files]]\npath = \"package.json\"\nkey = \"version\"\n",
        )
        .expect("write config");
        git(repo_path, &["add", "."]);
        git(repo_path, &["commit", "-m", "feat: initial package"]);

        let repo = GitRepository::discover(repo_path).expect("repo");
        let config = Config::load(&repo_path.join("relx.toml")).expect("config");
        let mut analysis = crate::analysis::analyze(&repo, &config).expect("analysis");
        assert_eq!(analysis.next_version.as_ref().unwrap().to_string(), "0.3.0");

        adjust_for_merged_release_pr(&repo, &config, &mut analysis).expect("adjust");

        assert_eq!(analysis.next_version.as_ref().unwrap().to_string(), "0.2.0");
        assert_eq!(analysis.bump, BumpLevel::None);
    }

    #[test]
    fn analyze_for_publish_uses_previous_tag_for_release_set_tag_commits() {
        let repo_dir = tempdir().expect("tempdir");
        let repo_path = repo_dir.path();

        run(repo_path, &["git", "init", "-b", "main"]);
        run(repo_path, &["git", "config", "user.name", "Relx Test"]);
        run(
            repo_path,
            &["git", "config", "user.email", "relx@example.com"],
        );

        fs::create_dir_all(repo_path.join("packages/delta/src")).expect("create package dirs");
        fs::write(
            repo_path.join("pyproject.toml"),
            r#"[project]
name = "phlo"
version = "0.7.3"

[tool.uv.workspace]
members = ["packages/delta"]
"#,
        )
        .expect("write root pyproject");
        fs::write(repo_path.join("src_placeholder.txt"), "root initial\n")
            .expect("write root placeholder");
        fs::write(
            repo_path.join("packages/delta/pyproject.toml"),
            r#"[project]
name = "phlo-delta"
version = "0.2.3"
"#,
        )
        .expect("write package pyproject");
        fs::write(
            repo_path.join("packages/delta/src/mod.py"),
            "print('initial')\n",
        )
        .expect("write package source");
        fs::write(
            repo_path.join("relx.toml"),
            r#"[project]
ecosystem = "python"

[release]
branch = "main"
tag_prefix = "v"

[versioning]
strategy = "conventional_commits"
initial_version = "0.7.0"

[[version_files]]
path = "pyproject.toml"
key = "project.version"

[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/delta"]

[workspace]
cascade_bumps = false
"#,
        )
        .expect("write config");
        run(repo_path, &["git", "add", "."]);
        run(
            repo_path,
            &["git", "commit", "-m", "chore: initial release state"],
        );
        run(
            repo_path,
            &["git", "-c", "tag.gpgSign=false", "tag", "v0.7.3"],
        );

        fs::write(repo_path.join("src_placeholder.txt"), "root changed\n").expect("update root");
        fs::write(
            repo_path.join("packages/delta/src/mod.py"),
            "print('changed')\n",
        )
        .expect("update package");
        run(
            repo_path,
            &[
                "git",
                "add",
                "src_placeholder.txt",
                "packages/delta/src/mod.py",
            ],
        );
        run(
            repo_path,
            &["git", "commit", "-m", "fix: centralize host resolution"],
        );

        fs::write(
            repo_path.join("pyproject.toml"),
            r#"[project]
name = "phlo"
version = "0.7.4"

[tool.uv.workspace]
members = ["packages/delta"]
"#,
        )
        .expect("bump root version");
        fs::write(
            repo_path.join("packages/delta/pyproject.toml"),
            r#"[project]
name = "phlo-delta"
version = "0.2.4"
"#,
        )
        .expect("bump package version");
        run(
            repo_path,
            &[
                "git",
                "add",
                "pyproject.toml",
                "packages/delta/pyproject.toml",
            ],
        );
        run(
            repo_path,
            &[
                "git",
                "commit",
                "-m",
                "chore(release): phlo 0.7.4 + 1 packages",
            ],
        );
        run(
            repo_path,
            &[
                "git",
                "-c",
                "tag.gpgSign=false",
                "tag",
                "v2pkgs-phlo-phlo-delta-deadbeef",
            ],
        );

        let repo = GitRepository::discover(repo_path).expect("repo");
        let config = Config::load(&repo_path.join("relx.toml")).expect("config");
        let analysis = analyze_for_publish(&repo, &config).expect("analysis");
        let selected = analysis.package_plan.selected_packages();

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name, "phlo");
        assert_eq!(selected[1].name, "phlo-delta");
    }

    #[test]
    fn beta_prerelease_selects_root_package_when_workspace_config_includes_root() {
        let config: Config = toml::from_str(
            r#"
            [project]
            ecosystem = "python"

            [prerelease]
            enabled = true

            [monorepo]
            enabled = true
            release_mode = "release_set"
            "#,
        )
        .expect("config");
        let mut analysis = sample_release_set_analysis(false);
        let args = PreReleaseArgs {
            channel: None,
            next_version: None,
            pre_release: Some(PreReleaseKind::Beta),
            finalize: false,
            package: Vec::new(),
        };

        apply_pre_release_override(&config, &mut analysis, &args).expect("apply beta override");

        let selected = analysis.package_plan.selected_packages();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name, "phlo");
        assert_eq!(selected[0].selection_reason, "root prerelease");
        assert_eq!(
            selected[0].next_version.as_ref().unwrap().to_string(),
            "0.8.1b1"
        );
        assert_eq!(selected[1].name, "phlo-iceberg");
        assert_eq!(
            selected[1].next_version.as_ref().unwrap().to_string(),
            "0.3.1b1"
        );
    }

    #[test]
    fn finalize_selects_all_packages_currently_on_prerelease_versions() {
        let config: Config = toml::from_str(
            r#"
            [project]
            ecosystem = "python"

            [prerelease]
            enabled = true

            [monorepo]
            enabled = true
            release_mode = "release_set"
            "#,
        )
        .expect("config");
        let mut analysis = sample_finalize_analysis();
        let args = PreReleaseArgs {
            channel: None,
            next_version: None,
            pre_release: None,
            finalize: true,
            package: Vec::new(),
        };

        apply_pre_release_override(&config, &mut analysis, &args).expect("apply finalize");

        let selected = analysis.package_plan.selected_packages();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name, "phlo");
        assert_eq!(selected[0].selection_reason, "finalize prerelease package");
        assert_eq!(
            selected[0].next_version.as_ref().unwrap().to_string(),
            "0.8.1"
        );
        assert_eq!(selected[1].name, "phlo-iceberg");
        assert_eq!(
            selected[1].next_version.as_ref().unwrap().to_string(),
            "0.3.1"
        );
        assert!(!analysis.package_plan.packages[2].selected);
    }

    #[test]
    fn next_version_override_replaces_the_conventional_commit_bump() {
        let mut analysis = sample_release_set_analysis(true);
        let args = PreReleaseArgs {
            channel: None,
            next_version: Some("0.14.0".to_string()),
            pre_release: None,
            finalize: false,
            package: Vec::new(),
        };

        apply_next_version_override(&mut analysis, &args).expect("apply version override");

        assert_eq!(analysis.next_version.unwrap().to_string(), "0.14.0");
        assert!(
            analysis
                .package_plan
                .selected_packages()
                .iter()
                .all(|package| package.next_version.as_ref().unwrap().to_string() == "0.14.0")
        );
    }

    #[test]
    fn next_version_override_rejects_invalid_or_non_incrementing_versions() {
        let mut analysis = sample_release_set_analysis(true);
        analysis.current_version = "0.12.1".parse().unwrap();
        analysis.package_plan.packages[0].current_version = "0.12.1".parse().unwrap();
        analysis.package_plan.packages[1].current_version = "0.12.1".parse().unwrap();

        for value in ["not-a-version", "0.12.1"] {
            let args = PreReleaseArgs {
                channel: None,
                next_version: Some(value.to_string()),
                pre_release: None,
                finalize: false,
                package: Vec::new(),
            };
            assert!(apply_next_version_override(&mut analysis, &args).is_err());
        }
    }

    #[test]
    fn next_version_override_rejects_prerelease_channels() {
        let repo_dir = tempdir().expect("tempdir");
        run(repo_dir.path(), &["git", "init", "-b", "beta"]);
        let repo = GitRepository::discover(repo_dir.path()).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [[channels]]
            branch = "beta"
            prerelease = "b"
            "#,
        )
        .expect("config");
        let args = PreReleaseArgs {
            channel: None,
            next_version: Some("0.14.0".to_string()),
            pre_release: None,
            finalize: false,
            package: Vec::new(),
        };

        assert!(validate_next_version_channel(&config, &repo, &args).is_err());
    }

    #[test]
    fn tag_next_version_must_match_prepared_source() {
        let mut analysis = sample_release_set_analysis(true);
        analysis.current_version = "0.14.0".parse().unwrap();
        for package in &mut analysis.package_plan.packages {
            if package.selected {
                package.current_version = "0.14.0".parse().unwrap();
            }
        }
        let matching = PreReleaseArgs {
            channel: None,
            next_version: Some("0.14.0".to_string()),
            pre_release: None,
            finalize: false,
            package: Vec::new(),
        };
        validate_tag_next_version(&analysis, &matching).expect("matching prepared source");

        let mismatched = PreReleaseArgs {
            next_version: Some("0.14.1".to_string()),
            ..matching
        };
        assert!(validate_tag_next_version(&analysis, &mismatched).is_err());
    }

    fn sample_release_set_analysis(root_selected: bool) -> ReleaseAnalysis {
        ReleaseAnalysis {
            current_version: Version {
                major: 0,
                minor: 8,
                patch: 0,
                suffix: None,
            },
            next_version: Some(Version {
                major: 0,
                minor: 8,
                patch: 1,
                suffix: None,
            }),
            bump: BumpLevel::Patch,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::new(),
                contributors: Vec::new(),
            },
            package_plan: PackagePlan {
                release_mode: "release_set".to_string(),
                discovery_source: "test".to_string(),
                packages: vec![
                    PackageReleaseAnalysis {
                        name: "phlo".to_string(),
                        root: ".".to_string(),
                        current_version: Version {
                            major: 0,
                            minor: 8,
                            patch: 0,
                            suffix: None,
                        },
                        next_version: if root_selected {
                            Some(Version {
                                major: 0,
                                minor: 8,
                                patch: 1,
                                suffix: None,
                            })
                        } else {
                            None
                        },
                        bump: if root_selected {
                            BumpLevel::Patch
                        } else {
                            BumpLevel::None
                        },
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::new(),
                        changed_paths: Vec::new(),
                        selected: root_selected,
                        selection_reason: "test".to_string(),
                        baseline: Default::default(),
                        release_tag: String::new(),
                        required_dependency_changes: Vec::new(),
                    },
                    PackageReleaseAnalysis {
                        name: "phlo-iceberg".to_string(),
                        root: "packages/iceberg".to_string(),
                        current_version: Version {
                            major: 0,
                            minor: 3,
                            patch: 0,
                            suffix: None,
                        },
                        next_version: Some(Version {
                            major: 0,
                            minor: 3,
                            patch: 1,
                            suffix: None,
                        }),
                        bump: BumpLevel::Patch,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::new(),
                        changed_paths: vec!["packages/iceberg/src/mod.py".to_string()],
                        selected: true,
                        selection_reason: "changed since latest tag".to_string(),
                        baseline: Default::default(),
                        release_tag: String::new(),
                        required_dependency_changes: Vec::new(),
                    },
                ],
            },
        }
    }

    fn sample_finalize_analysis() -> ReleaseAnalysis {
        let mut analysis = sample_release_set_analysis(false);
        analysis.current_version = Version {
            major: 0,
            minor: 8,
            patch: 1,
            suffix: Some(Suffix::Pre(PreRelease::Beta(5))),
        };
        analysis.next_version = None;
        analysis.package_plan.packages[0].current_version = analysis.current_version.clone();
        analysis.package_plan.packages[1].current_version = Version {
            major: 0,
            minor: 3,
            patch: 1,
            suffix: Some(Suffix::Pre(PreRelease::Beta(1))),
        };
        analysis.package_plan.packages[1].selected = false;
        analysis.package_plan.packages[1].next_version = None;
        analysis.package_plan.packages.push(PackageReleaseAnalysis {
            name: "phlo-sql".to_string(),
            root: "packages/sql".to_string(),
            current_version: Version {
                major: 0,
                minor: 2,
                patch: 0,
                suffix: None,
            },
            next_version: None,
            bump: BumpLevel::None,
            changelog: PendingChangelog {
                sections: BTreeMap::new(),
                contributors: Vec::new(),
            },
            version_files: Vec::new(),
            commits: Vec::new(),
            changed_paths: Vec::new(),
            selected: false,
            selection_reason: "test".to_string(),
            baseline: Default::default(),
            release_tag: String::new(),
            required_dependency_changes: Vec::new(),
        });
        analysis
    }

    fn run(repo_path: &std::path::Path, args: &[&str]) {
        let status = Command::new(args[0])
            .args(&args[1..])
            .current_dir(repo_path)
            .status()
            .expect("command should run");
        assert!(status.success(), "command failed: {args:?}");
    }

    fn git(repo_path: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
            .env("GIT_CONFIG_VALUE_0", "false")
            .env("GIT_CONFIG_KEY_1", "tag.gpgsign")
            .env("GIT_CONFIG_VALUE_1", "false")
            .env("GIT_EDITOR", "true")
            .current_dir(repo_path)
            .status()
            .expect("git should run");
        assert!(status.success(), "git failed: {args:?}");
    }
}
