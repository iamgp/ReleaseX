pub mod init;
pub mod release;
pub mod status;
pub mod validate;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "pyrls",
    version,
    about = "Automated Python release tooling for Git repositories"
)]
pub struct Cli {
    #[arg(long, global = true, value_name = "PATH", default_value = "pyrls.toml")]
    pub config: PathBuf,
    #[arg(long, global = true)]
    pub dry_run: bool,
    #[arg(long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Init,
    Status,
    Validate,
    Release(ReleaseCommand),
}

#[derive(Debug, Args)]
pub struct ReleaseCommand {
    #[command(subcommand)]
    pub command: ReleaseSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ReleaseSubcommand {
    Pr,
    Tag,
    Publish,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Command::Init => init::run(&cli),
        Command::Status => status::run(&cli),
        Command::Validate => validate::run(&cli),
        Command::Release(cmd) => release::run(&cli, cmd),
    }
}
