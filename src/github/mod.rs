use std::{env, path::Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::tempdir;

use crate::{
    analysis::{self, ReleaseAnalysis},
    changelog,
    config::{Config, GitHubConfig},
    git::{GitRepository, run_git},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePrPlan {
    pub version: String,
    pub branch: String,
    pub base: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub release_notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTagPlan {
    pub version: String,
    pub tag_name: String,
    pub title: String,
    pub target: String,
    pub release_notes: String,
    pub label: String,
}

pub fn build_release_pr_plan(config: &Config, analysis: &ReleaseAnalysis) -> Result<ReleasePrPlan> {
    let release_label = release_label(analysis)?;
    let version = analysis
        .next_version
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| release_label.clone());
    let title = if config.monorepo.enabled {
        monorepo_pr_title(config, analysis)?
    } else {
        config
            .release
            .pr_title
            .replace("{version}", &format!("v{version}"))
    };
    let branch = format!(
        "{}/{}",
        config.github.release_branch_prefix.trim_end_matches('/'),
        release_branch_suffix(analysis)?
    );
    let date = today_utc();
    let release_notes = changelog::render_release_notes(&release_label, &date, &analysis.changelog);
    let body = format!(
        "## Release summary\n\n{release_notes}\n\n## Maintainer checklist\n- [ ] Review version bump\n- [ ] Review changelog\n- [ ] Merge to cut the release"
    );

    Ok(ReleasePrPlan {
        version,
        branch,
        base: config.release.branch.clone(),
        title,
        body,
        labels: vec![config.github.pending_label.clone()],
        release_notes,
    })
}

pub fn build_release_tag_plan(
    config: &Config,
    repo: &GitRepository,
    analysis: &ReleaseAnalysis,
) -> Result<ReleaseTagPlan> {
    let release_label = release_label(analysis)?;
    let version = analysis
        .next_version
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| release_label.clone());
    let tag_name = if config.monorepo.enabled {
        format!(
            "{}{}",
            config.release.tag_prefix,
            sanitize_label(&release_label)
        )
    } else {
        format!("{}{}", config.release.tag_prefix, version)
    };
    Ok(ReleaseTagPlan {
        version,
        title: format!("Release {tag_name}"),
        target: repo.current_branch()?,
        release_notes: changelog::render_release_notes(
            &tag_name,
            &today_utc(),
            &analysis.changelog,
        ),
        tag_name,
        label: config.github.tagged_label.clone(),
    })
}

pub fn detect_repo(repo: &GitRepository, github: &GitHubConfig) -> Result<RepoRef> {
    if let (Some(owner), Some(name)) = (&github.owner, &github.repo) {
        return Ok(RepoRef {
            owner: owner.clone(),
            name: name.clone(),
        });
    }

    let remote = repo
        .remote_url("origin")?
        .context("unable to detect GitHub repo: set [github].owner and [github].repo or add an origin remote")?;
    parse_remote_url(&remote).context("failed to parse GitHub remote")
}

pub fn execute_release_pr(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let plan = build_release_pr_plan(config, analysis)?;
    let repo_ref = detect_repo(repo, &config.github)?;
    let token = env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    let client = GitHubClient::new(&config.github.api_base, &token, repo_ref)?;

    let clone_dir = tempdir().context("failed to create temporary workspace")?;
    let clone_path = clone_dir.path().join("repo");
    let origin_url = repo
        .remote_url("origin")?
        .context("origin remote is required for release PR flow")?;

    run_git(
        clone_dir.path(),
        vec![
            "clone".into(),
            repo.path().as_os_str().to_owned(),
            clone_path.as_os_str().to_owned(),
        ],
    )?;
    run_git(
        &clone_path,
        ["remote", "set-url", "origin", origin_url.as_str().as_ref()],
    )?;
    run_git(&clone_path, ["fetch", "origin", plan.base.as_str()])?;
    run_git(
        &clone_path,
        [
            "checkout",
            "-B",
            plan.branch.as_str(),
            format!("origin/{}", plan.base).as_str(),
        ],
    )?;

    analysis::update_version_files(
        &clone_path,
        &config.version_files,
        analysis.next_version.as_ref().unwrap(),
    )?;
    changelog::prepend_release_notes(
        &clone_path.join(&config.release.changelog_file),
        &plan.release_notes,
    )?;

    run_git(&clone_path, ["add", "."])?;
    let diff = run_git(&clone_path, ["status", "--short"])?;
    if diff.trim().is_empty() {
        bail!("release PR would not change any files");
    }

    run_git(
        &clone_path,
        [
            "-c",
            "user.name=pyrls",
            "-c",
            "user.email=pyrls@users.noreply.github.com",
            "commit",
            "-m",
            plan.title.as_str(),
        ],
    )?;
    run_git(
        &clone_path,
        [
            "push",
            "--force-with-lease",
            "origin",
            format!("HEAD:{}", plan.branch).as_str(),
        ],
    )?;

    let pr = match client.find_open_pr(&plan.branch, &plan.base)? {
        Some(existing) => client.update_pr(existing.number, &plan.title, &plan.body)?,
        None => client.create_pr(&plan.title, &plan.branch, &plan.base, &plan.body)?,
    };

    for label in &plan.labels {
        client.ensure_label(label)?;
    }
    client.add_labels(pr.number, &plan.labels)?;

    println!("Release PR ready: #{} {}", pr.number, plan.title);
    println!("Branch: {}", plan.branch);
    Ok(())
}

pub fn execute_release_tag(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let plan = build_release_tag_plan(config, repo, analysis)?;
    let repo_ref = detect_repo(repo, &config.github)?;
    let token = env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    let client = GitHubClient::new(&config.github.api_base, &token, repo_ref)?;

    run_git(
        repo.path(),
        [
            "tag",
            "-a",
            plan.tag_name.as_str(),
            "-m",
            plan.title.as_str(),
        ],
    )?;
    run_git(repo.path(), ["push", "origin", plan.tag_name.as_str()])?;

    match client.find_release_by_tag(&plan.tag_name)? {
        Some(existing) => {
            client.update_release(existing.id, &plan.title, &plan.release_notes)?;
        }
        None => {
            client.create_release(
                &plan.tag_name,
                &plan.title,
                &plan.release_notes,
                &config.release.branch,
            )?;
        }
    }

    println!("Release tagged: {}", plan.tag_name);
    Ok(())
}

pub fn print_release_pr_dry_run(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let repo_ref = detect_repo(repo, &config.github)?;
    let plan = build_release_pr_plan(config, analysis)?;
    if config.monorepo.enabled {
        let selected = selected_package_summaries(analysis);
        println!(
            "Would create or update {} release PR set covering: {}",
            analysis.package_plan.release_mode,
            selected.join(", ")
        );
    }
    println!(
        "Would push release branch `{}` from `{}`",
        plan.branch, plan.base
    );
    println!("Would update version files to {}", plan.version);
    println!("Would prepend {} with:", config.release.changelog_file);
    println!("{}", indent_block(&plan.release_notes, "  "));
    println!(
        "Would create or update PR `{}` in {}/{}",
        plan.title, repo_ref.owner, repo_ref.name
    );
    println!("Would apply labels: {}", plan.labels.join(", "));
    Ok(())
}

pub fn print_release_tag_dry_run(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let repo_ref = detect_repo(repo, &config.github)?;
    let plan = build_release_tag_plan(config, repo, analysis)?;
    if config.monorepo.enabled {
        println!(
            "Would tag selected package set for {} mode: {}",
            analysis.package_plan.release_mode,
            selected_package_summaries(analysis).join(", ")
        );
    }
    println!(
        "Would create and push tag `{}` to {}/{}",
        plan.tag_name, repo_ref.owner, repo_ref.name
    );
    println!("Would create or update GitHub Release `{}`", plan.title);
    println!("{}", indent_block(&plan.release_notes, "  "));
    Ok(())
}

fn indent_block(value: &str, prefix: &str) -> String {
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn selected_package_summaries(analysis: &ReleaseAnalysis) -> Vec<String> {
    analysis
        .package_plan
        .selected_packages()
        .into_iter()
        .map(|package| {
            format!(
                "{} {}",
                package.name,
                package
                    .next_version
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "unchanged".to_string())
            )
        })
        .collect()
}

fn release_label(analysis: &ReleaseAnalysis) -> Result<String> {
    if analysis.package_plan.release_mode == "single" {
        return Ok(analysis
            .next_version
            .as_ref()
            .context("no release is pending from the current commit set")?
            .to_string());
    }

    let selected = selected_package_summaries(analysis);
    if selected.is_empty() {
        bail!("no releasable package set is pending from the current commit set");
    }
    Ok(selected.join(", "))
}

fn release_branch_suffix(analysis: &ReleaseAnalysis) -> Result<String> {
    if analysis.package_plan.release_mode == "single" {
        return Ok(format!(
            "v{}",
            analysis
                .next_version
                .as_ref()
                .context("no release is pending from the current commit set")?
        ));
    }

    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        bail!("no releasable package set is pending from the current commit set");
    }

    if analysis.package_plan.release_mode == "unified" {
        return Ok(format!(
            "monorepo/{}",
            sanitize_label(&selected_package_summaries(analysis).join("-"))
        ));
    }

    Ok(format!("per-package/{}", selected.len()))
}

