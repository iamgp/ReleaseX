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
    Pr(PreReleaseArgs),
    Plan(PlanArgs),
    Prepare(PrepareArgs),
    Tag(PreReleaseArgs),
    Publish(PublishArgs),
    PreviewPr(PreviewPrArgs),
    #[command(alias = "finalize", alias = "promote")]
    Release(ReleaseArgs),
    ForwardPort(ForwardPortArgs),
}

#[derive(Debug, Args)]
pub struct PlanArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct PrepareArgs {
    #[arg(long)]
    pub check: bool,
    #[command(flatten)]
    pub release: PreReleaseArgs,
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

    /// Use an exact stable version instead of deriving a conventional-commit bump
    #[arg(
        long,
        value_name = "VERSION",
        conflicts_with_all = ["pre_release", "finalize"]
    )]
    pub next_version: Option<String>,

    /// Create a pre-release version (alpha, beta, or rc)
    #[arg(long, value_name = "KIND", conflicts_with = "finalize")]
    pub pre_release: Option<PreReleaseKind>,

    /// Strip the pre-release suffix to produce a final release
    #[arg(long, conflicts_with = "pre_release")]
    pub finalize: bool,
}

#[derive(Debug, Args)]
pub struct PublishArgs {
    /// Skip packages that are already published to PyPI/crates.io
    #[arg(long)]
    pub skip_published: bool,
}

#[derive(Debug, Args)]
pub struct PreviewPrArgs {
    /// Existing promotion PR number. When omitted, relx finds or creates the
    /// development → production PR.
    #[arg(long)]
    pub pr: Option<u64>,
    /// Promotion head branch (defaults to [promotion].development_branch).
    /// Use a `hotfix/*` branch to preview a hotfix PR.
    #[arg(long)]
    pub head: Option<String>,
    /// Production base branch (defaults to the configured production branch).
    #[arg(long)]
    pub base: Option<String>,
    /// Emit machine-readable outputs as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ReleaseArgs {
    /// Merged promotion PR number. When omitted, relx looks up the most
    /// recently merged development → production PR.
    #[arg(long)]
    pub pr: Option<u64>,
    /// Emit machine-readable outputs as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ForwardPortArgs {}

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
