use std::{env, path::Path, process::Command};

use anyhow::Result;
use console::style;

use crate::{
    analysis::{detect_project_name, read_current_version},
    config::{Config, Ecosystem},
    cratesio, ecosystem,
    git::{GitRepository, run_git},
    github, pypi,
    version::Version,
};

use super::{Cli, HealthcheckArgs};

#[derive(Debug)]
enum CheckResult {
    Pass(String),
    Warn(String),
    Fail(String),
}

struct HealthcheckReport {
    categories: Vec<(String, Vec<CheckResult>)>,
}

impl HealthcheckReport {
    fn new() -> Self {
        Self {
            categories: Vec::new(),
        }
    }

    fn add_category(&mut self, name: &str, checks: Vec<CheckResult>) {
        self.categories.push((name.to_string(), checks));
    }

    fn print(&self) {
        println!();
        println!("{}", style("relx healthcheck").bold());

        for (category, checks) in &self.categories {
            println!();
            println!(" {}", style(category).cyan().bold());
            for check in checks {
                match check {
                    CheckResult::Pass(msg) => {
                        println!(" {} {}", style("✓").green(), msg);
                    }
                    CheckResult::Warn(msg) => {
                        println!(" {} {}", style("⚠").yellow(), msg);
                    }
                    CheckResult::Fail(msg) => {
                        println!(" {} {}", style("✗").red(), msg);
                    }
                }
            }
        }

        let (warnings, errors) = self.counts();
        println!();
        println!(" {} warning(s), {} error(s).", warnings, errors);
    }

    fn counts(&self) -> (usize, usize) {
        let mut warnings = 0;
        let mut errors = 0;
        for (_, checks) in &self.categories {
            for check in checks {
                match check {
                    CheckResult::Warn(_) => warnings += 1,
                    CheckResult::Fail(_) => errors += 1,
                    CheckResult::Pass(_) => {}
                }
            }
        }
        (warnings, errors)
    }

    fn exit_code(&self) -> i32 {
        let (warnings, errors) = self.counts();
        if errors > 0 {
            1
        } else if warnings > 0 {
            2
        } else {
            0
        }
    }
}

const CATEGORIES: &[&str] = &["config", "git", "github", "build", "registry"];

fn normalize_category(category: &str) -> &str {
    match category {
        "pypi" => "registry",
        other => other,
    }
}

pub fn run(cli: &Cli, args: &HealthcheckArgs) -> Result<()> {
    if let Some(only) = &args.only {
        let lower = only.to_lowercase();
        let normalized = normalize_category(&lower);
        if !CATEGORIES.contains(&normalized) {
            anyhow::bail!(
                "unknown category `{only}`. Valid categories: {}",
                CATEGORIES.join(", ")
            );
        }
    }

    let filter = args
        .only
        .as_deref()
        .map(|s| normalize_category(&s.to_lowercase()).to_string());
    let should_run = |cat: &str| filter.as_deref().is_none() || filter.as_deref() == Some(cat);

    let config = Config::load(&cli.config_path()).ok();
    let repo = GitRepository::discover(".").ok();

    let mut report = HealthcheckReport::new();

    if should_run("config") {
        report.add_category("Config", check_config(cli, &config));
    }

    if should_run("git") {
        report.add_category("Git", check_git(&config, &repo));
    }

    if should_run("github") {
        report.add_category("GitHub", check_github(&config, &repo));
    }

    if should_run("build") {
        report.add_category("Build", check_build(&config));
    }

    if should_run("registry") {
        report.add_category("Registry", check_registry(&config, &repo));
    }

    report.print();

    let code = report.exit_code();
    if code != 0 {
        std::process::exit(code);
    }

    Ok(())
}

