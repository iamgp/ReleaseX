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
    analysis::ReleaseAnalysis,
    config::{Config, PublishConfig},
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

pub fn execute(repo_root: &Path, config: &Config) -> Result<()> {
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
) -> Result<()> {
    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        bail!("no releasable packages found in monorepo");
    }

    for package in &selected {
        let package_root = repo_root.join(&package.root);
        let plan = build_plan(&package_root, &config.publish)?;
        let mut command = command_from_plan(&plan);
        let status = command
            .current_dir(&package_root)
            .status()
            .with_context(|| {
                format!(
                    "failed to launch {} publish command for {}",
                    plan.provider, package.name
                )
            })?;

        if !status.success() {
            bail!(
                "{} publish failed for {} with status {}",
                plan.provider,
                package.name,
                status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            );
        }

        println!(
            "Published {} ({} artifact(s)) with {} to {}",
            package.name,
            plan.dist_files.len(),
            plan.provider,
            plan.target_label()
        );
    }

    Ok(())
}

pub fn print_dry_run(repo_root: &Path, config: &Config) -> Result<()> {
    let plan = build_plan_dry_run(repo_root, &config.publish)?;

    println!("Publish is enabled: {}", config.publish.enabled);
    println!("Provider: {}", plan.provider);
    println!("Target repository: {}", plan.target_label());
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
    build_plan_inner(repo_root, publish, false)
}

fn build_plan_dry_run(repo_root: &Path, publish: &PublishConfig) -> Result<PublishPlan> {
    build_plan_inner(repo_root, publish, true)
}

fn build_plan_inner(
    repo_root: &Path,
    publish: &PublishConfig,
    dry_run: bool,
) -> Result<PublishPlan> {
    if !publish.enabled {
        bail!("publish flow is disabled; set [publish].enabled = true to use release publish");
    }

    let dist_files = collect_dist_files(repo_root, &publish.dist_dir)?;
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

    let use_oidc = publish.oidc && !dry_run && publish.token_env.is_none() && oidc_env_available();

    let oidc_token = if use_oidc {
        Some(exchange_oidc_token()?)
    } else {
        None
    };

    match provider {
        "uv" => {
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

fn collect_dist_files(repo_root: &Path, dist_dir: &str) -> Result<Vec<PathBuf>> {
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
            files.push(path);
        }
    }

    files.sort();

    if files.is_empty() {
        bail!("no publish artifacts found in {}", dist_path.display());
    }

    Ok(files)
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
