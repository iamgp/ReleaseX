pub mod generate_ci;
pub mod healthcheck;
pub mod init;
pub mod release;
pub mod snapshot;
pub mod status;
pub mod validate;
pub mod workspace;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

pub const DEFAULT_CONFIG: &str = "relx.toml";

#[derive(Debug, Parser)]
#[command(
    name = "relx",
    version,
    about = "Automated release tooling for Python, Rust, and Go repositories"
)]
pub struct Cli {
    #[arg(long, global = true, value_name = "PATH", default_value = DEFAULT_CONFIG)]
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
    GenerateCi(GenerateCiArgs),
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
    #[arg(long)]
    pub channel: bool,
    #[arg(long, value_name = "TAG")]
    pub since: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReleaseCommand {
    #[arg(long)]
    pub snapshot: bool,
    #[command(subcommand)]
    pub command: ReleaseSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ReleaseSubcommand {
    Pending(ReleasePendingArgs),
    Pr(PreReleaseArgs),
    Tag(PreReleaseArgs),
    Publish,
}

#[derive(Debug, Args)]
pub struct ReleasePendingArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct GenerateCiArgs {
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,
}

#[derive(Debug, Args)]
pub struct PreReleaseArgs {
    #[arg(long, value_name = "CHANNEL")]
    pub channel: Option<String>,

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

impl Cli {
    pub fn config_path(&self) -> PathBuf {
        self.config.clone()
    }

    pub fn config_path_for_init_conflict(&self) -> Option<PathBuf> {
        if self.config.exists() {
            return Some(self.config.clone());
        }

        None
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    if cli.no_color {
        unsafe { std::env::set_var("NO_COLOR", "1") };
    }

    if cli.verbose {
        unsafe { std::env::set_var("RELX_VERBOSE", "1") };
        eprintln!("[relx] verbose mode enabled");
    }

    match &cli.command {
        Command::Init => init::run(&cli),
        Command::Status(args) => status::run(&cli, args),
        Command::Validate => validate::run(&cli),
        Command::Healthcheck(args) => healthcheck::run(&cli, args),
        Command::Release(cmd) => release::run(&cli, cmd),
        Command::Workspace => workspace::run(&cli),
        Command::GenerateCi(args) => generate_ci::run(&cli, args),
    }
}