fn check_config(cli: &Cli, config: &Option<Config>) -> Vec<CheckResult> {
    let mut checks = Vec::new();

    match config {
        Some(cfg) => {
            let ecosystem = ecosystem::detect(Path::new("."), Some(cfg));
            checks.push(CheckResult::Pass(format!(
                "{} is valid",
                cli.config_path().display()
            )));

            let manifest = ecosystem::manifest_name(ecosystem);
            if Path::new(manifest).exists() {
                checks.push(CheckResult::Pass(format!("{manifest} found")));
            } else {
                checks.push(CheckResult::Warn(format!("{manifest} not found")));
            }

            match read_current_version(Path::new("."), &cfg.version_files) {
                Ok(Some(version_str)) => match version_str.parse::<Version>() {
                    Ok(v) => {
                        checks.push(CheckResult::Pass(format!(
                            "project.version = {} (PEP 440 valid)",
                            v
                        )));
                    }
                    Err(_) => {
                        checks.push(CheckResult::Fail(format!(
                            "project.version = {} (not PEP 440 valid)",
                            version_str
                        )));
                    }
                },
                Ok(None) => {
                    checks.push(CheckResult::Warn(
                        "project.version not found in version files".to_string(),
                    ));
                }
                Err(e) => {
                    checks.push(CheckResult::Fail(format!(
                        "failed to read project.version: {}",
                        e
                    )));
                }
            }
        }
        None => {
            checks.push(CheckResult::Fail(format!(
                "{} could not be loaded",
                cli.config_path().display()
            )));
        }
    }

    checks
}

fn check_git(config: &Option<Config>, repo: &Option<GitRepository>) -> Vec<CheckResult> {
    let mut checks = Vec::new();

    let repo = match repo {
        Some(r) => r,
        None => {
            checks.push(CheckResult::Fail("Not inside a git repository".to_string()));
            return checks;
        }
    };

    match run_git(repo.path(), ["status", "--porcelain"]) {
        Ok(output) => {
            if output.trim().is_empty() {
                checks.push(CheckResult::Pass("Working tree is clean".to_string()));
            } else {
                checks.push(CheckResult::Warn(
                    "Working tree has uncommitted changes".to_string(),
                ));
            }
        }
        Err(_) => {
            checks.push(CheckResult::Warn(
                "Could not check working tree status".to_string(),
            ));
        }
    }

    match repo.current_branch() {
        Ok(branch) => {
            let expected = config
                .as_ref()
                .map(|c| c.release.branch.as_str())
                .unwrap_or("main");
            if branch == expected {
                checks.push(CheckResult::Pass(format!("On branch {}", branch)));
            } else {
                checks.push(CheckResult::Warn(format!(
                    "On branch {} (expected {})",
                    branch, expected
                )));
            }
        }
        Err(_) => {
            checks.push(CheckResult::Warn(
                "Could not determine current branch".to_string(),
            ));
        }
    }

    match run_git(repo.path(), ["ls-remote", "--exit-code", "origin"]) {
        Ok(_) => {
            checks.push(CheckResult::Pass("Remote origin is reachable".to_string()));
        }
        Err(_) => {
            checks.push(CheckResult::Fail(
                "Remote origin is not reachable".to_string(),
            ));
        }
    }

    match repo.commits_since_latest_tag() {
        Ok(commits) => {
            if commits.is_empty() {
                checks.push(CheckResult::Warn(
                    "No commits since last tag (nothing to release)".to_string(),
                ));
            } else {
                checks.push(CheckResult::Pass(format!(
                    "{} commit(s) since last tag",
                    commits.len()
                )));
            }
        }
        Err(_) => {
            checks.push(CheckResult::Warn(
                "Could not determine commits since last tag".to_string(),
            ));
        }
    }

    checks
}