fn monorepo_pr_title(config: &Config, analysis: &ReleaseAnalysis) -> Result<String> {
    let selected = selected_package_summaries(analysis);
    if selected.is_empty() {
        bail!("no releasable package set is pending from the current commit set");
    }

    if analysis.package_plan.release_mode == "unified" {
        return Ok(format!("chore(release): {}", selected.join(", ")));
    }

    Ok(format!(
        "{} package release set",
        config
            .release
            .pr_title
            .replace("{version}", &format!("{} packages", selected.len()))
    ))
}

fn sanitize_label(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch.to_ascii_lowercase(),
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn today_utc() -> String {
    run_git(Path::new("."), ["show", "-s", "--format=%cs", "HEAD"])
        .unwrap_or_else(|_| "1970-01-01".to_string())
}

pub fn parse_remote_url(value: &str) -> Option<RepoRef> {
    let trimmed = value.trim().trim_end_matches(".git");
    let cleaned = trimmed
        .strip_prefix("git@github.com:")
        .or_else(|| trimmed.strip_prefix("ssh://git@github.com/"))
        .or_else(|| trimmed.strip_prefix("https://github.com/"))
        .or_else(|| trimmed.strip_prefix("http://github.com/"))?;
    let mut parts = cleaned.split('/');
    let owner = parts.next()?.to_string();
    let name = parts.next()?.to_string();
    Some(RepoRef { owner, name })
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    number: u64,
}

#[derive(Debug, Deserialize)]
struct Release {
    id: u64,
}

struct GitHubClient {
    api_base: String,
    token: String,
    repo: RepoRef,
}

impl GitHubClient {
    fn new(api_base: &str, token: &str, repo: RepoRef) -> Result<Self> {
        Ok(Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            repo,
        })
    }

    fn find_open_pr(&self, head_branch: &str, base_branch: &str) -> Result<Option<PullRequest>> {
        let url = format!(
            "{}/repos/{}/{}/pulls?state=open&head={}:{}&base={}",
            self.api_base,
            self.repo.owner,
            self.repo.name,
            self.repo.owner,
            head_branch,
            base_branch
        );
        let prs: Vec<PullRequest> = self.get(&url)?;
        Ok(prs.into_iter().next())
    }

    fn create_pr(&self, title: &str, head: &str, base: &str, body: &str) -> Result<PullRequest> {
        self.post(
            &format!(
                "{}/repos/{}/{}/pulls",
                self.api_base, self.repo.owner, self.repo.name
            ),
            &json!({ "title": title, "head": head, "base": base, "body": body }),
        )
    }

    fn update_pr(&self, number: u64, title: &str, body: &str) -> Result<PullRequest> {
        self.patch(
            &format!(
                "{}/repos/{}/{}/pulls/{}",
                self.api_base, self.repo.owner, self.repo.name, number
            ),
            &json!({ "title": title, "body": body }),
        )
    }

    fn ensure_label(&self, name: &str) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/labels/{}",
            self.api_base, self.repo.owner, self.repo.name, name
        );
        if self.get_raw(&url).is_ok() {
            return Ok(());
        }

        let _: serde_json::Value = self.post(
            &format!(
                "{}/repos/{}/{}/labels",
                self.api_base, self.repo.owner, self.repo.name
            ),
            &json!({ "name": name, "color": "ededed", "description": "Managed by pyrls" }),
        )?;
        Ok(())
    }

    fn add_labels(&self, number: u64, labels: &[String]) -> Result<()> {
        let _: serde_json::Value = self.post(
            &format!(
                "{}/repos/{}/{}/issues/{}/labels",
                self.api_base, self.repo.owner, self.repo.name, number
            ),
            &json!({ "labels": labels }),
        )?;
        Ok(())
    }

    fn find_release_by_tag(&self, tag: &str) -> Result<Option<Release>> {
        let url = format!(
            "{}/repos/{}/{}/releases/tags/{}",
            self.api_base, self.repo.owner, self.repo.name, tag
        );
        match self.get_raw(&url) {
            Ok(response) => Ok(Some(parse_json(response)?)),
            Err(_) => Ok(None),
        }
    }

    fn create_release(&self, tag: &str, name: &str, body: &str, target: &str) -> Result<Release> {
        self.post(
            &format!(
                "{}/repos/{}/{}/releases",
                self.api_base, self.repo.owner, self.repo.name
            ),
            &json!({
                "tag_name": tag,
                "target_commitish": target,
                "name": name,
                "body": body,
                "generate_release_notes": false
            }),
        )
    }

    fn update_release(&self, release_id: u64, name: &str, body: &str) -> Result<Release> {
        self.patch(
            &format!(
                "{}/repos/{}/{}/releases/{}",
                self.api_base, self.repo.owner, self.repo.name, release_id
            ),
            &json!({ "name": name, "body": body }),
        )
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        parse_json(self.get_raw(url)?)
    }

    fn get_raw(&self, url: &str) -> Result<String> {
        let response = ureq::get(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "pyrls")
            .call();
        read_response(response)
    }

    fn post<T: for<'de> Deserialize<'de>, B: Serialize>(&self, url: &str, body: &B) -> Result<T> {
        let response = ureq::post(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "pyrls")
            .send_json(body);
        parse_json(read_response(response)?)
    }

    fn patch<T: for<'de> Deserialize<'de>, B: Serialize>(&self, url: &str, body: &B) -> Result<T> {
        let response = ureq::request("PATCH", url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "pyrls")
            .send_json(body);
        parse_json(read_response(response)?)
    }
}

