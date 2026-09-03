use anyhow::{Context, Result};

use crate::{
    analysis,
    cli::{Cli, CurrentVersionArgs},
    config::Config,
    git::GitRepository,
};

pub fn run(cli: &Cli, args: &CurrentVersionArgs) -> Result<()> {
    let repo = GitRepository::discover(".").context("failed to inspect git repository")?;
    let config = Config::load(&cli.config_path())?;
    config.validate()?;

    if config.monorepo.enabled {
        return run_monorepo(&repo, &config, args);
    }

    let version = if config.promotion.enabled {
        crate::promotion::resolve_active_baseline(&repo, &config)?
            .0
            .to_string()
    } else {
        analysis::read_current_version(repo.path(), &config.version_files)?
            .or_else(|| {
                config
                    .versioning
                    .initial_version
                    .parse::<crate::version::Version>()
                    .ok()
                    .map(|version| version.to_string())
            })
            .context("no version found in version files")?
    };

    if args.json {
        println!("{}", serde_json::json!({ "version": version }));
    } else {
        println!("{version}");
    }
    crate::promotion::emit_github_output("version", &version)?;
    Ok(())
}

fn run_monorepo(repo: &GitRepository, config: &Config, args: &CurrentVersionArgs) -> Result<()> {
    let analysis = analysis::analyze(repo, config)?;
    if args.json {
        let packages: std::collections::BTreeMap<String, String> = analysis
            .package_plan
            .packages
            .iter()
            .map(|package| (package.name.clone(), package.current_version.to_string()))
            .collect();
        println!("{}", serde_json::json!({ "packages": packages }));
    } else {
        for package in &analysis.package_plan.packages {
            println!("{} {}", package.name, package.current_version);
        }
    }
    Ok(())
}
