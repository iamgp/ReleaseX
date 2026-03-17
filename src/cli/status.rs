use anyhow::{Context, Result};
use console::style;
use std::env;

use crate::{
    analysis::{self, PackageReleaseAnalysis, ReleaseAnalysis},
    channels,
    cli::{Cli, StatusArgs},
    config::{Config, Ecosystem},
    conventional_commits::ConventionalCommit,
    cratesio, ecosystem,
    git::GitRepository,
    github::{self, GitHubClient, PullRequest},
    progress, pypi,
};

fn display_version(config: &Config, version: &crate::version::Version) -> String {
    ecosystem::format_version(version, ecosystem::config_ecosystem(config))
}

pub fn run(cli: &Cli, args: &StatusArgs) -> Result<()> {
    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config_path = cli.config_path();
    let config = Config::load(&config_path)?;

    let mut analysis = if cli.dry_run {
        match &args.since {
            Some(tag) => analysis::analyze_since(&repo, &config, tag)?,
            None => analysis::analyze(&repo, &config)?,
        }
    } else {
        let sp = progress::spinner("Analyzing commits…");
        let result = match &args.since {
            Some(tag) => analysis::analyze_since(&repo, &config, tag),
            None => analysis::analyze(&repo, &config),
        };
        sp.finish_and_clear();
        result?
    };
    let branch = repo
        .current_branch()
        .unwrap_or_else(|_| "unknown".to_string());
    let resolved_channel =
        channels::apply_channel_to_analysis(&repo, &config, &mut analysis, &branch, None)?;

    if args.channel {
        print_channel(&branch, resolved_channel.as_ref());
    } else if args.json {
        print_json(
            &repo,
            &config,
            &analysis,
            &branch,
            resolved_channel.as_ref(),
        )?;
    } else if args.short {
        print_short(&config, &analysis);
    } else if cli.dry_run && !args.json && !args.short {
        print_legacy(&config, cli, &repo, &analysis)?;
    } else {
        print_dashboard(&repo, &config, &analysis, args, &branch)?;
    }

    Ok(())
}

fn print_channel(branch: &str, channel: Option<&channels::ResolvedChannel>) {
    match channel {
        Some(channel) => {
            let prerelease = channel.prerelease.as_deref().unwrap_or("stable");
            let publish = if channel.publish {
                "publish"
            } else {
                "no-publish"
            };
            if let Some(range) = &channel.version_range {
                println!("{} {} {} {}", branch, prerelease, publish, range);
            } else {
                println!("{} {} {}", branch, prerelease, publish);
            }
        }
        None => println!("{} unconfigured", branch),
    }
}

fn print_legacy(
    config: &Config,
    cli: &Cli,
    repo: &GitRepository,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    println!("Repository: {}", repo.path().display());
    println!("Branch: {}", repo.current_branch()?);
    println!("Config: {}", cli.config_path().display());
    println!(
        "Last tag: {}",
        repo.latest_tag()?.unwrap_or_else(|| "none".to_string())
    );
    println!(
        "Current version: {}",
        display_version(&config, &analysis.current_version)
    );
    println!("Commit count: {}", analysis.commits.len());
    println!("Proposed bump: {}", analysis.bump.as_str());
    println!(
        "Next version: {}",
        analysis
            .next_version
            .as_ref()
            .map(|version| display_version(&config, version))
            .unwrap_or_else(|| "unchanged".to_string())
    );
    println!("Release mode: {}", analysis.package_plan.release_mode);
    println!(
        "Package discovery: {}",
        analysis.package_plan.discovery_source
    );

    let selected_packages = analysis.package_plan.selected_packages();
    println!(
        "Selected package set: {} package(s)",
        selected_packages.len()
    );
    for package in &analysis.package_plan.packages {
        println!(
            "  - {} [{}] current={} next={} bump={} reason={}",
            package.name,
            package.root,
            display_version(config, &package.current_version),
            package
                .next_version
                .as_ref()
                .map(|version| display_version(&config, version))
                .unwrap_or_else(|| "unchanged".to_string()),
            package.bump.as_str(),
            package.selection_reason
        );
        if !package.changed_paths.is_empty() {
            println!("    changed files: {}", package.changed_paths.join(", "));
        }
    }

    if analysis.changelog.is_empty() {
        println!("Pending changelog: none");
    } else {
        println!("Pending changelog:");
        for (section, entries) in &analysis.changelog.sections {
            println!("  {section}:");
            for entry in entries {
                println!("    - {entry}");
            }
        }
    }

    if cli.dry_run {
        println!("Dry run: no files changed");
    }

    Ok(())
}

