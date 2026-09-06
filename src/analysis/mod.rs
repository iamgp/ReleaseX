use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Component, Path},
};

use anyhow::{Context, Result, bail};

use crate::{
    baseline::{self, PackageBaseline},
    changelog::PendingChangelog,
    config::{Config, Ecosystem, VersionFileConfig},
    conventional_commits::ConventionalCommit,
    ecosystem,
    git::{CommitSummary, GitRepository},
    github::{self, GitHubClient},
    version::{BumpLevel, Version},
    version_files,
};

#[derive(Debug, Clone, Default)]
pub struct AnalyzeOptions {
    pub packages: Vec<String>,
    pub prerelease_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredDependencyChange {
    pub package: String,
    pub path: String,
    pub dependency: String,
    pub declared_range: String,
    pub required_version: String,
    pub reason: String,
}

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
    pub baseline: PackageBaseline,
    pub release_tag: String,
    pub required_dependency_changes: Vec<RequiredDependencyChange>,
}

#[derive(Debug, Clone)]
struct PackageDefinition {
    name: String,
    root: String,
    version_files: Vec<VersionFileConfig>,
}

pub fn analyze(repo: &GitRepository, config: &Config) -> Result<ReleaseAnalysis> {
    analyze_with(repo, config, &AnalyzeOptions::default())
}

pub fn analyze_with(
    repo: &GitRepository,
    config: &Config,
    options: &AnalyzeOptions,
) -> Result<ReleaseAnalysis> {
    config.validate()?;

    if config.monorepo.is_multi_package() {
        analyze_monorepo(repo, config, options, None)
    } else {
        let commits = repo.commits_since_latest_tag()?;
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
        analyze_monorepo(repo, config, &AnalyzeOptions::default(), Some(commits))
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
    let mut changelog = PendingChangelog::from_commits(config, &conventional_commits);

    if config.changelog.contributors {
        let known_authors: std::collections::BTreeSet<String> =
            repo.authors_before_latest_tag()?.into_iter().collect();
        let display_commits = resolve_contributor_identities(repo, config, &commits);
        changelog.add_contributors(&display_commits, &known_authors, &config.changelog);
    }

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
                current_version: current_version.clone(),
                next_version: next_version.clone(),
                bump,
                changelog,
                version_files: config.version_files.clone(),
                commits,
                changed_paths: Vec::new(),
                selected: true,
                selection_reason: "single-package repository".to_string(),
                baseline: PackageBaseline::default(),
                release_tag: format!(
                    "{}{}",
                    config.release.tag_prefix,
                    next_version.as_ref().unwrap_or(&current_version)
                ),
                required_dependency_changes: Vec::new(),
            }],
        },
    })
}

