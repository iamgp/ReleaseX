use anyhow::Result;

use crate::{cli::Cli, config::Config};

pub fn run(cli: &Cli) -> Result<()> {
    let config = Config::load(&cli.config_path())?;
    config.validate()?;

    println!(
        "Config is valid: release branch={}, version files={}",
        config.release.branch,
        config.version_files.len()
    );

    Ok(())
}