fn read_response(response: std::result::Result<ureq::Response, ureq::Error>) -> Result<String> {
    match response {
        Ok(response) => response.into_string().map_err(Into::into),
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            bail!("GitHub API request failed with status {status}: {body}")
        }
        Err(error) => Err(error.into()),
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: String) -> Result<T> {
    serde_json::from_str(&body).with_context(|| format!("failed to parse GitHub response: {body}"))
}

#[cfg(test)]
mod tests {
    use super::{build_release_pr_plan, build_release_tag_plan, parse_remote_url};
    use crate::{
        analysis::{PackagePlan, PackageReleaseAnalysis, ReleaseAnalysis},
        changelog::PendingChangelog,
        config::Config,
        git::GitRepository,
        version::{BumpLevel, Version},
    };
    use std::{collections::BTreeMap, fs};
    use tempfile::tempdir;

    #[test]
    fn parses_common_github_remote_formats() {
        assert_eq!(
            parse_remote_url("git@github.com:acme/pyrls.git"),
            Some(super::RepoRef {
                owner: "acme".into(),
                name: "pyrls".into()
            })
        );
        assert_eq!(
            parse_remote_url("https://github.com/acme/pyrls.git"),
            Some(super::RepoRef {
                owner: "acme".into(),
                name: "pyrls".into()
            })
        );
    }