fn analyze_monorepo(
    repo: &GitRepository,
    config: &Config,
    options: &AnalyzeOptions,
    shared_commits: Option<Vec<CommitSummary>>,
) -> Result<ReleaseAnalysis> {
    let (definitions, discovery_source) = discover_packages(repo.path(), config)?;
    if definitions.is_empty() {
        bail!("monorepo.enabled is true but no packages were discovered");
    }
    let known_names = definitions
        .iter()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    let exclusive = baseline::uses_independent_package_identity(config);

    let mut packages = Vec::new();
    for definition in &definitions {
        let baseline = baseline::resolve_package_baseline(
            repo,
            config,
            &definition.name,
            &definition.version_files,
            options.prerelease_kind.as_deref(),
            &known_names,
        )?;
        let commits = match &shared_commits {
            Some(commits) => commits.clone(),
            None => repo.commits_since_commit(baseline.commit.as_deref())?,
        };
        let package_commits = if exclusive {
            commits_for_package_exclusive(&commits, &definition.root, &definitions, config)
        } else {
            commits_for_package(&commits, &definition.root)
        };
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
        let intended = next_version.as_ref().unwrap_or(&current_version);
        let release_tag = if exclusive {
            baseline::package_release_tag(&definition.name, intended)
        } else {
            format!("{}{intended}", config.release.tag_prefix)
        };

        let mut changelog = PendingChangelog::from_commits(config, &conventional_commits);
        if config.changelog.contributors {
            let known_authors: std::collections::BTreeSet<String> =
                repo.authors_before_latest_tag()?.into_iter().collect();
            let display_commits = resolve_contributor_identities(repo, config, &package_commits);
            changelog.add_contributors(&display_commits, &known_authors, &config.changelog);
        }

        let selection_reason = if selected {
            format!(
                "package files changed since {} and produced a release bump",
                baseline
                    .reference
                    .as_deref()
                    .unwrap_or("the start of history")
            )
        } else {
            format!(
                "no releasable package changes detected since {}",
                baseline
                    .reference
                    .as_deref()
                    .unwrap_or("the start of history")
            )
        };

        packages.push(PackageReleaseAnalysis {
            name: definition.name.clone(),
            root: definition.root.clone(),
            current_version,
            next_version,
            bump,
            changelog,
            version_files: definition.version_files.clone(),
            commits: package_commits,
            changed_paths,
            selected,
            selection_reason,
            baseline,
            release_tag,
            required_dependency_changes: Vec::new(),
        });
    }

    apply_package_filter(&mut packages, &options.packages)?;
    apply_dependency_policy(repo.path(), config, &mut packages, &options.packages)?;

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
    let commits = shared_commits.unwrap_or_else(|| {
        packages
            .iter()
            .filter(|package| package.selected)
            .flat_map(|package| package.commits.iter().cloned())
            .collect()
    });

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
        let mut package_roots = config.monorepo.packages.clone();
        maybe_include_prerelease_root_package(repo_root, config, &mut package_roots);
        normalize_and_deduplicate_package_roots(&mut package_roots);
        let packages = package_roots
            .iter()
            .map(|package_root| load_package_definition(repo_root, package_root))
            .collect::<Result<Vec<_>>>()?;
        return Ok((packages, "[monorepo].packages".to_string()));
    }

    if let Some(uv_roots) = discover_uv_workspace(repo_root) {
        let mut package_roots = uv_roots;
        maybe_include_prerelease_root_package(repo_root, config, &mut package_roots);
        normalize_and_deduplicate_package_roots(&mut package_roots);
        let packages = package_roots
            .iter()
            .map(|package_root| load_package_definition(repo_root, package_root))
            .collect::<Result<Vec<_>>>()?;
        return Ok((
            packages,
            "uv workspace (tool.uv.workspace.members)".to_string(),
        ));
    }

    if let Some(cargo_roots) = discover_cargo_workspace(repo_root) {
        let packages = cargo_roots
            .iter()
            .map(|package_root| load_package_definition(repo_root, package_root))
            .collect::<Result<Vec<_>>>()?;
        return Ok((packages, "cargo workspace (workspace.members)".to_string()));
    }

    if let Some(go_roots) = discover_go_workspace(repo_root) {
        let packages = go_roots
            .iter()
            .map(|package_root| load_package_definition(repo_root, package_root))
            .collect::<Result<Vec<_>>>()?;
        return Ok((packages, "go workspace (go.work use)".to_string()));
    }

    if let Some(npm_roots) = discover_npm_workspace(repo_root) {
        let packages = npm_roots
            .iter()
            .map(|package_root| load_package_definition(repo_root, package_root))
            .collect::<Result<Vec<_>>>()?;
        return Ok((
            packages,
            "npm workspaces (package.json workspaces)".to_string(),
        ));
    }

    let mut package_roots = Vec::new();
    scan_for_package_roots(repo_root, repo_root, &mut package_roots);
    maybe_include_prerelease_root_package(repo_root, config, &mut package_roots);
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

fn maybe_include_prerelease_root_package(
    repo_root: &Path,
    config: &Config,
    package_roots: &mut Vec<String>,
) {
    if !config.prerelease.enabled
        || !config.prerelease.workspace.include_root
        || config.project.ecosystem != Some(Ecosystem::Python)
        || package_roots.iter().any(|root| root == ".")
    {
        return;
    }

    let Ok(version_files) = detect_python_package_version_files(repo_root, repo_root) else {
        return;
    };
    if version_files.is_empty() {
        return;
    }

    package_roots.insert(0, ".".to_string());
}

fn normalize_and_deduplicate_package_roots(package_roots: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    let mut normalized_roots = Vec::new();
    for package_root in package_roots.drain(..) {
        let normalized = normalize_package_root(&package_root);
        if seen.insert(normalized.clone()) {
            normalized_roots.push(normalized);
        }
    }
    *package_roots = normalized_roots;
}

fn normalize_package_root(package_root: &str) -> String {
    let mut parts = Vec::new();
    for component in Path::new(package_root).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir => {
                if parts.last().is_some_and(|part| part != "..") {
                    parts.pop();
                } else {
                    parts.push("..".to_string());
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
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
                    let rel = format!("{}/{}", prefix, entry.file_name().to_string_lossy());
                    roots.push(rel);
                }
            }
        } else if let Some(prefix) = pattern.strip_suffix("/**") {
            let parent_dir = repo_root.join(prefix);
            let entries = fs::read_dir(parent_dir).ok()?;
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let rel = format!("{}/{}", prefix, entry.file_name().to_string_lossy());
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

    if roots.is_empty() { None } else { Some(roots) }
}

