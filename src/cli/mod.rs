pub mod healthcheck;
pub mod init;
pub mod release;
pub mod status;
pub mod validate;
pub mod workspace;

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
    Status(StatusArgs),
    Validate,
    Healthcheck(HealthcheckArgs),
    Release(ReleaseCommand),
    Workspace,
}

#[derive(Debug, Args)]
pub struct HealthcheckArgs {
    #[arg(long, value_name = "CATEGORY")]
    pub only: Option<String>,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(long)]
    pub short: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long, value_name = "TAG")]
    pub since: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReleaseCommand {
    #[command(subcommand)]
    pub command: ReleaseSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ReleaseSubcommand {
    Pr(PreReleaseArgs),
    Tag(PreReleaseArgs),
    Publish,
}

#[derive(Debug, Args)]
pub struct PreReleaseArgs {
    /// Create a pre-release version (alpha, beta, or rc)
    #[arg(long, value_name = "KIND", conflicts_with = "finalize")]
    pub pre_release: Option<PreReleaseKind>,

    /// Strip the pre-release suffix to produce a final release
    #[arg(long, conflicts_with = "pre_release")]
    pub finalize: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum PreReleaseKind {
    Alpha,
    Beta,
    Rc,
    Post,
    Dev,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    if cli.no_color {
        unsafe { std::env::set_var("NO_COLOR", "1") };
    }

    if cli.verbose {
        unsafe { std::env::set_var("PYRLS_VERBOSE", "1") };
        eprintln!("[pyrls] verbose mode enabled");
    }

    match &cli.command {
        Command::Init => init::run(&cli),
        Command::Status(args) => status::run(&cli, args),
        Command::Validate => validate::run(&cli),
        Command::Healthcheck(args) => healthcheck::run(&cli, args),
        Command::Release(cmd) => release::run(&cli, cmd),
        Command::Workspace => workspace::run(&cli),
    }
}