    #[test]
    fn builds_release_pr_plan() {
        let config: Config = toml::from_str(
            r#"
            [[version_files]]
            path = "pyproject.toml"
            key = "project.version"
            "#,
        )
        .expect("config");
        let analysis = sample_analysis();

        let plan = build_release_pr_plan(&config, &analysis).expect("plan");
        assert_eq!(plan.branch, "pyrls/release/v1.2.0");
        assert!(plan.title.contains("v1.2.0"));
        assert!(plan.body.contains("Release summary"));
    }

    #[test]
    fn builds_release_tag_plan() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname='demo'\nversion='1.1.0'\n",
        )
        .expect("write");
        run(dir.path(), &["git", "init", "-b", "main"]);
        run(dir.path(), &["git", "config", "user.name", "Pyrls Test"]);
        run(
            dir.path(),
            &["git", "config", "user.email", "pyrls@example.com"],
        );
        run(dir.path(), &["git", "add", "."]);
        run(dir.path(), &["git", "commit", "-m", "feat: initial"]);

        let repo = GitRepository::discover(dir.path()).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [release]
            tag_prefix = "v"

            [[version_files]]
            path = "pyproject.toml"
            key = "project.version"
            "#,
        )
        .expect("config");
        let analysis = sample_analysis();
        let plan = build_release_tag_plan(&config, &repo, &analysis).expect("plan");
        assert_eq!(plan.tag_name, "v1.2.0");
    }

    #[test]
    fn builds_monorepo_release_pr_plan() {
        let config: Config = toml::from_str(
            r#"
            [monorepo]
            enabled = true
            release_mode = "unified"
            packages = ["packages/core", "packages/cli"]
            "#,
        )
        .expect("config");
        let analysis = monorepo_analysis();

        let plan = build_release_pr_plan(&config, &analysis).expect("plan");
        assert!(plan.branch.contains("monorepo"), "{}", plan.branch);
        assert!(plan.title.contains("core 1.2.0"), "{}", plan.title);
        assert!(plan.title.contains("cli 0.5.1"), "{}", plan.title);
    }

    fn sample_analysis() -> ReleaseAnalysis {
        ReleaseAnalysis {
            current_version: Version {
                major: 1,
                minor: 1,
                patch: 0,
            },
            next_version: Some(Version {
                major: 1,
                minor: 2,
                patch: 0,
            }),
            bump: BumpLevel::Minor,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::from([("Added".to_string(), vec!["search".to_string()])]),
            },
            package_plan: PackagePlan {
                release_mode: "single".to_string(),
                discovery_source: "top-level [[version_files]] configuration".to_string(),
                packages: vec![PackageReleaseAnalysis {
                    name: "demo".to_string(),
                    root: ".".to_string(),
                    current_version: Version {
                        major: 1,
                        minor: 1,
                        patch: 0,
                    },
                    next_version: Some(Version {
                        major: 1,
                        minor: 2,
                        patch: 0,
                    }),
                    bump: BumpLevel::Minor,
                    changelog: PendingChangelog {
                        sections: BTreeMap::from([(
                            "Added".to_string(),
                            vec!["search".to_string()],
                        )]),
                    },
                    version_files: Vec::new(),
                    commits: Vec::new(),
                    changed_paths: Vec::new(),
                    selected: true,
                    selection_reason: "single-package repository".to_string(),
                }],
            },
        }
    }

    fn monorepo_analysis() -> ReleaseAnalysis {
        ReleaseAnalysis {
            current_version: Version {
                major: 1,
                minor: 1,
                patch: 0,
            },
            next_version: None,
            bump: BumpLevel::Minor,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::from([(
                    "Added".to_string(),
                    vec!["core: search".to_string(), "cli: status".to_string()],
                )]),
            },
            package_plan: PackagePlan {
                release_mode: "unified".to_string(),
                discovery_source: "auto-discovered package pyproject.toml files".to_string(),
                packages: vec![
                    PackageReleaseAnalysis {
                        name: "core".to_string(),
                        root: "packages/core".to_string(),
                        current_version: Version {
                            major: 1,
                            minor: 1,
                            patch: 0,
                        },
                        next_version: Some(Version {
                            major: 1,
                            minor: 2,
                            patch: 0,
                        }),
                        bump: BumpLevel::Minor,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::new(),
                        changed_paths: vec!["packages/core/src/core.py".to_string()],
                        selected: true,
                        selection_reason: "changed".to_string(),
                    },
                    PackageReleaseAnalysis {
                        name: "cli".to_string(),
                        root: "packages/cli".to_string(),
                        current_version: Version {
                            major: 0,
                            minor: 5,
                            patch: 0,
                        },
                        next_version: Some(Version {
                            major: 0,
                            minor: 5,
                            patch: 1,
                        }),
                        bump: BumpLevel::Patch,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::new(),
                        changed_paths: vec!["packages/cli/src/cli.py".to_string()],
                        selected: true,
                        selection_reason: "changed".to_string(),
                    },
                ],
            },
        }
    }

    fn run(repo_path: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(repo_path)
            .status()
            .expect("command should run");
        assert!(status.success(), "command failed: {args:?}");
    }
}
