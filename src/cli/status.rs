use anyhow::{Context, Result};

use crate::{analysis, cli::Cli, config::Config, git::GitRepository};

pub fn run(cli: &Cli) -> Result<()> {
    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config = Config::load(&cli.config)?;
    let analysis = analysis::analyze(&repo, &config)?;

    println!("Repository: {}", repo.path().display());
    println!("Branch: {}", repo.current_branch()?);
    println!("Config: {}", cli.config.display());
    println!(
        "Last tag: {}",
        repo.latest_tag()?.unwrap_or_else(|| "none".to_string())
    );
    println!("Current version: {}", analysis.current_version);
    println!("Commit count: {}", analysis.commits.len());
    println!("Proposed bump: {}", analysis.bump.as_str());
    println!(
        "Next version: {}",
        analysis
            .next_version
            .as_ref()
            .map(ToString::to_string)
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
            package.current_version,
            package
                .next_version
                .as_ref()
                .map(ToString::to_string)
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
