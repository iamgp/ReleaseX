use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::{
    changelog::PendingChangelog,
    config::{Config, VersionFileConfig},
    conventional_commits::ConventionalCommit,
    git::{CommitSummary, GitRepository},
    version::{BumpLevel, Version},
    version_files,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAnalysis {
    pub current_version: Version,
    pub next_version: Option<Version>,
    pub bump: BumpLevel,
    pub commits: Vec<CommitSummary>,
    pub changelog: PendingChangelog,
    pub package_plan: PackagePlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePlan {
    pub release_mode: String,
    pub discovery_source: String,
    pub packages: Vec<PackageReleaseAnalysis>,
}

impl PackagePlan {
    pub fn selected_packages(&self) -> Vec<&PackageReleaseAnalysis> {
        self.packages
            .iter()
            .filter(|package| package.selected)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageReleaseAnalysis {
    pub name: String,
    pub root: String,
    pub current_version: Version,
    pub next_version: Option<Version>,
    pub bump: BumpLevel,
    pub changelog: PendingChangelog,
    pub version_files: Vec<VersionFileConfig>,
    pub commits: Vec<CommitSummary>,
    pub changed_paths: Vec<String>,
    pub selected: bool,
    pub selection_reason: String,
}

#[derive(Debug, Clone)]
struct PackageDefinition {
    name: String,
    root: String,
    version_files: Vec<VersionFileConfig>,
}

pub fn analyze(repo: &GitRepository, config: &Config) -> Result<ReleaseAnalysis> {
    config.validate()?;

    let commits = repo.commits_since_latest_tag()?;
    if config.monorepo.is_multi_package() {
        analyze_monorepo(repo, config, commits)
    } else {
        analyze_single_package(repo, config, commits)
    }
}

pub fn analyze_since(
    repo: &GitRepository,
    config: &Config,
    since_tag: &str,
) -> Result<ReleaseAnalysis> {
    config.validate()?;

    let commits = repo.commits_since_tag(since_tag)?;
    if config.monorepo.is_multi_package() {
        analyze_monorepo(repo, config, commits)
    } else {
        analyze_single_package(repo, config, commits)
    }
}

fn analyze_single_package(
    repo: &GitRepository,
    config: &Config,
    commits: Vec<CommitSummary>,
) -> Result<ReleaseAnalysis> {
    let current_version = match read_current_version(repo.path(), &config.version_files)? {
        Some(version) => version.parse()?,
        None => config.versioning.initial_version.parse()?,
    };

    let conventional_commits = commits
        .iter()
        .filter_map(|commit| ConventionalCommit::parse_message(&commit.message).ok())
        .collect::<Vec<_>>();
    let bump = BumpLevel::from_commits(&conventional_commits);
    let next_version = bump.apply(&current_version);
    let changelog = PendingChangelog::from_commits(config, &conventional_commits);

    Ok(ReleaseAnalysis {
        current_version: current_version.clone(),
        next_version: next_version.clone(),
        bump,
        commits: commits.clone(),
        changelog: changelog.clone(),
        package_plan: PackagePlan {
            release_mode: "single".to_string(),
            discovery_source: "top-level [[version_files]] configuration".to_string(),
            packages: vec![PackageReleaseAnalysis {
                name: package_name_from_repo_root(repo.path()),
                root: ".".to_string(),
                current_version,
                next_version,
                bump,
                changelog,
                version_files: config.version_files.clone(),
                commits,
                changed_paths: Vec::new(),
                selected: true,
                selection_reason: "single-package repository".to_string(),
            }],
        },
    })
}

fn analyze_monorepo(
    repo: &GitRepository,
    config: &Config,
    commits: Vec<CommitSummary>,
) -> Result<ReleaseAnalysis> {
    let (definitions, discovery_source) = discover_packages(repo.path(), config)?;
    if definitions.is_empty() {
        bail!("monorepo.enabled is true but no packages were discovered");
    }

    let mut packages = Vec::new();
    for definition in definitions {
        let package_commits = commits_for_package(&commits, &definition.root);
        let conventional_commits = package_commits
            .iter()
            .filter_map(|commit| ConventionalCommit::parse_message(&commit.message).ok())
            .collect::<Vec<_>>();
        let changed_paths = changed_paths_for_package(&package_commits, &definition.root);
        let current_version = match read_current_version(repo.path(), &definition.version_files)? {
            Some(version) => version.parse()?,
            None => config.versioning.initial_version.parse()?,
        };
        let bump = BumpLevel::from_commits(&conventional_commits);
        let next_version = bump.apply(&current_version);
        let selected = !changed_paths.is_empty() && next_version.is_some();

        packages.push(PackageReleaseAnalysis {
            name: definition.name,
            root: definition.root.clone(),
            current_version,
            next_version,
            bump,
            changelog: PendingChangelog::from_commits(config, &conventional_commits),
            version_files: definition.version_files,
            commits: package_commits,
            changed_paths,
            selected,
            selection_reason: if selected {
                "package files changed since the latest tag and produced a release bump".to_string()
            } else {
                "no releasable package changes detected since the latest tag".to_string()
            },
        });
    }

    apply_cascade_bumps(repo.path(), config, &mut packages);

    let selected_packages = packages.iter().filter(|package| package.selected);
    let aggregate_current_version = selected_packages
        .clone()
        .next()
        .map(|package| package.current_version.clone())
        .unwrap_or_else(|| {
            config
                .versioning
                .initial_version
                .parse()
                .expect("valid version")
        });
    let aggregate_bump = packages
        .iter()
        .filter(|package| package.selected)
        .fold(BumpLevel::None, |level, package| level.max(package.bump));
    let aggregate_next_version = aggregate_bump.apply(&aggregate_current_version);
    let aggregate_changelog = aggregate_changelog(&packages);

    Ok(ReleaseAnalysis {
        current_version: aggregate_current_version,
        next_version: aggregate_next_version,
        bump: aggregate_bump,
        commits,
        changelog: aggregate_changelog,
        package_plan: PackagePlan {
            release_mode: config.monorepo.release_mode.clone(),
            discovery_source,
            packages,
        },
    })
}

fn discover_packages(
    repo_root: &Path,
    config: &Config,
) -> Result<(Vec<PackageDefinition>, String)> {
    if !config.monorepo.packages.is_empty() {
        let packages = config
            .monorepo
            .packages
            .iter()
            .map(|package_root| load_package_definition(repo_root, package_root))
            .collect::<Result<Vec<_>>>()?;
        return Ok((packages, "[monorepo].packages".to_string()));
    }

    if let Some(uv_roots) = discover_uv_workspace(repo_root) {
        let packages = uv_roots
            .iter()
            .map(|package_root| load_package_definition(repo_root, package_root))
            .collect::<Result<Vec<_>>>()?;
        return Ok((
            packages,
            "uv workspace (tool.uv.workspace.members)".to_string(),
        ));
    }

    let mut package_roots = Vec::new();
    scan_for_package_roots(repo_root, repo_root, &mut package_roots);
    package_roots.sort();
    package_roots.dedup();

    let packages = package_roots
        .iter()
        .map(|package_root| load_package_definition(repo_root, package_root))
        .collect::<Result<Vec<_>>>()?;
    Ok((
        packages,
        "auto-discovered package pyproject.toml files".to_string(),
    ))
}

pub fn discover_uv_workspace(repo_root: &Path) -> Option<Vec<String>> {
    let pyproject_path = repo_root.join("pyproject.toml");
    let contents = fs::read_to_string(pyproject_path).ok()?;
    let parsed = contents.parse::<toml::Table>().ok()?;

    let members = parsed
        .get("tool")?
        .as_table()?
        .get("uv")?
        .as_table()?
        .get("workspace")?
        .as_table()?
        .get("members")?
        .as_array()?;

    let mut roots = Vec::new();
    for member in members {
        let pattern = member.as_str()?;
        if let Some(prefix) = pattern.strip_suffix("/*") {
            let parent_dir = repo_root.join(prefix);
            let entries = fs::read_dir(parent_dir).ok()?;
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let rel = format!(
                        "{}/{}",
                        prefix,
                        entry.file_name().to_string_lossy()
                    );
                    roots.push(rel);
                }
            }
        } else if let Some(prefix) = pattern.strip_suffix("/**") {
            let parent_dir = repo_root.join(prefix);
            let entries = fs::read_dir(parent_dir).ok()?;
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let rel = format!(
                        "{}/{}",
                        prefix,
                        entry.file_name().to_string_lossy()
                    );
                    roots.push(rel);
                }
            }
        } else {
            let dir = repo_root.join(pattern);
            if dir.is_dir() {
                roots.push(pattern.to_string());
            }
        }
    }

    roots.sort();
    roots.dedup();

    if roots.is_empty() {
        None
    } else {
        Some(roots)
    }
}

