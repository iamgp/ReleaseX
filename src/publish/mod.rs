use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    analysis::{self, ReleaseAnalysis},
    config::{Config, PublishConfig},
    pypi,
    version::Version,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishPlan {
    pub provider: String,
    pub repository: String,
    pub repository_url: Option<String>,
    pub dist_files: Vec<PathBuf>,
    pub command: Vec<OsString>,
    pub env: Vec<(String, String)>,
    pub trusted_publishing: bool,
}

pub fn execute(repo_root: &Path, config: &Config, skip_published: bool) -> Result<()> {
    // Check if we should skip based on PyPI version check
    if skip_published
        && let Some(version) = get_current_version(repo_root, config)
        && let Some(package_name) = get_package_name(repo_root, ".")
    {
        match check_already_published(&package_name, &version, config.publish.provider.as_str()) {
            Ok(true) => {
                println!("Skipping {package_name} {version}: already published");
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("Warning: Could not check if package is already published: {e}");
            }
        }
    }

    let plan = build_plan(repo_root, &config.publish)?;
    let mut command = command_from_plan(&plan);
    let status = command
        .current_dir(repo_root)
        .status()
        .with_context(|| format!("failed to launch {} publish command", plan.provider))?;

    if !status.success() {
        bail!(
            "{} publish failed with status {}",
            plan.provider,
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
    }

    println!(
        "Published {} artifact(s) with {} to {}",
        plan.dist_files.len(),
        plan.provider,
        plan.target_label()
    );
    Ok(())
}

pub fn execute_monorepo(
    repo_root: &Path,
    config: &Config,
    analysis: &ReleaseAnalysis,
    skip_published: bool,
) -> Result<()> {
    let mut skipped = Vec::new();
    let mut published = Vec::new();

    for (package_name, package_root) in monorepo_publish_targets(repo_root, analysis)? {
        // Check if we should skip based on PyPI version check
        if skip_published {
            let package_version = analysis
                .package_plan
                .packages
                .iter()
                .find(|p| p.name == package_name)
                .and_then(|p| p.next_version.clone());

            if let Some(ref version) = package_version {
                match check_already_published(
                    package_name,
                    version,
                    config.publish.provider.as_str(),
                ) {
                    Ok(true) => {
                        println!("Skipping {package_name} {version}: already published");
                        skipped.push((package_name, version.to_string()));
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!(
                            "Warning: Could not check if {package_name} is already published: {e}"
                        );
                    }
                }
            }
        }

        let plan = build_plan_for_package(&package_root, &config.publish, Some(package_name))?;
        let mut command = command_from_plan(&plan);
        let status = command
            .current_dir(&package_root)
            .status()
            .with_context(|| {
                format!(
                    "failed to launch {} publish command for {}",
                    plan.provider, package_name
                )
            })?;

        if !status.success() {
            bail!(
                "{} publish failed for {} with status {}",
                plan.provider,
                package_name,
                status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            );
        }

        println!(
            "Published {} ({} artifact(s)) with {} to {}",
            package_name,
            plan.dist_files.len(),
            plan.provider,
            plan.target_label()
        );
        published.push(package_name);
    }

    if skipped.is_empty() && published.is_empty() {
        bail!("no releasable packages found in monorepo");
    }

    if !skipped.is_empty() {
        println!("\nSkipped (already published):");
        for (name, version) in &skipped {
            println!("  - {} {}", name, version);
        }
    }

    Ok(())
}

fn monorepo_publish_targets<'a>(
    repo_root: &'a Path,
    analysis: &'a ReleaseAnalysis,
) -> Result<Vec<(&'a str, PathBuf)>> {
    if analysis.package_plan.release_mode == "unified" {
        return Ok(vec![("workspace", repo_root.to_path_buf())]);
    }

    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        bail!("no releasable packages found in monorepo");
    }

    Ok(selected
        .into_iter()
        .map(|package| (package.name.as_str(), repo_root.join(&package.root)))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path};

    use tempfile::tempdir;

    use super::{artifact_matches_package, build_plan_for_package, monorepo_publish_targets};
    use crate::{
        analysis::{PackagePlan, PackageReleaseAnalysis, ReleaseAnalysis},
        changelog::PendingChangelog,
        config::PublishConfig,
        git::CommitSummary,
        version::{BumpLevel, Version},
    };

    #[test]
    fn unified_monorepo_publish_targets_repo_root() {
        let repo_path = Path::new("/tmp/workspace");
        let analysis = ReleaseAnalysis {
            current_version: Version {
                major: 0,
                minor: 2,
                patch: 0,
                suffix: None,
            },
            next_version: Some(Version {
                major: 0,
                minor: 2,
                patch: 1,
                suffix: None,
            }),
            bump: BumpLevel::Patch,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::new(),
                contributors: Vec::new(),
            },
            package_plan: PackagePlan {
                release_mode: "unified".to_string(),
                discovery_source: "test".to_string(),
                packages: vec![PackageReleaseAnalysis {
                    name: "phlo".to_string(),
                    root: ".".to_string(),
                    current_version: Version {
                        major: 0,
                        minor: 2,
                        patch: 0,
                        suffix: None,
                    },
                    next_version: Some(Version {
                        major: 0,
                        minor: 2,
                        patch: 1,
                        suffix: None,
                    }),
                    bump: BumpLevel::Patch,
                    changelog: PendingChangelog {
                        sections: BTreeMap::new(),
                        contributors: Vec::new(),
                    },
                    version_files: Vec::new(),
                    commits: Vec::<CommitSummary>::new(),
                    changed_paths: vec!["pyproject.toml".to_string()],
                    selected: true,
                    selection_reason: "test".to_string(),
                }],
            },
        };

        let targets = monorepo_publish_targets(repo_path, &analysis).expect("targets");
        assert_eq!(targets, vec![("workspace", repo_path.to_path_buf())]);
    }

    #[test]
    fn release_set_monorepo_publish_targets_selected_packages() {
        let repo_path = Path::new("/tmp/workspace");
        let analysis = ReleaseAnalysis {
            current_version: Version {
                major: 0,
                minor: 7,
                patch: 2,
                suffix: None,
            },
            next_version: Some(Version {
                major: 0,
                minor: 7,
                patch: 3,
                suffix: None,
            }),
            bump: BumpLevel::Patch,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::new(),
                contributors: Vec::new(),
            },
            package_plan: PackagePlan {
                release_mode: "release_set".to_string(),
                discovery_source: "test".to_string(),
                packages: vec![
                    PackageReleaseAnalysis {
                        name: "phlo".to_string(),
                        root: ".".to_string(),
                        current_version: Version {
                            major: 0,
                            minor: 7,
                            patch: 2,
                            suffix: None,
                        },
                        next_version: Some(Version {
                            major: 0,
                            minor: 7,
                            patch: 3,
                            suffix: None,
                        }),
                        bump: BumpLevel::Patch,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::<CommitSummary>::new(),
                        changed_paths: vec!["pyproject.toml".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                    PackageReleaseAnalysis {
                        name: "phlo-delta".to_string(),
                        root: "packages/phlo-delta".to_string(),
                        current_version: Version {
                            major: 0,
                            minor: 2,
                            patch: 3,
                            suffix: None,
                        },
                        next_version: Some(Version {
                            major: 0,
                            minor: 2,
                            patch: 4,
                            suffix: None,
                        }),
                        bump: BumpLevel::Patch,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::<CommitSummary>::new(),
                        changed_paths: vec!["packages/phlo-delta/src/mod.py".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                ],
            },
        };

        let targets = monorepo_publish_targets(repo_path, &analysis).expect("targets");
        assert_eq!(
            targets,
            vec![
                ("phlo", repo_path.to_path_buf()),
                ("phlo-delta", repo_path.join("packages/phlo-delta"),),
            ]
        );
    }

    #[test]
    fn artifact_matching_requires_exact_distribution_name_prefix() {
        assert!(artifact_matches_package("phlo-0.7.8.tar.gz", "phlo"));
        assert!(artifact_matches_package(
            "phlo_core_plugins-0.2.3-py3-none-any.whl",
            "phlo-core-plugins"
        ));
        assert!(!artifact_matches_package(
            "phlo_core_plugins-0.2.3.tar.gz",
            "phlo"
        ));
        assert!(!artifact_matches_package(
            "phlo_lineage-0.2.4.tar.gz",
            "phlo-dbt"
        ));
    }

    #[test]
    fn release_set_root_publish_plan_filters_repo_dist_to_selected_package() {
        let dir = tempdir().expect("tempdir");
        let dist_dir = dir.path().join("dist");
        fs::create_dir_all(&dist_dir).expect("create dist");
        fs::write(dist_dir.join("phlo-0.7.8.tar.gz"), b"root sdist").expect("write root sdist");
        fs::write(dist_dir.join("phlo-0.7.8-py3-none-any.whl"), b"root wheel")
            .expect("write root wheel");
        fs::write(
            dist_dir.join("phlo_core_plugins-0.2.3.tar.gz"),
            b"workspace sdist",
        )
        .expect("write plugin sdist");
        fs::write(
            dist_dir.join("phlo_core_plugins-0.2.3-py3-none-any.whl"),
            b"workspace wheel",
        )
        .expect("write plugin wheel");

        let publish = PublishConfig {
            enabled: true,
            provider: "uv".to_string(),
            repository: "pypi".to_string(),
            repository_url: None,
            dist_dir: "dist".to_string(),
            username_env: None,
            password_env: None,
            token_env: None,
            trusted_publishing: false,
            oidc: false,
            skip_published: false,
        };

        let plan = build_plan_for_package(dir.path(), &publish, Some("phlo")).expect("plan");
        let file_names = plan
            .dist_files
            .iter()
            .map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .expect("filename")
                    .to_string()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            file_names,
            vec![
                "phlo-0.7.8-py3-none-any.whl".to_string(),
                "phlo-0.7.8.tar.gz".to_string(),
            ]
        );
    }
}

pub fn print_dry_run(repo_root: &Path, config: &Config, skip_published: bool) -> Result<()> {
    let plan = build_plan_dry_run(repo_root, &config.publish)?;

    println!("Publish is enabled: {}", config.publish.enabled);
    println!("Provider: {}", plan.provider);
    println!("Target repository: {}", plan.target_label());
    if skip_published {
        println!("Skip published: enabled (will check PyPI/crates.io before publishing)");
    }
    println!("Artifacts: {}", plan.dist_files.len());
    for artifact in &plan.dist_files {
        println!("  - {}", artifact.display());
    }
    if plan.trusted_publishing {
        println!("Trusted publishing: enabled");
        if oidc_env_available() {
            println!("OIDC: Would exchange OIDC token with PyPI");
        } else {
            println!("OIDC: GitHub Actions OIDC env vars not detected");
        }
    }
    if !plan.env.is_empty() {
        println!("Environment:");
        for (key, _) in &plan.env {
            println!("  - {}=<set>", key);
        }
    }
    println!("Command: {}", render_command(&plan.command));

    Ok(())
}

pub fn build_plan(repo_root: &Path, publish: &PublishConfig) -> Result<PublishPlan> {
    build_plan_inner(repo_root, publish, false, None)
}

fn build_plan_dry_run(repo_root: &Path, publish: &PublishConfig) -> Result<PublishPlan> {
    build_plan_inner(repo_root, publish, true, None)
}

fn build_plan_for_package(
    repo_root: &Path,
    publish: &PublishConfig,
    package_name: Option<&str>,
) -> Result<PublishPlan> {
    build_plan_inner(repo_root, publish, false, package_name)
}

fn build_plan_inner(
    repo_root: &Path,
    publish: &PublishConfig,
    dry_run: bool,
    package_name: Option<&str>,
) -> Result<PublishPlan> {
    if !publish.enabled {
        bail!("publish flow is disabled; set [publish].enabled = true to use release publish");
    }

    let provider = publish.provider.trim();
    let repository = publish.repository.trim();
    let repository_url = publish
        .repository_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let mut command = Vec::new();
    let mut env_pairs = Vec::new();
    let mut dist_files = Vec::new();

    let use_oidc = publish.oidc && !dry_run && publish.token_env.is_none() && oidc_env_available();

    let oidc_token = if use_oidc {
        Some(exchange_oidc_token()?)
    } else {
        None
    };

    match provider {
        "uv" => {
            dist_files = collect_dist_files(repo_root, &publish.dist_dir, package_name)?;
            command.push("uv".into());
            command.push("publish".into());

            if repository_url.is_none() && repository != "pypi" {
                command.push("--index".into());
                command.push(repository.into());
            }

            if let Some(url) = &repository_url {
                env_pairs.push(("UV_PUBLISH_URL".to_string(), url.clone()));
            }

            if let Some(token) = &oidc_token {
                command.push("--token".into());
                command.push(token.into());
            } else {
                append_auth_envs(publish, &mut env_pairs, "UV_PUBLISH_")?;
            }
            command.extend(dist_files.iter().map(|path| path.as_os_str().to_owned()));
        }
        "twine" => {
            dist_files = collect_dist_files(repo_root, &publish.dist_dir, package_name)?;
            command.push("twine".into());
            command.push("upload".into());
            command.push("--non-interactive".into());

            if let Some(url) = &repository_url {
                command.push("--repository-url".into());
                command.push(url.into());
            } else if repository != "pypi" {
                command.push("--repository".into());
                command.push(repository.into());
            }

            if let Some(token) = &oidc_token {
                env_pairs.push(("TWINE_USERNAME".to_string(), "__token__".to_string()));
                env_pairs.push(("TWINE_PASSWORD".to_string(), token.clone()));
            } else {
                append_auth_envs(publish, &mut env_pairs, "TWINE_")?;
            }
            command.extend(dist_files.iter().map(|path| path.as_os_str().to_owned()));
        }
        "cargo" => {
            command.push("cargo".into());
            command.push("publish".into());
            command.push("--locked".into());

            if repository != "crates-io" {
                command.push("--registry".into());
                command.push(repository.into());
            }
        }
        "goreleaser" => {
            command.push("goreleaser".into());
            command.push("release".into());
            command.push("--clean".into());
        }
        _ => bail!("unsupported publish provider `{provider}`"),
    }

    let trusted_publishing = publish.trusted_publishing || (publish.oidc && oidc_env_available());

    Ok(PublishPlan {
        provider: provider.to_string(),
        repository: repository.to_string(),
        repository_url,
        dist_files,
        command,
        env: env_pairs,
        trusted_publishing,
    })
}

impl PublishPlan {
    pub fn target_label(&self) -> String {
        match &self.repository_url {
            Some(url) => format!("{} ({url})", self.repository),
            None => self.repository.clone(),
        }
    }
}

const PYPI_OIDC_MINT_URL: &str = "https://pypi.org/_/oidc/mint-token";

fn oidc_env_available() -> bool {
    env::var("ACTIONS_ID_TOKEN_REQUEST_URL").is_ok()
        && env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").is_ok()
}

#[derive(Serialize)]
struct OidcMintRequest {
    token: String,
}

#[derive(Deserialize)]
struct OidcMintResponse {
    token: String,
}

fn exchange_oidc_token() -> Result<String> {
    let request_url =
        env::var("ACTIONS_ID_TOKEN_REQUEST_URL").context("ACTIONS_ID_TOKEN_REQUEST_URL not set")?;
    let request_token = env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .context("ACTIONS_ID_TOKEN_REQUEST_TOKEN not set")?;

    let url = format!("{request_url}&audience=pypi");
    let gh_response: serde_json::Value = ureq::get(&url)
        .set("Authorization", &format!("Bearer {request_token}"))
        .call()
        .context("failed to request OIDC token from GitHub")?
        .into_json()
        .context("failed to parse GitHub OIDC token response")?;

    let oidc_token = gh_response["value"]
        .as_str()
        .context("GitHub OIDC response missing 'value' field")?
        .to_string();

    let mint_response: OidcMintResponse = ureq::post(PYPI_OIDC_MINT_URL)
        .send_json(&OidcMintRequest { token: oidc_token })
        .context("failed to exchange OIDC token with PyPI")?
        .into_json()
        .context("failed to parse PyPI OIDC mint response")?;

    Ok(mint_response.token)
}

fn collect_dist_files(
    repo_root: &Path,
    dist_dir: &str,
    package_name: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let dist_path = repo_root.join(dist_dir);
    let entries = fs::read_dir(&dist_path).with_context(|| {
        format!(
            "failed to read publish artifacts from {}",
            dist_path.display()
        )
    })?;
    let mut files = Vec::new();

    for entry in entries {
        let path = entry?.path();
        if path.is_file() {
            if let Some(package_name) = package_name {
                let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !artifact_matches_package(file_name, package_name) {
                    continue;
                }
            }
            files.push(path);
        }
    }

    files.sort();

    if files.is_empty() {
        bail!("no publish artifacts found in {}", dist_path.display());
    }

    Ok(files)
}

fn artifact_matches_package(file_name: &str, package_name: &str) -> bool {
    let normalized_file = file_name.to_ascii_lowercase();
    let normalized_package = package_name.to_ascii_lowercase();

    let mut pattern = String::with_capacity(normalized_package.len() * 2 + 4);
    for ch in normalized_package.chars() {
        match ch {
            '-' | '_' | '.' => pattern.push('-'),
            other => pattern.push(other),
        }
    }

    let mut candidate = String::with_capacity(normalized_file.len());
    for ch in normalized_file.chars() {
        match ch {
            '-' | '_' | '.' => candidate.push('-'),
            other => candidate.push(other),
        }
    }

    let Some(rest) = candidate.strip_prefix(&pattern) else {
        return false;
    };

    matches!(rest.chars().next(), Some('-')) && matches!(rest.chars().nth(1), Some('0'..='9'))
}

fn append_auth_envs(
    publish: &PublishConfig,
    env_pairs: &mut Vec<(String, String)>,
    prefix: &str,
) -> Result<()> {
    let bindings = [
        ("USERNAME", publish.username_env.as_deref()),
        ("PASSWORD", publish.password_env.as_deref()),
        ("TOKEN", publish.token_env.as_deref()),
    ];

    for (suffix, source_env) in bindings {
        let Some(source_env) = source_env else {
            continue;
        };
        let source_env = source_env.trim();
        let value = env::var(source_env)
            .with_context(|| format!("missing publish credential env var {source_env}"))?;
        env_pairs.push((format!("{prefix}{suffix}"), value));
    }

    Ok(())
}

fn command_from_plan(plan: &PublishPlan) -> Command {
    let mut command = Command::new(&plan.command[0]);
    command.args(plan.command.iter().skip(1));
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    command
}

fn render_command(args: &[OsString]) -> String {
    args.iter().map(shell_escape).collect::<Vec<_>>().join(" ")
}

fn shell_escape(arg: &OsString) -> String {
    let value = arg.to_string_lossy();
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':'))
    {
        value.into_owned()
    } else {
        format!("{value:?}")
    }
}

