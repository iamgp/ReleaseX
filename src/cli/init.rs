use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use toml::Value;

use crate::{
    cli::Cli,
    config::{ChangelogConfig, Config, GitHubConfig, VersionFileConfig},
    git::GitRepository,
    github, progress,
};

pub fn run(cli: &Cli) -> Result<()> {
    if cli.config.exists() {
        bail!("config already exists at {}", cli.config.display());
    }

    let repo = GitRepository::discover(".").ok();
    let repo_root = repo
        .as_ref()
        .map(|repo| repo.path())
        .unwrap_or(Path::new("."));

    let config = if cli.dry_run {
        build_config(repo.as_ref(), repo_root)
    } else {
        let sp = progress::spinner("Detecting project layout…");
        let result = build_config(repo.as_ref(), repo_root);
        sp.finish_and_clear();
        result
    };
    let rendered = toml::to_string_pretty(&config).context("failed to render config")?;

    if cli.dry_run {
        println!("Would create {}", cli.config.display());
        println!();
        print!("{rendered}");
        return Ok(());
    }

    fs::write(&cli.config, rendered)
        .with_context(|| format!("failed to write {}", cli.config.display()))?;

    println!("Created {}", cli.config.display());
    Ok(())
}

fn build_config(repo: Option<&GitRepository>, repo_root: &Path) -> Config {
    let branch = repo
        .and_then(|repo| repo.current_branch().ok())
        .filter(|branch| !branch.trim().is_empty())
        .unwrap_or_else(|| "main".to_string());
    let version_files = detect_version_files(repo_root);
    let initial_version =
        detect_initial_version(repo_root, &version_files).unwrap_or_else(|| "0.1.0".to_string());
    let mut github_config = GitHubConfig::default();

    if let Some(repo) = repo
        && let Ok(repo_ref) = github::detect_repo(repo, &github_config)
    {
        github_config.owner = Some(repo_ref.owner);
        github_config.repo = Some(repo_ref.name);
    }

    Config {
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
    ChangelogConfig { sections }
}

fn detect_version_files(repo_root: &Path) -> Vec<VersionFileConfig> {
    let mut version_files = Vec::new();

    let pyproject_path = repo_root.join("pyproject.toml");
    if pyproject_path.exists() {
        version_files.push(VersionFileConfig {
            path: "pyproject.toml".to_string(),
            key: Some("project.version".to_string()),
            pattern: None,
        });
    }

    let setup_cfg_path = repo_root.join("setup.cfg");
    if setup_cfg_path.exists() {
        version_files.push(VersionFileConfig {
            path: "setup.cfg".to_string(),
            key: Some("metadata.version".to_string()),
            pattern: None,
        });
    }

    version_files.extend(detect_python_version_files(repo_root));

    if version_files.is_empty() {
        version_files.push(VersionFileConfig {
            path: "pyproject.toml".to_string(),
            key: Some("project.version".to_string()),
            pattern: None,
        });
    }

    version_files
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

fn detect_python_version_files(repo_root: &Path) -> Vec<VersionFileConfig> {
    let mut candidates = Vec::new();

    for relative in [PathBuf::from("src"), PathBuf::from(".")] {
        let dir = repo_root.join(&relative);
        if !dir.is_dir() {
            continue;
        }

        scan_python_dir(repo_root, &dir, &mut candidates);
    }

    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path == right.path);
    candidates
}

fn scan_python_dir(repo_root: &Path, dir: &Path, candidates: &mut Vec<VersionFileConfig>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".git" | "target" | ".venv" | "venv" | "__pycache__")
        ) {
            continue;
        }

        if path.is_dir() {
            scan_python_dir(repo_root, &path, candidates);
            continue;
        }

        if path.file_name().and_then(|name| name.to_str()) != Some("__init__.py") {
            continue;
        }

        let Some(pattern) = detect_python_pattern(&path) else {
            continue;
        };
        let Ok(relative_path) = path.strip_prefix(repo_root) else {
            continue;
        };

        candidates.push(VersionFileConfig {
            path: relative_path.to_string_lossy().replace('\\', "/"),
            key: None,
            pattern: Some(pattern),
        });
    }
}

fn detect_python_pattern(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;

    for line in contents.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("__version__") {
            continue;
        }

        let (prefix, raw_value) = trimmed.split_once('=')?;
        let value = raw_value.trim();
        if value.len() < 2 {
            continue;
        }

        let quote = value.chars().next()?;
        if (quote != '"' && quote != '\'') || !value.ends_with(quote) {
            continue;
        }

        return Some(format!("{}= {}{{version}}{}", prefix, quote, quote));
    }

    None
}