fn check_github(config: &Option<Config>, repo: &Option<GitRepository>) -> Vec<CheckResult> {
    let mut checks = Vec::new();

    let token_env = config
        .as_ref()
        .map(|c| c.github.token_env.as_str())
        .unwrap_or("GITHUB_TOKEN");

    let token = env::var(token_env);
    match &token {
        Ok(_) => {
            checks.push(CheckResult::Pass(format!("{} is set", token_env)));
        }
        Err(_) => {
            checks.push(CheckResult::Fail(format!("{} is not set", token_env)));
            return checks;
        }
    }

    let token = token.unwrap();
    let config = match config {
        Some(c) => c,
        None => return checks,
    };
    let repo = match repo {
        Some(r) => r,
        None => return checks,
    };

    match github::detect_repo(repo, &config.github) {
        Ok(repo_ref) => {
            if let Ok(client) =
                github::GitHubClient::new(&config.github.api_base, &token, repo_ref.clone())
                && let Ok(scopes) = client.token_scopes()
            {
                if scopes.is_empty() {
                    checks.push(CheckResult::Warn(
                        "GitHub token scopes could not be determined".to_string(),
                    ));
                } else {
                    for required in ["contents", "pull_requests", "repo"] {
                        if scopes.iter().any(|scope| scope == required) {
                            checks.push(CheckResult::Pass(format!(
                                "GitHub token exposes {} scope",
                                required
                            )));
                        }
                    }
                    if !scopes
                        .iter()
                        .any(|scope| scope == "repo" || scope == "contents")
                    {
                        checks.push(CheckResult::Warn(
                            "GitHub token does not advertise a contents/repo scope".to_string(),
                        ));
                    }
                }
            }

            let url = format!(
                "{}/repos/{}/{}",
                config.github.api_base.trim_end_matches('/'),
                repo_ref.owner,
                repo_ref.name
            );
            match ureq::get(&url)
                .set("Authorization", &format!("Bearer {}", token))
                .set("User-Agent", "relx")
                .call()
            {
                Ok(_) => {
                    checks.push(CheckResult::Pass(format!(
                        "GitHub repo {}/{} is reachable",
                        repo_ref.owner, repo_ref.name
                    )));
                }
                Err(_) => {
                    checks.push(CheckResult::Fail(format!(
                        "GitHub repo {}/{} is not reachable",
                        repo_ref.owner, repo_ref.name
                    )));
                }
            }
        }
        Err(_) => {
            checks.push(CheckResult::Warn(
                "Could not detect GitHub repository".to_string(),
            ));
        }
    }

    checks
}

fn check_build(config: &Option<Config>) -> Vec<CheckResult> {
    let mut checks = Vec::new();

    let config = match config {
        Some(c) => c,
        None => {
            checks.push(CheckResult::Warn(
                "Config not loaded, skipping build checks".to_string(),
            ));
            return checks;
        }
    };
    let ecosystem = ecosystem::detect(Path::new("."), Some(config));
    let build_backend = if ecosystem == Ecosystem::Python {
        ecosystem::python_build_backend(Path::new("."))
    } else {
        None
    };

    let tool_check = ecosystem::tool_check_command(ecosystem, Some(config.publish.provider.trim()));
    let tool_name = tool_check.first().copied().unwrap_or("tool");
    if Command::new(tool_name)
        .args(&tool_check[1..])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        checks.push(CheckResult::Pass(format!("{tool_name} is installed")));
    } else {
        checks.push(CheckResult::Fail(format!("{tool_name} is not installed")));
    }

    let manifest = ecosystem::manifest_name(ecosystem);
    if !Path::new(manifest).exists() {
        checks.push(CheckResult::Warn(format!(
            "{manifest} not found (build may fail)"
        )));
        return checks;
    }

    checks.push(CheckResult::Pass(format!("{manifest} exists")));

    let build_cmd = ecosystem::healthcheck_command(ecosystem, build_backend.as_deref());
    let build_program = build_cmd.first().copied().unwrap_or("build");
    if Command::new(build_program)
        .args(&build_cmd[1..])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        checks.push(CheckResult::Pass(format!(
            "{} succeeds",
            build_cmd.join(" ")
        )));
    } else {
        checks.push(CheckResult::Warn(format!("{} failed", build_cmd.join(" "))));
    }

    checks
}