pub fn extract_dependency_names(repo_root: &Path, package_root: &str) -> Vec<String> {
    let ecosystem = ecosystem::detect(&repo_root.join(package_root), None);
    match ecosystem {
        Ecosystem::Python => extract_python_dependency_names(repo_root, package_root),
        Ecosystem::Rust => extract_rust_dependency_names(repo_root, package_root),
        Ecosystem::Go => extract_go_dependency_names(repo_root, package_root),
        Ecosystem::TypeScript => extract_typescript_dependency_names(repo_root, package_root),
    }
}

pub fn discover_cargo_workspace(repo_root: &Path) -> Option<Vec<String>> {
    let cargo_toml_path = repo_root.join("Cargo.toml");
    let contents = fs::read_to_string(cargo_toml_path).ok()?;
    let parsed = contents.parse::<toml::Table>().ok()?;

    let members = parsed
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
                    let rel = format!("{}/{}", prefix, entry.file_name().to_string_lossy());
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

    if roots.is_empty() { None } else { Some(roots) }
}

pub fn discover_go_workspace(repo_root: &Path) -> Option<Vec<String>> {
    let go_work_path = repo_root.join("go.work");
    let contents = fs::read_to_string(go_work_path).ok()?;
    let mut roots = Vec::new();
    let mut in_use_block = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        if let Some(path) = trimmed.strip_prefix("use ") {
            let path = path.trim();
            if path == "(" {
                in_use_block = true;
                continue;
            }

            let normalized = path.trim_matches('"').trim();
            if normalized != "." {
                roots.push(normalized.to_string());
            }
            continue;
        }

        if in_use_block {
            if trimmed == ")" {
                in_use_block = false;
                continue;
            }

            let normalized = trimmed.trim_matches('"').trim();
            if !normalized.is_empty() && normalized != "." {
                roots.push(normalized.to_string());
            }
        }
    }

    roots.sort();
    roots.dedup();

    if roots.is_empty() { None } else { Some(roots) }
}

pub fn discover_npm_workspace(repo_root: &Path) -> Option<Vec<String>> {
    let manifest_path = repo_root.join("package.json");
    let contents = fs::read_to_string(manifest_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let workspaces = parsed.get("workspaces")?;
    let patterns: Vec<&str> = match workspaces {
        serde_json::Value::Array(array) => {
            array.iter().filter_map(|value| value.as_str()).collect()
        }
        serde_json::Value::Object(object) => object
            .get("packages")?
            .as_array()?
            .iter()
            .filter_map(|value| value.as_str())
            .collect(),
        _ => return None,
    };

    let mut roots = Vec::new();
    for pattern in patterns {
        if let Some(prefix) = pattern.strip_suffix("/*") {
            let parent_dir = repo_root.join(prefix);
            let entries = fs::read_dir(parent_dir).ok()?;
            for entry in entries.flatten() {
                if entry.path().is_dir() && entry.path().join("package.json").exists() {
                    let rel = format!("{}/{}", prefix, entry.file_name().to_string_lossy());
                    roots.push(rel);
                }
            }
        } else {
            let dir = repo_root.join(pattern);
            if dir.is_dir() && dir.join("package.json").exists() {
                roots.push(pattern.to_string());
            }
        }
    }

    roots.sort();
    roots.dedup();

    if roots.is_empty() { None } else { Some(roots) }
}

fn extract_python_dependency_names(repo_root: &Path, package_root: &str) -> Vec<String> {
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
                .split(['>', '<', '=', '!', '[', ';', ' ', '~'])
                .next()
                .unwrap_or(s);
            name.to_string()
        })
        .collect()
}

fn extract_rust_dependency_names(repo_root: &Path, package_root: &str) -> Vec<String> {
    let cargo_toml_path = repo_root.join(package_root).join("Cargo.toml");
    let contents = match fs::read_to_string(cargo_toml_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let parsed = match contents.parse::<toml::Table>() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut deps = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = parsed.get(section).and_then(|v| v.as_table()) else {
            continue;
        };
        deps.extend(table.keys().cloned());
    }
    deps.sort();
    deps.dedup();
    deps
}

fn extract_go_dependency_names(repo_root: &Path, package_root: &str) -> Vec<String> {
    let go_mod_path = repo_root.join(package_root).join("go.mod");
    let contents = match fs::read_to_string(go_mod_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut deps = Vec::new();
    let mut in_require_block = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("require ") {
            let rest = rest.trim();
            if rest == "(" {
                in_require_block = true;
                continue;
            }

            if let Some(module) = rest.split_whitespace().next() {
                deps.push(go_module_short_name(module));
            }
            continue;
        }

        if in_require_block {
            if trimmed == ")" {
                in_require_block = false;
                continue;
            }

            if let Some(module) = trimmed.split_whitespace().next() {
                deps.push(go_module_short_name(module));
            }
        }
    }

    deps.sort();
    deps.dedup();
    deps
}

