use anyhow::{Context, Result};

use crate::{
    analysis,
    cli::{Cli, ReleaseCommand, ReleaseSubcommand},
    config::Config,
    git::GitRepository,
    github, publish,
};

pub fn run(cli: &Cli, command: &ReleaseCommand) -> Result<()> {
    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config = Config::load(&cli.config)?;
    let analysis = analysis::analyze(&repo, &config)?;

    if config.monorepo.enabled && !cli.dry_run {
        anyhow::bail!(
            "monorepo execution is currently limited to dry-run planning; use --dry-run to inspect unified or per-package release sets"
        );
    }

    match command.command {
        ReleaseSubcommand::Pr => {
            if cli.dry_run {
                github::print_release_pr_dry_run(&repo, &config, &analysis)?;
            } else {
                github::execute_release_pr(&repo, &config, &analysis)?;
            }
        }
        ReleaseSubcommand::Tag => {
            if cli.dry_run {
                github::print_release_tag_dry_run(&repo, &config, &analysis)?;
            } else {
                github::execute_release_tag(&repo, &config, &analysis)?;
            }
        }
        ReleaseSubcommand::Publish => {
            if cli.dry_run {
                publish::print_dry_run(repo.path(), &config)?;
            } else {
                publish::execute(repo.path(), &config)?;
            }
        }
    }

    Ok(())
}