fn check_registry(config: &Option<Config>, repo: &Option<GitRepository>) -> Vec<CheckResult> {
    let mut checks = Vec::new();

    let config = match config {
        Some(c) => c,
        None => return checks,
    };
    let ecosystem = ecosystem::detect(Path::new("."), Some(config));

    if ecosystem == Ecosystem::Go {
        checks.push(CheckResult::Pass(
            "go ecosystem detected, skipping registry version checks".to_string(),
        ));
        return checks;
    }

    if !config.publish.enabled {
        checks.push(CheckResult::Pass(
            "Publishing is disabled, skipping registry checks".to_string(),
        ));
        return checks;
    }

    if let Some(repo) = repo {
        match (
            read_current_version(Path::new("."), &config.version_files),
            repo.latest_tag(),
        ) {
            (Ok(Some(version_str)), Ok(Some(tag))) => {
                let parsed = version_str.parse::<Version>().ok();
                let expected_tag = format!("{}{}", config.release.tag_prefix, version_str);
                if tag == expected_tag {
                    checks.push(CheckResult::Fail(format!(
                        "Version {} is already tagged ({})",
                        version_str, tag
                    )));
                } else {
                    checks.push(CheckResult::Pass(format!(
                        "Version {} is not yet tagged",
                        version_str
                    )));
                }

                if let (Some(version), Some(project_name)) =
                    (parsed, detect_project_name(repo.path(), "."))
                {
                    match ecosystem {
                        Ecosystem::Python => match pypi::has_version(&project_name, &version) {
                            Ok(true) => checks.push(CheckResult::Fail(format!(
                                "Version {} is already published on PyPI",
                                version
                            ))),
                            Ok(false) => checks.push(CheckResult::Pass(format!(
                                "Version {} is not yet published on PyPI",
                                version
                            ))),
                            Err(_) => checks.push(CheckResult::Warn(
                                "Could not query the Python package registry for the current version"
                                    .to_string(),
                            )),
                        },
                        Ecosystem::Rust => match cratesio::has_version(&project_name, &version) {
                            Ok(true) => checks.push(CheckResult::Fail(format!(
                                "Version {} is already published on crates.io",
                                version
                            ))),
                            Ok(false) => checks.push(CheckResult::Pass(format!(
                                "Version {} is not yet published on crates.io",
                                version
                            ))),
                            Err(_) => checks.push(CheckResult::Warn(
                                "Could not query crates.io for the current version".to_string(),
                            )),
                        },
                        Ecosystem::Go => {}
                    }
                }
            }
            (Ok(Some(version_str)), Ok(None)) => {
                checks.push(CheckResult::Pass(format!(
                    "Version {} is not yet tagged (no tags exist)",
                    version_str
                )));
            }
            _ => {
                checks.push(CheckResult::Warn(
                    "Could not check version tag status".to_string(),
                ));
            }
        }
    }

    match ecosystem {
        Ecosystem::Python => {
            let has_credentials = config.publish.oidc
                || config.publish.trusted_publishing
                || config.publish.token_env.is_some()
                || config.publish.username_env.is_some();

            if has_credentials {
                checks.push(CheckResult::Pass(
                    "PyPI credentials or OIDC are configured".to_string(),
                ));
            } else {
                checks.push(CheckResult::Warn(
                    "No PyPI credentials or OIDC are configured".to_string(),
                ));
            }

            if config.publish.oidc || config.publish.trusted_publishing {
                if env::var("ACTIONS_ID_TOKEN_REQUEST_URL").is_ok()
                    && env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").is_ok()
                {
                    checks.push(CheckResult::Pass(
                        "OIDC environment is available".to_string(),
                    ));
                } else {
                    checks.push(CheckResult::Warn(
                        "OIDC trusted publisher could not be validated outside GitHub Actions"
                            .to_string(),
                    ));
                }
            }
        }
        Ecosystem::Rust => {
            let token_env = config
                .publish
                .token_env
                .as_deref()
                .unwrap_or("CARGO_REGISTRY_TOKEN");
            if env::var(token_env).is_ok() {
                checks.push(CheckResult::Pass(format!(
                    "{} is set for crates.io publishing",
                    token_env
                )));
            } else {
                checks.push(CheckResult::Warn(format!(
                    "{} is not set for crates.io publishing",
                    token_env
                )));
            }
        }
        Ecosystem::Go => {}
    }

    checks
}