pub fn apply_cascade_bumps(
    repo_root: &Path,
    config: &Config,
    packages: &mut [PackageReleaseAnalysis],
) {
    if !config.workspace.cascade_bumps {
        return;
    }

    let bumped_names: Vec<(String, Version)> = packages
        .iter()
        .filter(|p| p.selected && p.next_version.is_some())
        .map(|p| {
            (
                p.name.clone(),
                p.next_version.clone().expect("next version"),
            )
        })
        .collect();

    if bumped_names.is_empty() {
        return;
    }

    for package in packages.iter_mut() {
        if package.selected {
            continue;
        }

        let deps = extract_declared_dependencies(repo_root, &package.root);
        let mut matched: Option<(String, Option<String>, Version)> = None;
        for dep in &deps {
            if let Some((_, version)) = bumped_names.iter().find(|(name, _)| name == &dep.name) {
                matched = Some((dep.name.clone(), dep.range.clone(), version.clone()));
                break;
            }
        }
        let Some((dep_name, range, version)) = matched else {
            continue;
        };

        if let Some(declared) = range.as_deref()
            && version_satisfies_range(&version, declared)
        {
            continue;
        }

        let next = BumpLevel::Patch.apply(&package.current_version);
        package.next_version = next;
        package.bump = BumpLevel::Patch;
        package.selected = true;
        package.release_tag = if baseline::uses_independent_package_identity(config) {
            baseline::package_release_tag(
                &package.name,
                package
                    .next_version
                    .as_ref()
                    .unwrap_or(&package.current_version),
            )
        } else {
            package.release_tag.clone()
        };
        package.selection_reason = if range.is_some() {
            format!(
                "cascade bump: published dependency on {dep_name} is outside the declared range"
            )
        } else {
            "cascade bump: depends on a package with a version bump".to_string()
        };
    }
}