pub fn extract_dependency_names(repo_root: &Path, package_root: &str) -> Vec<String> {
    let pyproject_path = repo_root.join(package_root).join("pyproject.toml");
    let contents = match fs::read_to_string(pyproject_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let parsed = match contents.parse::<toml::Table>() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let Some(deps) = parsed
        .get("project")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("dependencies"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };

    deps.iter()
        .filter_map(|v| v.as_str())
        .map(|s| {
            let name = s
                .split(|c: char| matches!(c, '>' | '<' | '=' | '!' | '[' | ';' | ' ' | '~'))
                .next()
                .unwrap_or(s);
            name.to_string()
        })
        .collect()
}

pub fn apply_cascade_bumps(
    repo_root: &Path,
    config: &Config,
    packages: &mut Vec<PackageReleaseAnalysis>,
) {
    if !config.workspace.cascade_bumps {
        return;
    }

    let bumped_names: Vec<String> = packages
        .iter()
        .filter(|p| p.selected && p.next_version.is_some())
        .map(|p| p.name.clone())
        .collect();

    if bumped_names.is_empty() {
        return;
    }

    for i in 0..packages.len() {
        if packages[i].selected {
            continue;
        }

        let deps = extract_dependency_names(repo_root, &packages[i].root);
        let depends_on_bumped = deps.iter().any(|dep| bumped_names.contains(dep));

        if depends_on_bumped {
            let next = BumpLevel::Patch.apply(&packages[i].current_version);
            packages[i].next_version = next;
            packages[i].bump = BumpLevel::Patch;
            packages[i].selected = true;
            packages[i].selection_reason =
                "cascade bump: depends on a package with a version bump".to_string();
        }
    }
}

fn load_package_definition(repo_root: &Path, package_root: &str) -> Result<PackageDefinition> {
    let package_path = repo_root.join(package_root);
    if !package_path.is_dir() {
        bail!(
            "configured monorepo package {} is not a directory",
            package_root
        );
    }

    let version_files = detect_package_version_files(repo_root, &package_path)?;
    if version_files.is_empty() {
        bail!(
            "monorepo package {} has no supported version files",
            package_root
        );
    }

    Ok(PackageDefinition {
        name: detect_package_name(&package_path).unwrap_or_else(|| {
            package_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into()
        }),
        root: normalize_relative_path(package_root),
        version_files,
    })
}

fn scan_for_package_roots(repo_root: &Path, current: &Path, package_roots: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };

        if matches!(
            name,
            ".git" | "target" | ".venv" | "venv" | "__pycache__" | ".mypy_cache"
        ) {
            continue;
        }

        if path.is_dir() {
            scan_for_package_roots(repo_root, &path, package_roots);
            continue;
        }

        if name != "pyproject.toml" || path.parent() == Some(repo_root) {
            continue;
        }

        if let Some(parent) = path
            .parent()
            .and_then(|parent| parent.strip_prefix(repo_root).ok())
        {
            package_roots.push(parent.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn detect_package_version_files(
    repo_root: &Path,
    package_root: &Path,
) -> Result<Vec<VersionFileConfig>> {
    let mut version_files = Vec::new();
    let pyproject_path = package_root.join("pyproject.toml");
    if pyproject_path.exists() {
        version_files.push(VersionFileConfig {
            path: relative_to_repo(repo_root, &pyproject_path)?,
            key: Some("project.version".to_string()),
            pattern: None,
        });
    }

    let setup_cfg_path = package_root.join("setup.cfg");
    if setup_cfg_path.exists() {
        version_files.push(VersionFileConfig {
            path: relative_to_repo(repo_root, &setup_cfg_path)?,
            key: Some("metadata.version".to_string()),
            pattern: None,
        });
    }

    scan_python_version_files(repo_root, package_root, &mut version_files)?;

    version_files.sort_by(|left, right| left.path.cmp(&right.path));
    version_files.dedup_by(|left, right| left.path == right.path);
    Ok(version_files)
}

fn scan_python_version_files(
    repo_root: &Path,
    package_root: &Path,
    version_files: &mut Vec<VersionFileConfig>,
) -> Result<()> {
    let mut stack = vec![package_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };

            if matches!(name, ".git" | "target" | ".venv" | "venv" | "__pycache__") {
                continue;
            }

            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if name != "__init__.py" {
                continue;
            }

            let Some(pattern) = detect_python_pattern(&path) else {
                continue;
            };

            version_files.push(VersionFileConfig {
                path: relative_to_repo(repo_root, &path)?,
                key: None,
                pattern: Some(pattern),
            });
        }
    }

    Ok(())
}

fn detect_package_name(package_root: &Path) -> Option<String> {
    let pyproject = package_root.join("pyproject.toml");
    let contents = fs::read_to_string(pyproject).ok()?;
    let parsed = contents.parse::<toml::Table>().ok()?;
    parsed
        .get("project")?
        .as_table()?
        .get("name")?
        .as_str()
        .map(ToString::to_string)
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

fn package_name_from_repo_root(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn relative_to_repo(repo_root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(repo_root)
        .with_context(|| format!("{} is not inside {}", path.display(), repo_root.display()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn normalize_relative_path(path: &str) -> String {
    let normalized = path.trim_matches('/').replace('\\', "/");
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

fn commits_for_package(commits: &[CommitSummary], package_root: &str) -> Vec<CommitSummary> {
    commits
        .iter()
        .filter(|commit| commit_touches_package(commit, package_root))
        .cloned()
        .collect()
}

fn changed_paths_for_package(commits: &[CommitSummary], package_root: &str) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for commit in commits {
        for path in &commit.changed_paths {
            if path_in_package(path, package_root) {
                paths.insert(path.clone());
            }
        }
    }
    paths.into_iter().collect()
}

fn commit_touches_package(commit: &CommitSummary, package_root: &str) -> bool {
    commit
        .changed_paths
        .iter()
        .any(|path| path_in_package(path, package_root))
}

fn path_in_package(path: &str, package_root: &str) -> bool {
    package_root == "." || path == package_root || path.starts_with(&format!("{package_root}/"))
}

fn aggregate_changelog(packages: &[PackageReleaseAnalysis]) -> PendingChangelog {
    let mut sections = std::collections::BTreeMap::new();
    for package in packages.iter().filter(|package| package.selected) {
        for (section, entries) in &package.changelog.sections {
            let bucket = sections.entry(section.clone()).or_insert_with(Vec::new);
            for entry in entries {
                bucket.push(format!("{}: {}", package.name, entry));
            }
        }
    }
    PendingChangelog { sections }
}

pub fn read_current_version(
    repo_root: &Path,
    version_files: &[VersionFileConfig],
) -> Result<Option<String>> {
    for version_file in version_files {
        let path = repo_root.join(&version_file.path);
        if !path.exists() {
            continue;
        }

        let value = if let Some(key) = &version_file.key {
            version_files::read_key(&path, key)?
        } else if let Some(pattern) = &version_file.pattern {
            version_files::read_pattern(&path, pattern)?
        } else {
            None
        };

        if value.is_some() {
            return Ok(value);
        }
    }

    Ok(None)
}

pub fn update_version_files(
    repo_root: &Path,
    version_files: &[VersionFileConfig],
    version: &Version,
) -> Result<()> {
    for version_file in version_files {
        let path = repo_root.join(&version_file.path);

        if let Some(key) = &version_file.key {
            version_files::rewrite_key(&path, key, &version.to_string())
                .with_context(|| format!("failed to update {}", path.display()))?;
            continue;
        }

        if let Some(pattern) = &version_file.pattern {
            version_files::rewrite_pattern(&path, pattern, &version.to_string())
                .with_context(|| format!("failed to update {}", path.display()))?;
            continue;
        }

        bail!("version file {} has no key or pattern", path.display());
    }

    Ok(())
}