/// Check if a package/version is already published to the registry
fn check_already_published(package_name: &str, version: &Version, provider: &str) -> Result<bool> {
    match provider {
        "uv" | "twine" => {
            // For Python packages, check PyPI
            pypi::has_version(package_name, version)
                .with_context(|| format!("failed to check PyPI for {package_name} {version}"))
        }
        "cargo" => {
            // For Rust, we'd check crates.io - for now, assume not published
            // TODO: Implement crates.io check using src/cratesio/mod.rs
            Ok(false)
        }
        _ => {
            // For other providers, we can't check, so assume not published
            Ok(false)
        }
    }
}

/// Get the current version for a single-package repo
fn get_current_version(repo_root: &Path, config: &Config) -> Option<Version> {
    analysis::read_current_version(repo_root, &config.version_files)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
}

/// Get the package name for a single-package repo
fn get_package_name(repo_root: &Path, _package_root: &str) -> Option<String> {
    // For single-package repos, try to read from pyproject.toml or Cargo.toml
    let pyproject_path = repo_root.join("pyproject.toml");
    if pyproject_path.exists() {
        let contents = fs::read_to_string(pyproject_path).ok()?;
        let parsed = contents.parse::<toml::Table>().ok()?;
        return parsed
            .get("project")?
            .as_table()?
            .get("name")?
            .as_str()
            .map(ToString::to_string);
    }

    let cargo_path = repo_root.join("Cargo.toml");
    if cargo_path.exists() {
        let contents = fs::read_to_string(cargo_path).ok()?;
        let parsed = contents.parse::<toml::Table>().ok()?;
        return parsed
            .get("package")?
            .as_table()?
            .get("name")?
            .as_str()
            .map(ToString::to_string);
    }

    None
}
