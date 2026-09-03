use anyhow::{Context, Result};

use crate::{
    cli::{Cli, LintArgs},
    config::Config,
    conventional_commits::ConventionalCommit,
    git::GitRepository,
};

pub fn run(cli: &Cli, args: &LintArgs) -> Result<()> {
    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config = Config::load(&cli.config_path())?;
    config.validate()?;

    let commits = match &args.since {
        Some(tag) => repo.commits_since_tag(tag)?,
        None => repo.commits_since_latest_tag()?,
    };

    let mut offenders = Vec::new();
    for commit in &commits {
        if ConventionalCommit::parse_message(&commit.message).is_err() {
            offenders.push(commit);
        }
    }

    if args.json {
        let payload = serde_json::json!({
            "checked": commits.len(),
            "valid": commits.len() - offenders.len(),
            "invalid": offenders.iter().map(|commit| {
                serde_json::json!({
                    "sha": commit.id,
                    "subject": commit.message.lines().next().unwrap_or_default(),
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if offenders.is_empty() {
        println!(
            "All {} commit(s) follow Conventional Commits",
            commits.len()
        );
    } else {
        println!(
            "{} of {} commit(s) are not Conventional Commits:",
            offenders.len(),
            commits.len()
        );
        for commit in &offenders {
            let subject = commit.message.lines().next().unwrap_or_default();
            println!("  {} {subject}", &commit.id[..7.min(commit.id.len())]);
        }
    }

    if offenders.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "{} commit(s) do not follow Conventional Commits",
            offenders.len()
        )
    }
}