fn print_json(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
    branch: &str,
    channel: Option<&channels::ResolvedChannel>,
) -> Result<()> {
    let github_status = fetch_github_status(repo, config, analysis).ok().flatten();
    let packages: Vec<serde_json::Value> = analysis
        .package_plan
        .packages
        .iter()
        .map(|pkg| {
            let published = pypi_version_for_package(repo, pkg);
            serde_json::json!({
                "name": pkg.name,
                "root": pkg.root,
                "current_version": display_version(config, &pkg.current_version),
                "next_version": pkg.next_version.as_ref().map(|v| display_version(config, v)),
                "bump": pkg.bump.as_str(),
                "selected": pkg.selected,
                "published_version": published.map(|(_, v)| display_version(config, &v)),
                "commit_count": pkg.commits.len(),
                "commits": pkg.commits.iter().map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "message": c.message,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    let output = serde_json::json!({
        "branch": branch,
        "channel": channel.map(|c| serde_json::json!({
            "branch": c.branch,
            "publish": c.publish,
            "prerelease": c.prerelease,
            "version_range": c.version_range,
        })),
        "last_tag": repo.latest_tag().unwrap_or(None),
        "current_version": display_version(config, &analysis.current_version),
        "next_version": analysis.next_version.as_ref().map(|v| display_version(config, v)),
        "bump": analysis.bump.as_str(),
        "commit_count": analysis.commits.len(),
        "release_mode": analysis.package_plan.release_mode,
        "github": github_status.as_ref().map(|status| serde_json::json!({
            "number": status.pr.number,
            "title": status.pr.title,
            "url": status.pr.html_url,
            "approvals": status.approvals,
            "checks": status.check_state,
        })),
        "packages": packages,
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_short(config: &Config, analysis: &ReleaseAnalysis) {
    for pkg in &analysis.package_plan.packages {
        let version_info = match &pkg.next_version {
            Some(next) => format!(
                "{} → {} ({})",
                display_version(config, &pkg.current_version),
                display_version(config, next),
                pkg.bump.as_str()
            ),
            None => format!(
                "{} → no change",
                display_version(config, &pkg.current_version)
            ),
        };

        let commit_info = if pkg.commits.is_empty() {
            String::new()
        } else {
            format!("   {} commits", pkg.commits.len())
        };

        println!(
            " {:<12} {}{}",
            style(&pkg.name).bold(),
            version_info,
            commit_info
        );
    }
}

fn print_dashboard(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
    args: &StatusArgs,
    branch: &str,
) -> Result<()> {
    println!();
    println!("{}", style("relx status").bold());
    println!();

    let last_tag = repo.latest_tag().unwrap_or(None);
    let github_status = fetch_github_status(repo, config, analysis).ok().flatten();

    for pkg in &analysis.package_plan.packages {
        print_package_section(pkg, branch, &last_tag, config, args);
        println!();
    }

    if let Some(status) = github_status {
        println!(
            " {} #{} open · {} approval(s) · checks {}",
            style("Release PR").cyan().bold(),
            status.pr.number,
            status.approvals,
            status.check_state
        );
    } else {
        println!(" {} none open", style("Release PR").cyan().bold());
    }

    for pkg in &analysis.package_plan.packages {
        match pypi_version_for_package(repo, pkg) {
            Some((label, version)) => println!(
                " {} {} published",
                style(format!("{} {}", label, pkg.name)).cyan().bold(),
                display_version(config, &version)
            ),
            None => {
                let label = registry_label(repo, config);
                println!(
                    " {} {}",
                    style(format!("{} {}", label, pkg.name)).cyan().bold(),
                    style("not published or unavailable").dim()
                )
            }
        }
    }
    println!();

    Ok(())
}

fn print_package_section(
    pkg: &PackageReleaseAnalysis,
    branch: &str,
    last_tag: &Option<String>,
    config: &Config,
    _args: &StatusArgs,
) {
    println!(
        " {} {}",
        style("Package").cyan().bold(),
        style(&pkg.name).bold()
    );

    match &pkg.next_version {
        Some(next) => {
            println!(
                " {} {} → {} ({})",
                style("Version").cyan().bold(),
                display_version(config, &pkg.current_version),
                style(display_version(config, next)).green(),
                style(pkg.bump.as_str()).yellow()
            );
        }
        None => {
            println!(
                " {} {} (no change)",
                style("Version").cyan().bold(),
                display_version(config, &pkg.current_version)
            );
        }
    }

    println!(" {} {}", style("Branch").cyan().bold(), branch);

    match last_tag {
        Some(tag) => println!(" {} {}", style("Last release").cyan().bold(), tag),
        None => println!(
            " {} {}",
            style("Last release").cyan().bold(),
            style("none").dim()
        ),
    }

    if pkg.commits.is_empty() {
        println!();
        println!(" {}", style("No unreleased commits").dim());
        return;
    }

    println!();
    println!(
        " {} ({})",
        style("Unreleased commits").cyan().bold(),
        pkg.commits.len()
    );

    // Parse conventional commits for type info
    let rows: Vec<(String, String, String)> = pkg
        .commits
        .iter()
        .map(|commit| {
            let commit_type = ConventionalCommit::parse_message(&commit.message)
                .map(|cc| cc.commit_type)
                .unwrap_or_else(|_| "other".to_string());
            let first_line = commit.message.lines().next().unwrap_or(&commit.message);
            let description = ConventionalCommit::parse_message(&commit.message)
                .map(|cc| cc.description)
                .unwrap_or_else(|_| first_line.to_string());
            let short_hash = if commit.id.len() >= 7 {
                &commit.id[..7]
            } else {
                &commit.id
            };
            (commit_type, description, short_hash.to_string())
        })
        .collect();

    // Calculate column widths
    let type_width = rows
        .iter()
        .map(|(t, _, _)| t.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let msg_width = rows
        .iter()
        .map(|(_, m, _)| m.len())
        .max()
        .unwrap_or(7)
        .clamp(7, 50);
    let hash_width = 7;

    // Table header
    println!(
        " ┌{:─<tw$}┬{:─<mw$}┬{:─<hw$}┐",
        "",
        "",
        "",
        tw = type_width + 2,
        mw = msg_width + 2,
        hw = hash_width + 2
    );
    println!(
        " │ {:<tw$} │ {:<mw$} │ {:<hw$} │",
        "Type",
        "Message",
        "Hash",
        tw = type_width,
        mw = msg_width,
        hw = hash_width
    );
    println!(
        " ├{:─<tw$}┼{:─<mw$}┼{:─<hw$}┤",
        "",
        "",
        "",
        tw = type_width + 2,
        mw = msg_width + 2,
        hw = hash_width + 2
    );

    for (commit_type, description, hash) in &rows {
        let truncated_msg = if description.len() > msg_width {
            format!("{}…", &description[..msg_width - 1])
        } else {
            description.clone()
        };
        println!(
            " │ {:<tw$} │ {:<mw$} │ {:<hw$} │",
            commit_type,
            truncated_msg,
            hash,
            tw = type_width,
            mw = msg_width,
            hw = hash_width
        );
    }

    println!(
        " └{:─<tw$}┴{:─<mw$}┴{:─<hw$}┘",
        "",
        "",
        "",
        tw = type_width + 2,
        mw = msg_width + 2,
        hw = hash_width + 2
    );
}

struct StatusPr {
    pr: PullRequest,
    approvals: usize,
    check_state: String,
}

fn fetch_github_status(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<Option<StatusPr>> {
    let repo_ref = github::detect_repo(repo, &config.github)?;
    let token = env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    let client = GitHubClient::new(&config.github.api_base, &token, repo_ref)?;
    let current_branch = repo.current_branch()?;
    let plan = github::build_release_pr_plan(config, analysis, &current_branch)?;
    let Some(pr) = client.find_open_pr(&plan.branch, &plan.base)? else {
        return Ok(None);
    };
    let approvals = client
        .list_reviews(pr.number)?
        .into_iter()
        .filter(|review| review.state == "APPROVED")
        .count();
    let check_state = match pr.head.as_ref() {
        Some(head) => client
            .combined_status(&head.sha)
            .map(|status| status.state)
            .unwrap_or_else(|_| "unknown".to_string()),
        None => "unknown".to_string(),
    };
    Ok(Some(StatusPr {
        pr,
        approvals,
        check_state,
    }))
}

fn pypi_version_for_package(
    repo: &GitRepository,
    pkg: &PackageReleaseAnalysis,
) -> Option<(String, crate::version::Version)> {
    let ecosystem = ecosystem::detect(repo.path(), None);
    let project_name = if pkg.root == "." {
        analysis::detect_project_name(repo.path(), ".").unwrap_or_else(|| pkg.name.clone())
    } else {
        analysis::detect_project_name(repo.path(), &pkg.root).unwrap_or_else(|| pkg.name.clone())
    };
    match ecosystem {
        Ecosystem::Python => pypi::latest_published_version(&project_name)
            .ok()
            .flatten()
            .map(|version| ("PyPI".to_string(), version)),
        Ecosystem::Rust => cratesio::latest_published_version(&project_name)
            .ok()
            .flatten()
            .map(|version| ("crates.io".to_string(), version)),
        Ecosystem::Go => None,
    }
}

fn registry_label(repo: &GitRepository, config: &Config) -> &'static str {
    match ecosystem::detect(repo.path(), Some(config)) {
        Ecosystem::Python => "PyPI",
        Ecosystem::Rust => "crates.io",
        Ecosystem::Go => "Registry",
    }
}
