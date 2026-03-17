use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use toml::Value;

use crate::{
    cli::Cli,
    config::{ChangelogConfig, Config, Ecosystem, GitHubConfig, VersionFileConfig},
    ecosystem,
    git::GitRepository,
    github, progress,
};

pub fn run(cli: &Cli) -> Result<()> {
    if let Some(path) = cli.config_path_for_init_conflict() {
        bail!("config already exists at {}", path.display());
    }

    let repo = GitRepository::discover(".").ok();
    let repo_root = repo
        .as_ref()
        .map(|repo| repo.path())
        .unwrap_or(Path::new("."));

    let plan = if cli.dry_run {
        build_config(repo.as_ref(), repo_root)
    } else {
        let sp = progress::spinner("Detecting project layout…");
        let result = build_config(repo.as_ref(), repo_root);
        sp.finish_and_clear();
        result
    };
    let rendered = toml::to_string_pretty(&plan.config).context("failed to render config")?;

    if cli.dry_run {
        println!("Would create {}", cli.config.display());
        for (path, _) in &plan.extra_files {
            println!("Would create {}", path.display());
        }
        println!();
        print!("{rendered}");
        return Ok(());
    }

    for (path, contents) in &plan.extra_files {
        fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    }

    fs::write(&cli.config, rendered)
        .with_context(|| format!("failed to write {}", cli.config.display()))?;

    println!("Created {}", cli.config.display());
    Ok(())
}

struct InitPlan {
    config: Config,
    extra_files: Vec<(PathBuf, String)>,
}

fn build_config(repo: Option<&GitRepository>, repo_root: &Path) -> InitPlan {
    let branch = repo
        .and_then(|repo| repo.current_branch().ok())
        .filter(|branch| !branch.trim().is_empty())
        .unwrap_or_else(|| "main".to_string());
    let detected_ecosystem = ecosystem::detect(repo_root, None);
    let mut version_files = ecosystem::discover_version_files(repo_root, detected_ecosystem);
    let mut extra_files = Vec::new();
    if detected_ecosystem == Ecosystem::Go && version_files.is_empty() {
        version_files.push(VersionFileConfig {
            path: "VERSION".to_string(),
            key: None,
            pattern: Some("{version}".to_string()),
        });
        extra_files.push((repo_root.join("VERSION"), "0.1.0\n".to_string()));
    }
    let initial_version =
        detect_initial_version(repo_root, &version_files).unwrap_or_else(|| "0.1.0".to_string());
    let mut github_config = GitHubConfig::default();

    if let Some(repo) = repo
        && let Ok(repo_ref) = github::detect_repo(repo, &github_config)
    {
        github_config.owner = Some(repo_ref.owner);
        github_config.repo = Some(repo_ref.name);
    }

    InitPlan {
        config: Config {
            project: crate::config::ProjectConfig {
                ecosystem: Some(detected_ecosystem),
            },
            release: crate::config::ReleaseConfig {
                branch,
                ..Default::default()
            },
            versioning: crate::config::VersioningConfig {
                initial_version,
                ..Default::default()
            },
            monorepo: Default::default(),
            version_files,
            changelog: default_changelog_config(),
            publish: Default::default(),
            github: github_config,
            workspace: Default::default(),
            ci: Default::default(),
            channels: Vec::new(),
        },
        extra_files,
    }
}

fn default_changelog_config() -> ChangelogConfig {
    let sections = BTreeMap::from([
        ("docs".to_string(), Value::Boolean(false)),
        ("feat".to_string(), Value::String("Added".to_string())),
        ("fix".to_string(), Value::String("Fixed".to_string())),
        ("perf".to_string(), Value::String("Changed".to_string())),
        ("refactor".to_string(), Value::String("Changed".to_string())),
    ]);
    ChangelogConfig {
        sections,
        ..Default::default()
    }
}

fn detect_initial_version(repo_root: &Path, version_files: &[VersionFileConfig]) -> Option<String> {
    for version_file in version_files {
        let path = repo_root.join(&version_file.path);
        if !path.exists() {
            continue;
        }

        let value = if let Some(key) = &version_file.key {
            crate::version_files::read_key(&path, key).ok().flatten()
        } else if let Some(pattern) = &version_file.pattern {
            crate::version_files::read_pattern(&path, pattern)
                .ok()
                .flatten()
        } else {
            None
        };

        if value.is_some() {
            return value;
        }
    }

    None
}
