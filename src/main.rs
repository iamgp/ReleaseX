mod analysis;
mod changelog;
mod cli;
mod config;
mod conventional_commits;
mod git;
mod github;
mod publish;
mod version;
mod version_files;

use anyhow::Result;

fn main() -> Result<()> {
    cli::run()
}