fn apply_package_filter(
    packages: &mut [PackageReleaseAnalysis],
    requested: &[String],
) -> Result<()> {
    if requested.is_empty() {
        return Ok(());
    }
    let known = packages
        .iter()
        .map(|package| package.name.clone())
        .collect::<BTreeSet<_>>();
    for name in requested {
        if !known.contains(name) {
            bail!(
                "unknown --package `{name}`; known packages: {}",
                known.iter().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }
    for package in packages.iter_mut() {
        if !requested.iter().any(|name| name == &package.name) && package.selected {
            package.selected = false;
            package.next_version = None;
            package.bump = BumpLevel::None;
            package.selection_reason = format!(
                "excluded by --package filter; unreleased changes remain on {}",
                package
                    .baseline
                    .reference
                    .as_deref()
                    .unwrap_or("its own baseline")
            );
        }
    }
    Ok(())
}

fn apply_dependency_policy(
    repo_root: &Path,
    config: &Config,
    packages: &mut [PackageReleaseAnalysis],
    requested: &[String],
) -> Result<()> {
    let selected: Vec<(String, Version)> = packages
        .iter()
        .filter(|package| package.selected)
        .filter_map(|package| {
            package
                .next_version
                .clone()
                .map(|version| (package.name.clone(), version))
        })
        .collect();

    let mut required = Vec::new();
    for package in packages.iter() {
        let deps = extract_declared_dependencies(repo_root, &package.root);
        for dep in deps {
            let Some((_, version)) = selected.iter().find(|(name, _)| name == &dep.name) else {
                continue;
            };
            let Some(range) = dep.range.as_deref() else {
                continue;
            };
            if version_satisfies_range(version, range) {
                continue;
            }
            required.push(RequiredDependencyChange {
                package: package.name.clone(),
                path: package.root.clone(),
                dependency: dep.name.clone(),
                declared_range: range.to_string(),
                required_version: version.to_string(),
                reason: format!(
                    "selected {} {} is outside declared range {range}; the dependent must publish a metadata change",
                    dep.name,
                    version
                ),
            });
        }
    }

    let mut blocked = Vec::new();
    for change in required {
        let Some(package) = packages
            .iter_mut()
            .find(|package| package.name == change.package)
        else {
            continue;
        };
        package.required_dependency_changes.push(change.clone());
        if package.selected {
            continue;
        }
        if config.workspace.cascade_bumps {
            let next = BumpLevel::Patch.apply(&package.current_version);
            package.next_version = next.clone();
            package.bump = BumpLevel::Patch;
            package.selected = true;
            package.selection_reason = change.reason.clone();
            if baseline::uses_independent_package_identity(config)
                && let Some(version) = &package.next_version
            {
                package.release_tag = baseline::package_release_tag(&package.name, version);
            }
            continue;
        }
        if !requested.is_empty() && !requested.iter().any(|name| name == &package.name) {
            blocked.push(change);
            continue;
        }
        blocked.push(change);
    }

    if !blocked.is_empty() {
        let details = blocked
            .iter()
            .map(|change| {
                format!(
                    "`{}` must be selected because {} {} is outside `{}`",
                    change.package,
                    change.dependency,
                    change.required_version,
                    change.declared_range
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        bail!(
            "incompatible workspace dependency update requires an explicit package selection: {details}"
        );
    }

    if config.workspace.cascade_bumps {
        apply_cascade_bumps(repo_root, config, packages);
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct DeclaredDependency {
    name: String,
    range: Option<String>,
}

fn extract_declared_dependencies(repo_root: &Path, package_root: &str) -> Vec<DeclaredDependency> {
    let ecosystem = ecosystem::detect(&repo_root.join(package_root), None);
    match ecosystem {
        Ecosystem::Python => extract_python_declared_dependencies(repo_root, package_root),
        Ecosystem::Rust => extract_rust_declared_dependencies(repo_root, package_root),
        Ecosystem::Go => extract_go_declared_dependencies(repo_root, package_root),
        Ecosystem::TypeScript => extract_typescript_declared_dependencies(repo_root, package_root),
    }
}

fn extract_python_declared_dependencies(
    repo_root: &Path,
    package_root: &str,
) -> Vec<DeclaredDependency> {
    let pyproject_path = repo_root.join(package_root).join("pyproject.toml");
    let Ok(contents) = fs::read_to_string(pyproject_path) else {
        return Vec::new();
    };
    let Ok(parsed) = contents.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(deps) = parsed
        .get("project")
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("dependencies"))
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    deps.iter()
        .filter_map(|value| value.as_str())
        .filter_map(split_python_requirement)
        .map(|(name, range)| DeclaredDependency { name, range })
        .collect()
}

fn split_python_requirement(raw: &str) -> Option<(String, Option<String>)> {
    let base = raw.split(';').next()?.trim();
    let name_end = base
        .char_indices()
        .find_map(|(index, ch)| {
            matches!(ch, '[' | ' ' | '\t' | '<' | '>' | '=' | '!' | '~' | '@').then_some(index)
        })
        .unwrap_or(base.len());
    let name = base[..name_end].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let rest = base[name_end..].trim();
    let rest = rest
        .strip_prefix('[')
        .and_then(|value| value.find(']').map(|index| value[index + 1..].trim()))
        .unwrap_or(rest);
    let range = if rest.is_empty() || rest.starts_with('@') {
        None
    } else {
        Some(rest.to_string())
    };
    Some((name, range))
}

fn extract_rust_declared_dependencies(
    repo_root: &Path,
    package_root: &str,
) -> Vec<DeclaredDependency> {
    let cargo_toml_path = repo_root.join(package_root).join("Cargo.toml");
    let Ok(contents) = fs::read_to_string(cargo_toml_path) else {
        return Vec::new();
    };
    let Ok(parsed) = contents.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut deps = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = parsed.get(section).and_then(|value| value.as_table()) else {
            continue;
        };
        for (name, value) in table {
            let range = match value {
                toml::Value::String(version) => Some(format!("=={version}")),
                toml::Value::Table(table) => table
                    .get("version")
                    .and_then(|value| value.as_str())
                    .map(|version| format!("=={version}")),
                _ => None,
            };
            deps.push(DeclaredDependency {
                name: name.clone(),
                range,
            });
        }
    }
    deps
}

fn extract_go_declared_dependencies(
    repo_root: &Path,
    package_root: &str,
) -> Vec<DeclaredDependency> {
    extract_go_dependency_names(repo_root, package_root)
        .into_iter()
        .map(|name| DeclaredDependency { name, range: None })
        .collect()
}

fn extract_typescript_declared_dependencies(
    repo_root: &Path,
    package_root: &str,
) -> Vec<DeclaredDependency> {
    extract_typescript_dependency_names(repo_root, package_root)
        .into_iter()
        .map(|name| DeclaredDependency { name, range: None })
        .collect()
}

fn parse_version_bound(raw: &str) -> Option<Version> {
    let trimmed = raw.trim();
    if trimmed.parse::<Version>().is_ok() {
        return trimmed.parse().ok();
    }
    let padded = match trimmed.split('.').count() {
        1 => format!("{trimmed}.0.0"),
        2 => format!("{trimmed}.0"),
        _ => trimmed.to_string(),
    };
    padded.parse().ok()
}

fn version_satisfies_range(version: &Version, range: &str) -> bool {
    range.split(',').all(|raw| {
        let clause = raw.trim();
        if let Some(min) = clause.strip_prefix(">=") {
            return parse_version_bound(min).is_some_and(|bound| version >= &bound);
        }
        if let Some(max) = clause.strip_prefix("<=") {
            return parse_version_bound(max).is_some_and(|bound| version <= &bound);
        }
        if let Some(min) = clause.strip_prefix('>') {
            return parse_version_bound(min).is_some_and(|bound| version > &bound);
        }
        if let Some(max) = clause.strip_prefix('<') {
            return parse_version_bound(max).is_some_and(|bound| version < &bound);
        }
        if let Some(exact) = clause.strip_prefix("==") {
            return parse_version_bound(exact).is_some_and(|bound| version == &bound);
        }
        if let Some(exact) = clause.strip_prefix('=') {
            return parse_version_bound(exact).is_some_and(|bound| version == &bound);
        }
        false
    })
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
            ".git" | "target" | ".venv" | "venv" | "__pycache__" | ".mypy_cache" | "node_modules"
        ) {
            continue;
        }

        if path.is_dir() {
            scan_for_package_roots(repo_root, &path, package_roots);
            continue;
        }

        if (name != "pyproject.toml" && name != "package.json") || path.parent() == Some(repo_root)
        {
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

pub fn detect_package_version_files_for_manifest(
    repo_root: &Path,
    package_root: &Path,
) -> Result<Vec<VersionFileConfig>> {
    detect_package_version_files(repo_root, package_root)
}

fn detect_package_version_files(
    repo_root: &Path,
    package_root: &Path,
) -> Result<Vec<VersionFileConfig>> {
    let ecosystem = ecosystem::detect(package_root, None);
    match ecosystem {
        Ecosystem::Python => detect_python_package_version_files(repo_root, package_root),
        Ecosystem::Rust => detect_rust_package_version_files(repo_root, package_root),
        Ecosystem::Go => detect_go_package_version_files(repo_root, package_root),
        Ecosystem::TypeScript => detect_typescript_package_version_files(repo_root, package_root),
    }
}

fn detect_python_package_version_files(
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

fn detect_rust_package_version_files(
    repo_root: &Path,
    package_root: &Path,
) -> Result<Vec<VersionFileConfig>> {
    let cargo_toml_path = package_root.join("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Ok(Vec::new());
    }

    Ok(vec![VersionFileConfig {
        path: relative_to_repo(repo_root, &cargo_toml_path)?,
        key: Some("package.version".to_string()),
        pattern: None,
    }])
}

fn detect_go_package_version_files(
    repo_root: &Path,
    package_root: &Path,
) -> Result<Vec<VersionFileConfig>> {
    let mut version_files = Vec::new();

    for candidate in ["VERSION", "version.txt"] {
        let path = package_root.join(candidate);
        if path.exists() {
            version_files.push(VersionFileConfig {
                path: relative_to_repo(repo_root, &path)?,
                key: None,
                pattern: Some("{version}".to_string()),
            });
        }
    }

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
                if path != package_root && is_nested_package_root(&path) {
                    continue;
                }
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

fn is_nested_package_root(path: &Path) -> bool {
    path.join("pyproject.toml").exists()
        || path.join("Cargo.toml").exists()
        || path.join("go.mod").exists()
        || path.join("package.json").exists()
}

fn detect_package_name(package_root: &Path) -> Option<String> {
    if let Some(name) = detect_python_package_name(package_root) {
        return Some(name);
    }

    if let Some(name) = detect_rust_package_name(package_root) {
        return Some(name);
    }

    if let Some(name) = detect_go_package_name(package_root) {
        return Some(name);
    }

    detect_typescript_package_name(package_root)
}

pub fn detect_project_name(repo_root: &Path, package_root: &str) -> Option<String> {
    let package_path = if package_root == "." {
        repo_root.to_path_buf()
    } else {
        repo_root.join(package_root)
    };
    detect_package_name(&package_path)
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
    detect_package_name(repo_root).unwrap_or_else(|| {
        repo_root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    })
}

fn detect_python_package_name(package_root: &Path) -> Option<String> {
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

fn detect_rust_package_name(package_root: &Path) -> Option<String> {
    let cargo_toml = package_root.join("Cargo.toml");
    let contents = fs::read_to_string(cargo_toml).ok()?;
    let parsed = contents.parse::<toml::Table>().ok()?;
    parsed
        .get("package")?
        .as_table()?
        .get("name")?
        .as_str()
        .map(ToString::to_string)
}

fn detect_go_package_name(package_root: &Path) -> Option<String> {
    let go_mod = package_root.join("go.mod");
    let contents = fs::read_to_string(go_mod).ok()?;
    let module = contents.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("module ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })?;
    Some(module.rsplit('/').next().unwrap_or(module).to_string())
}

fn go_module_short_name(module: &str) -> String {
    module.rsplit('/').next().unwrap_or(module).to_string()
}

fn detect_typescript_package_version_files(
    repo_root: &Path,
    package_root: &Path,
) -> Result<Vec<VersionFileConfig>> {
    let manifest_path = package_root.join("package.json");
    if !manifest_path.exists() {
        return Ok(Vec::new());
    }

    Ok(vec![VersionFileConfig {
        path: relative_to_repo(repo_root, &manifest_path)?,
        key: Some("version".to_string()),
        pattern: None,
    }])
}

fn detect_typescript_package_name(package_root: &Path) -> Option<String> {
    let manifest = package_root.join("package.json");
    let contents = fs::read_to_string(manifest).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&contents).ok()?;
    parsed.get("name")?.as_str().map(ToString::to_string)
}

fn extract_typescript_dependency_names(repo_root: &Path, package_root: &str) -> Vec<String> {
    let manifest_path = repo_root.join(package_root).join("package.json");
    let contents = match fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut deps = Vec::new();
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        let Some(table) = parsed.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        deps.extend(table.keys().cloned());
    }
    deps.sort();
    deps.dedup();
    deps
}

fn relative_to_repo(repo_root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(repo_root)
        .with_context(|| format!("{} is not inside {}", path.display(), repo_root.display()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn normalize_relative_path(path: &str) -> String {
    let normalized = path
        .trim()
        .trim_start_matches("./")
        .trim_matches('/')
        .replace('\\', "/");
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

fn commits_for_package_exclusive(
    commits: &[CommitSummary],
    package_root: &str,
    definitions: &[PackageDefinition],
    config: &Config,
) -> Vec<CommitSummary> {
    commits
        .iter()
        .filter(|commit| {
            commit.changed_paths.iter().any(|path| {
                !baseline::is_bookkeeping_path(path, config)
                    && owning_package(path, definitions).is_some_and(|root| root == package_root)
            })
        })
        .cloned()
        .collect()
}

fn owning_package<'a>(path: &str, definitions: &'a [PackageDefinition]) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    let mut best_len = 0;
    for definition in definitions {
        if definition.root == "." {
            continue;
        }
        if (path == definition.root || path.starts_with(&format!("{}/", definition.root)))
            && definition.root.len() > best_len
        {
            best = Some(definition.root.as_str());
            best_len = definition.root.len();
        }
    }
    if best.is_some() {
        return best;
    }
    definitions
        .iter()
        .find(|definition| definition.root == ".")
        .map(|definition| definition.root.as_str())
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
    let mut contributor_map: std::collections::BTreeMap<String, (usize, bool)> =
        std::collections::BTreeMap::new();
    for package in packages.iter().filter(|package| package.selected) {
        for (section, entries) in &package.changelog.sections {
            let bucket = sections.entry(section.clone()).or_insert_with(Vec::new);
            for entry in entries {
                bucket.push(format!("{}: {}", package.name, entry));
            }
        }
        for contributor in &package.changelog.contributors {
            let entry = contributor_map
                .entry(contributor.name.clone())
                .or_insert((0, contributor.first_contribution));
            entry.0 += contributor.commit_count;
            entry.1 = entry.1 && contributor.first_contribution;
        }
    }
    let mut contributors: Vec<crate::changelog::ContributorInfo> = contributor_map
        .into_iter()
        .map(
            |(name, (commit_count, first_contribution))| crate::changelog::ContributorInfo {
                name,
                commit_count,
                first_contribution,
            },
        )
        .collect();
    contributors.sort_by(|a, b| {
        b.commit_count
            .cmp(&a.commit_count)
            .then(a.name.cmp(&b.name))
    });
    PendingChangelog {
        sections,
        contributors,
    }
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

fn resolve_contributor_identities(
    repo: &GitRepository,
    config: &Config,
    commits: &[CommitSummary],
) -> Vec<CommitSummary> {
    let Ok(repo_ref) = github::detect_repo(repo, &config.github) else {
        return commits.to_vec();
    };
    let Ok(token) = env::var(&config.github.token_env) else {
        return commits.to_vec();
    };
    let Ok(client) = GitHubClient::new(&config.github.api_base, &token, repo_ref) else {
        return commits.to_vec();
    };

    let mut logins = BTreeMap::new();
    for commit in commits {
        if let Ok(details) = client.commit_details(&commit.id)
            && let Some(user) = details.author.or(details.committer)
        {
            logins.insert(commit.id.clone(), user.login);
        }
    }

    commits
        .iter()
        .cloned()
        .map(|mut commit| {
            if let Some(login) = logins.get(&commit.id) {
                commit.author = login.clone();
            }
            commit
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{detect_python_package_version_files, discover_packages, read_current_version};
    use crate::config::Config;

    #[test]
    fn root_python_package_skips_nested_workspace_version_files() {
        let dir = tempdir().expect("tempdir");
        let repo_root = dir.path();

        fs::create_dir_all(repo_root.join("src/phlo")).expect("create root package");
        fs::write(
            repo_root.join("pyproject.toml"),
            r#"
[project]
name = "phlo"
version = "0.7.0"
"#,
        )
        .expect("write root pyproject");
        fs::write(
            repo_root.join("src/phlo/__init__.py"),
            r#"__version__ = "0.7.0""#,
        )
        .expect("write root init");

        fs::create_dir_all(repo_root.join("packages/plugin/src/plugin")).expect("create plugin");
        fs::write(
            repo_root.join("packages/plugin/pyproject.toml"),
            r#"
[project]
name = "phlo-plugin"
version = "0.2.1"
"#,
        )
        .expect("write plugin pyproject");
        fs::write(
            repo_root.join("packages/plugin/src/plugin/__init__.py"),
            r#"__version__ = "0.2.1""#,
        )
        .expect("write plugin init");

        let version_files =
            detect_python_package_version_files(repo_root, repo_root).expect("version files");

        assert!(
            version_files
                .iter()
                .all(|entry| !entry.path.starts_with("packages/plugin/")),
            "nested workspace version files should not belong to the root package: {version_files:?}"
        );

        let current_version =
            read_current_version(repo_root, &version_files).expect("read current version");
        assert_eq!(current_version.as_deref(), Some("0.7.0"));
    }

    #[test]
    fn prerelease_python_uv_workspace_discovery_includes_root_package() {
        let dir = tempdir().expect("tempdir");
        let repo_root = dir.path();

        fs::write(
            repo_root.join("pyproject.toml"),
            r#"
[project]
name = "phlo"
version = "0.8.0"

[tool.uv.workspace]
members = ["packages/*"]
"#,
        )
        .expect("write root pyproject");
        fs::create_dir_all(repo_root.join("packages/iceberg")).expect("create workspace package");
        fs::write(
            repo_root.join("packages/iceberg/pyproject.toml"),
            r#"
[project]
name = "phlo-iceberg"
version = "0.3.0"
"#,
        )
        .expect("write package pyproject");
        let config: Config = toml::from_str(
            r#"
[project]
ecosystem = "python"

[[version_files]]
path = "pyproject.toml"
key = "project.version"

[monorepo]
enabled = true
release_mode = "release_set"

[prerelease]
enabled = true
"#,
        )
        .expect("config");

        let (packages, source) = discover_packages(repo_root, &config).expect("packages");

        assert_eq!(source, "uv workspace (tool.uv.workspace.members)");
        assert_eq!(packages[0].name, "phlo");
        assert_eq!(packages[0].root, ".");
        assert_eq!(packages[1].name, "phlo-iceberg");
        assert_eq!(packages[1].root, "packages/iceberg");
    }

    #[test]
    fn npm_workspace_discovery_finds_member_packages() {
        let dir = tempdir().expect("tempdir");
        let repo_root = dir.path();

        fs::write(
            repo_root.join("package.json"),
            r#"{"name": "acme-app", "version": "1.4.0", "workspaces": ["packages/*"]}"#,
        )
        .expect("write root package.json");
        fs::create_dir_all(repo_root.join("packages/plugin")).expect("create package");
        fs::write(
            repo_root.join("packages/plugin/package.json"),
            r#"{"name": "@acme/plugin", "version": "1.4.0"}"#,
        )
        .expect("write package manifest");
        let config: Config = toml::from_str(
            r#"
            [project]
            ecosystem = "typescript"

            [monorepo]
            enabled = true
            release_mode = "release_set"
            "#,
        )
        .expect("config");

        let (packages, source) = discover_packages(repo_root, &config).expect("packages");

        assert_eq!(source, "npm workspaces (package.json workspaces)");
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "@acme/plugin");
        assert_eq!(packages[0].root, "packages/plugin");
        assert_eq!(
            packages[0].version_files,
            vec![crate::config::VersionFileConfig {
                path: "packages/plugin/package.json".to_string(),
                key: Some("version".to_string()),
                pattern: None,
            }]
        );
    }

    #[test]
    fn explicit_monorepo_package_roots_are_normalized_and_deduplicated() {
        let dir = tempdir().expect("tempdir");
        let repo_root = dir.path();

        fs::write(
            repo_root.join("pyproject.toml"),
            r#"
[project]
name = "phlo"
version = "0.8.0"
"#,
        )
        .expect("write root pyproject");
        fs::create_dir_all(repo_root.join("packages/iceberg")).expect("create workspace package");
        fs::write(
            repo_root.join("packages/iceberg/pyproject.toml"),
            r#"
[project]
name = "phlo-iceberg"
version = "0.3.0"
"#,
        )
        .expect("write package pyproject");
        let config: Config = toml::from_str(
            r#"
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "./", "packages/iceberg", "packages/./iceberg"]
"#,
        )
        .expect("config");

        let (packages, source) = discover_packages(repo_root, &config).expect("packages");
        let roots = packages
            .iter()
            .map(|package| package.root.as_str())
            .collect::<Vec<_>>();

        assert_eq!(source, "[monorepo].packages");
        assert_eq!(roots, vec![".", "packages/iceberg"]);
    }
}
