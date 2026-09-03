use std::{collections::BTreeMap, env};

use anyhow::{Context, Result, bail};
use openssl::sha::sha256;

use crate::{
    changelog::PendingChangelog,
    config::Config,
    conventional_commits::ConventionalCommit,
    git::GitRepository,
    github::{GitHubClient, IssueComment, PullRequestDetails},
    version::{BumpLevel, Version},
};

pub const PROMOTION_PR_MARKER: &str = "<!-- relx-promotion-pr -->";
pub const FORWARD_PORT_MARKER: &str = "<!-- relx-forward-port -->";
pub const PREVIEW_METADATA_PREFIX: &str = "<!-- relx-preview";
pub const DIGEST_LABEL: &str = "Release-digest:";

#[derive(Debug, Clone)]
pub struct PreviewOptions {
    pub pr_number: Option<u64>,
    pub head_branch: Option<String>,
    pub base_branch: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct PromoteOptions {
    pub pr_number: Option<u64>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionPlan {
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: String,
    pub base_sha: String,
    pub current_version: Version,
    pub baseline_tag: Option<String>,
    pub next_version: Option<Version>,
    pub tag_name: Option<String>,
    pub bump: BumpLevel,
    pub changelog: PendingChangelog,
    pub release_notes: String,
    pub digest: String,
    pub pr_number: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPreview {
    pub version: String,
    pub source_sha: String,
    pub base_sha: String,
    pub digest: String,
}

/// Glob match supporting `*` (any run) and `?` (single char).
pub fn tag_pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    pattern_matches(&pattern, &value)
}

fn pattern_matches(pattern: &[char], value: &[char]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    match pattern[0] {
        '*' => {
            for index in 0..=value.len() {
                if pattern_matches(&pattern[1..], &value[index..]) {
                    return true;
                }
            }
            false
        }
        '?' => !value.is_empty() && pattern_matches(&pattern[1..], &value[1..]),
        ch => !value.is_empty() && value[0] == ch && pattern_matches(&pattern[1..], &value[1..]),
    }
}

pub fn parse_tag_version(tag: &str, tag_prefix: &str) -> Option<Version> {
    tag.strip_prefix(tag_prefix)?.parse::<Version>().ok()
}

/// Resolve the active release line: the highest tag matching
/// `promotion.tag_pattern`, at or above `promotion.baseline_version` when set.
/// Falls back to the baseline, then `versioning.initial_version`.
pub fn resolve_active_baseline(
    repo: &GitRepository,
    config: &Config,
) -> Result<(Version, Option<String>)> {
    let promotion = &config.promotion;
    let baseline: Option<Version> = promotion
        .baseline_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value.parse::<Version>().with_context(|| {
                format!("promotion.baseline_version `{value}` is not a valid version")
            })
        })
        .transpose()?;

    let mut candidates: Vec<(Version, String)> = Vec::new();
    for tag in repo.list_tags()? {
        if !tag_pattern_matches(&promotion.tag_pattern, &tag) {
            continue;
        }
        let Some(version) = parse_tag_version(&tag, &config.release.tag_prefix) else {
            continue;
        };
        if let Some(floor) = &baseline
            && &version < floor
        {
            continue;
        }
        candidates.push((version, tag));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    let mut current: Version = config.versioning.initial_version.parse().with_context(|| {
        format!(
            "versioning.initial_version `{}` is not a valid version",
            config.versioning.initial_version
        )
    })?;
    let mut current_tag: Option<String> = None;
    if let Some(floor) = baseline
        && floor > current
    {
        current = floor;
    }
    if let Some((version, tag)) = candidates.into_iter().next_back()
        && version >= current
    {
        current = version;
        current_tag = Some(tag);
    }

    Ok((current, current_tag))
}

pub fn plan_from_range(
    repo: &GitRepository,
    config: &Config,
    head_branch: &str,
    base_branch: &str,
    head_sha: &str,
    base_sha: &str,
) -> Result<PromotionPlan> {
    let commits = repo.commits_in_range(base_sha, head_sha)?;
    let conventional = commits
        .iter()
        .filter_map(|commit| ConventionalCommit::parse_message(&commit.message).ok())
        .collect::<Vec<_>>();
    let bump = BumpLevel::from_commits(&conventional);
    let (current_version, baseline_tag) = resolve_active_baseline(repo, config)?;
    let next_version = bump.apply(&current_version);
    let changelog = PendingChangelog::from_commits(config, &conventional);
    let date = head_commit_date(repo, head_sha).unwrap_or_else(|| "1970-01-01".to_string());
    let version_label = next_version
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| current_version.to_string());
    let release_notes = crate::changelog::render_release_notes(
        &version_label,
        &date,
        &changelog,
        &config.changelog.first_contribution_emoji,
    );
    let tag_name = next_version
        .as_ref()
        .map(|version| format!("{}{}", config.release.tag_prefix, version));
    let digest = release_digest(
        next_version.as_ref().map(ToString::to_string).as_deref(),
        &changelog.sections,
    );

    Ok(PromotionPlan {
        head_branch: head_branch.to_string(),
        base_branch: base_branch.to_string(),
        head_sha: head_sha.to_string(),
        base_sha: base_sha.to_string(),
        current_version,
        baseline_tag,
        next_version,
        tag_name,
        bump,
        changelog,
        release_notes,
        digest,
        pr_number: None,
    })
}

fn head_commit_date(repo: &GitRepository, head_sha: &str) -> Option<String> {
    crate::git::run_git(repo.path(), ["show", "-s", "--format=%cs", head_sha]).ok()
}

pub fn release_digest(version: Option<&str>, sections: &BTreeMap<String, Vec<String>>) -> String {
    let mut input = String::new();
    input.push_str(version.unwrap_or("none"));
    input.push('\n');
    for (section, entries) in sections {
        input.push_str(section);
        input.push('\n');
        for entry in entries {
            input.push_str(entry);
            input.push('\n');
        }
    }
    hex_digest(&sha256(input.as_bytes()))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

pub fn render_preview_comment(marker: &str, plan: &PromotionPlan, tag_name: &str) -> String {
    let mut body = String::new();
    body.push_str(marker);
    body.push('\n');
    match &plan.next_version {
        Some(_) => {
            body.push_str(&format!("## Proposed release: {tag_name}\n\n"));
            body.push_str(&plan.release_notes);
            body.push('\n');
        }
        None => {
            body.push_str("## Proposed release: none\n\n");
            body.push_str("No releasable Conventional Commits found in this PR.\n");
        }
    }
    body.push_str(&format!(
        "\nRelease source: `{}@{}`\n",
        plan.head_branch,
        short_sha(&plan.head_sha)
    ));
    body.push_str(&format!(
        "Validated against: `{}@{}`\n",
        plan.base_branch,
        short_sha(&plan.base_sha)
    ));
    body.push_str(&format!("{DIGEST_LABEL} `{}`\n", plan.digest));
    body
}

pub fn parse_preview_comment(body: &str) -> Option<ParsedPreview> {
    let mut version: Option<String> = None;
    let mut source_sha: Option<String> = None;
    let mut base_sha: Option<String> = None;
    let mut digest: Option<String> = None;

    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("## Proposed release:") {
            let value = rest.trim();
            if !value.is_empty() && value != "none" {
                version = Some(value.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("Release source:") {
            source_sha = ref_sha(rest.trim());
        } else if let Some(rest) = line.strip_prefix("Validated against:") {
            base_sha = ref_sha(rest.trim());
        } else if let Some(rest) = line.strip_prefix(DIGEST_LABEL) {
            digest = Some(rest.trim().trim_matches('`').to_string());
        }
    }

    Some(ParsedPreview {
        version: version?,
        source_sha: source_sha?,
        base_sha: base_sha?,
        digest: digest?,
    })
}

fn ref_sha(value: &str) -> Option<String> {
    let inner = value.trim().trim_matches('`');
    inner.split_once('@').map(|(_, sha)| sha.to_string())
}

pub fn find_sticky_comment<'a>(
    comments: &'a [IssueComment],
    marker: &str,
) -> Option<&'a IssueComment> {
    comments
        .iter()
        .find(|comment| comment.body.contains(marker))
}

pub fn render_promotion_pr_body(plan: &PromotionPlan, tag_name: Option<&str>) -> String {
    let mut body = String::new();
    body.push_str(PROMOTION_PR_MARKER);
    body.push('\n');
    body.push_str(&render_preview_metadata(
        plan.tag_name.as_deref().unwrap_or("none"),
        &plan.head_sha,
        &plan.base_sha,
        &plan.digest,
    ));
    body.push('\n');
    body.push_str(&format!(
        "## Promotion: `{}@{}` → `{}@{}`\n\n",
        plan.head_branch,
        short_sha(&plan.head_sha),
        plan.base_branch,
        short_sha(&plan.base_sha)
    ));
    match tag_name {
        Some(tag) => body.push_str(&format!("Proposed release: **{tag}**\n\n")),
        None => body.push_str("Proposed release: none pending\n\n"),
    }
    body.push_str(&plan.release_notes);
    body.push('\n');
    body
}

/// Hidden machine-readable preview state embedded in relx-managed PR bodies.
/// This is the freshness anchor `promote` verifies; relx-managed PRs carry
/// the same information visibly above, so no sticky comment is needed.
pub fn render_preview_metadata(
    version: &str,
    source_sha: &str,
    base_sha: &str,
    digest: &str,
) -> String {
    format!(
        "{PREVIEW_METADATA_PREFIX} version=\"{version}\" source=\"{source_sha}\" base=\"{base_sha}\" digest=\"{digest}\" -->"
    )
}

pub fn parse_preview_metadata(body: &str) -> Option<ParsedPreview> {
    let line = body
        .lines()
        .find(|line| line.trim_start().starts_with(PREVIEW_METADATA_PREFIX))?;
    let inner = line
        .trim()
        .strip_prefix(PREVIEW_METADATA_PREFIX)?
        .strip_suffix("-->")?;
    let mut version = None;
    let mut source_sha = None;
    let mut base_sha = None;
    let mut digest = None;
    for part in inner.split_whitespace() {
        let (key, value) = part.split_once('=')?;
        let value = value.trim_matches('"');
        match key {
            "version" if value != "none" => version = Some(value.to_string()),
            "source" => source_sha = Some(value.to_string()),
            "base" => base_sha = Some(value.to_string()),
            "digest" => digest = Some(value.to_string()),
            _ => {}
        }
    }
    Some(ParsedPreview {
        version: version?,
        source_sha: source_sha?,
        base_sha: base_sha?,
        digest: digest?,
    })
}

pub fn render_promotion_pr_title(plan: &PromotionPlan) -> String {
    match &plan.tag_name {
        Some(tag) => format!(
            "chore(promote): {} -> {} ({})",
            plan.head_branch, plan.base_branch, tag
        ),
        None => format!(
            "chore(promote): {} -> {}",
            plan.head_branch, plan.base_branch
        ),
    }
}

/// Stable generated-branch name for the versioned promotion path, e.g.
/// `relx/promote/develop-main`.
pub fn promotion_branch_name(config: &Config, head_branch: &str, base_branch: &str) -> String {
    let prefix = config.promotion.release_branch_prefix.trim_end_matches('/');
    format!(
        "{}/{}-{}",
        prefix,
        sanitize_branch_segment(head_branch),
        sanitize_branch_segment(base_branch)
    )
}

fn sanitize_branch_segment(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch.to_ascii_lowercase(),
            _ => '-',
        })
        .collect();
    let cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.is_empty() {
        "branch".to_string()
    } else {
        cleaned
    }
}

pub fn emit_github_output(key: &str, value: &str) -> Result<()> {
    if let Ok(path) = env::var("GITHUB_OUTPUT") {
        use std::fmt::Write as _;
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        if value.contains('\n') {
            let delimiter = "RELX_EOF";
            let _ = writeln!(existing, "{key}<<{delimiter}\n{value}\n{delimiter}");
        } else {
            let _ = writeln!(existing, "{key}={value}");
        }
        std::fs::write(&path, existing).with_context(|| format!("failed to append to {path}"))?;
    }
    Ok(())
}

fn print_output(key: &str, value: &str) {
    if value.contains('\n') {
        println!("{key}:");
        for line in value.lines() {
            println!("  {line}");
        }
    } else {
        println!("{key}={value}");
    }
}

pub fn emit_preview_outputs(plan: &PromotionPlan, pr_number: u64, json: bool) -> Result<()> {
    let version = plan
        .next_version
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let tag_name = plan.tag_name.clone().unwrap_or_default();
    if json {
        let payload = serde_json::json!({
            "pr_number": pr_number,
            "version": version,
            "tag_name": tag_name,
            "release_notes": plan.release_notes,
            "source_sha": plan.head_sha,
            "base_sha": plan.base_sha,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        print_output("pr_number", &pr_number.to_string());
        print_output("version", &version);
        print_output("tag_name", &tag_name);
        print_output("release_notes", &plan.release_notes);
        print_output("source_sha", &plan.head_sha);
        print_output("base_sha", &plan.base_sha);
    }
    emit_github_output("pr_number", &pr_number.to_string())?;
    emit_github_output("version", &version)?;
    emit_github_output("tag_name", &tag_name)?;
    emit_github_output("release_notes", &plan.release_notes)?;
    emit_github_output("source_sha", &plan.head_sha)?;
    emit_github_output("base_sha", &plan.base_sha)?;
    Ok(())
}

fn ensure_sha_available(repo: &GitRepository, sha: &str) -> Result<()> {
    if repo.rev_parse(sha).is_ok() {
        return Ok(());
    }
    let path = repo.path();
    let fetch = crate::git::run_git(path, ["fetch", "origin", sha]);
    if fetch.is_err() {
        bail!(
            "reference `{sha}` is not available locally; fetch the PR head and base before running preview"
        );
    }
    Ok(())
}

fn resolve_preview_refs(
    repo: &GitRepository,
    config: &Config,
    client: Option<&GitHubClient>,
    options: &PreviewOptions,
) -> Result<(String, String, String, String, Option<PullRequestDetails>)> {
    let production = config
        .promotion
        .production_branch_for(&config.release.branch);
    let head_branch = options
        .head_branch
        .clone()
        .unwrap_or_else(|| config.promotion.development_branch.clone());
    let base_branch = options
        .base_branch
        .clone()
        .unwrap_or_else(|| production.clone());

    if base_branch != production {
        bail!("preview base `{base_branch}` must be the production branch `{production}`");
    }
    if !config.promotion.is_promotion_head(&head_branch) {
        bail!(
            "preview head `{head_branch}` must be `{}` or start with one of {:?}",
            config.promotion.development_branch,
            config.promotion.hotfix_prefixes
        );
    }

    if let (Some(number), Some(client)) = (options.pr_number, client) {
        let details = client.get_pr(number)?;
        if details.base.ref_name != base_branch || details.head.ref_name != head_branch {
            bail!(
                "PR #{number} is {} -> {}, expected {head_branch} -> {base_branch}",
                details.head.ref_name,
                details.base.ref_name
            );
        }
        return Ok((
            head_branch,
            base_branch,
            details.head.sha.clone(),
            details.base.sha.clone(),
            Some(details),
        ));
    }

    if options.pr_number.is_some() {
        bail!("--pr requires GitHub access (missing token?)");
    }

    let head_sha = repo
        .rev_parse(&head_branch)
        .or_else(|_| repo.rev_parse(&format!("origin/{head_branch}")))?;
    let base_sha = repo
        .rev_parse(&base_branch)
        .or_else(|_| repo.rev_parse(&format!("origin/{base_branch}")))?;
    Ok((head_branch, base_branch, head_sha, base_sha, None))
}

fn github_client(repo: &GitRepository, config: &Config) -> Result<GitHubClient> {
    let repo_ref = crate::github::detect_repo(repo, &config.github)?;
    let token = env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    GitHubClient::new(&config.github.api_base, &token, repo_ref)
}

/// Create the develop/hotfix -> production PR when it does not exist, and
/// refresh the title/body of PRs previously created by relx. Pre-existing
/// user-owned PRs are left untouched. Returns the PR number and whether the
/// PR is relx-managed (created by relx or carrying its marker); only
/// user-owned PRs receive the sticky preview comment.
pub fn ensure_promotion_pr(
    client: &GitHubClient,
    plan: &PromotionPlan,
    existing_number: Option<u64>,
) -> Result<(u64, bool)> {
    if let Some(number) = existing_number {
        let details = client.get_pr(number)?;
        if details.base.ref_name != plan.base_branch || details.head.ref_name != plan.head_branch {
            bail!(
                "PR #{number} is {} -> {}, expected {} -> {}",
                details.head.ref_name,
                details.base.ref_name,
                plan.head_branch,
                plan.base_branch
            );
        }
        let managed = details.has_marker(PROMOTION_PR_MARKER);
        if managed {
            client.update_pr(
                number,
                &render_promotion_pr_title(plan),
                &render_promotion_pr_body(plan, plan.tag_name.as_deref()),
            )?;
        }
        return Ok((number, managed));
    }

    if let Some(open) = client.find_open_pr(&plan.head_branch, &plan.base_branch)? {
        let details = client.get_pr(open.number)?;
        let managed = details.has_marker(PROMOTION_PR_MARKER);
        if managed {
            client.update_pr(
                open.number,
                &render_promotion_pr_title(plan),
                &render_promotion_pr_body(plan, plan.tag_name.as_deref()),
            )?;
        }
        return Ok((open.number, managed));
    }

    let created = client.create_pr(
        &render_promotion_pr_title(plan),
        &plan.head_branch,
        &plan.base_branch,
        &render_promotion_pr_body(plan, plan.tag_name.as_deref()),
    )?;
    Ok((created.number, true))
}

pub fn upsert_preview_comment(
    client: &GitHubClient,
    marker: &str,
    pr_number: u64,
    body: &str,
) -> Result<()> {
    let comments = client.list_issue_comments(pr_number)?;
    match find_sticky_comment(&comments, marker) {
        Some(existing) if existing.body == body => Ok(()),
        Some(existing) => {
            client.update_issue_comment(existing.id, body)?;
            Ok(())
        }
        None => {
            client.create_issue_comment(pr_number, body)?;
            Ok(())
        }
    }
}

pub fn execute_preview(
    repo: &GitRepository,
    config: &Config,
    options: &PreviewOptions,
    dry_run: bool,
) -> Result<PromotionPlan> {
    if !config.promotion.enabled {
        bail!("promotion mode is not enabled; set [promotion].enabled = true");
    }
    if config.monorepo.enabled {
        bail!("promotion mode does not support monorepo repositories yet");
    }
    config.validate()?;

    let client = github_client(repo, config).ok();
    let (head_branch, base_branch, head_sha, base_sha, pr_details) =
        resolve_preview_refs(repo, config, client.as_ref(), options)?;
    ensure_sha_available(repo, &head_sha)?;
    ensure_sha_available(repo, &base_sha)?;

    let mut plan = plan_from_range(
        repo,
        config,
        &head_branch,
        &base_branch,
        &head_sha,
        &base_sha,
    )?;
    let marker = config.promotion.preview_marker.clone();
    let tag_name = plan.tag_name.clone().unwrap_or_default();
    let comment = render_preview_comment(&marker, &plan, &tag_name);

    if !config.version_files.is_empty() {
        return execute_versioned_preview(repo, config, client, options, &mut plan, dry_run);
    }

    if dry_run {
        println!(
            "Would ensure {} -> {} promotion PR exists (tag-only, no generated branch)",
            head_branch, base_branch
        );
        println!("Would refresh the relx-managed PR body with the preview");
        println!("Would post the sticky preview comment only on user-owned PRs");
        println!("{comment}");
        emit_preview_outputs(&plan, options.pr_number.unwrap_or(0), options.json)?;
        return Ok(plan);
    }

    let Some(client) = client else {
        bail!(
            "missing GitHub token in {}: preview must update the promotion PR",
            config.github.token_env
        );
    };

    let (pr_number, managed) = match pr_details {
        Some(details) => {
            let number = details.number;
            let managed = details.has_marker(PROMOTION_PR_MARKER);
            if managed {
                client.update_pr(
                    number,
                    &render_promotion_pr_title(&plan),
                    &render_promotion_pr_body(&plan, plan.tag_name.as_deref()),
                )?;
            }
            (number, managed)
        }
        None => ensure_promotion_pr(&client, &plan, None)?,
    };
    if managed {
        println!(
            "PR #{pr_number} is relx-managed; preview lives in the PR body, no comment posted"
        );
    } else {
        upsert_preview_comment(&client, &marker, pr_number, &comment)?;
    }
    plan.pr_number = Some(pr_number);

    let pr_title = client
        .get_pr(pr_number)
        .map(|details| details.title)
        .unwrap_or_default();

    println!("Promotion PR ready: #{pr_number} {head_branch} -> {base_branch}");
    if !pr_title.is_empty() {
        println!("PR title: {pr_title}");
    }
    if let Some(version) = &plan.next_version {
        println!("Proposed release: {}{}", config.release.tag_prefix, version);
    } else {
        println!("No releasable changes pending");
    }
    emit_preview_outputs(&plan, pr_number, options.json)?;
    Ok(plan)
}

/// Versioned promotion path: cut a generated `relx/promote/*` branch from
/// the promotion head, apply the version bump and changelog entry, and open
/// (or update) a single PR to production carrying code plus versioning.
/// Generated PRs are always relx-managed, so the preview lives in the PR
/// body and no sticky comment is posted.
#[allow(clippy::too_many_arguments)]
pub fn execute_versioned_preview(
    repo: &GitRepository,
    config: &Config,
    client: Option<GitHubClient>,
    options: &PreviewOptions,
    plan: &mut PromotionPlan,
    dry_run: bool,
) -> Result<PromotionPlan> {
    let branch = promotion_branch_name(config, &plan.head_branch, &plan.base_branch);
    let title = render_promotion_pr_title(plan);
    let body = render_promotion_pr_body(plan, plan.tag_name.as_deref());

    let Some(next_version) = plan.next_version.clone() else {
        if dry_run {
            println!(
                "Would ensure {} -> {} promotion PR exists ({})",
                plan.head_branch, plan.base_branch, branch
            );
            println!("No releasable changes pending; no branch or PR would be changed");
            emit_preview_outputs(plan, options.pr_number.unwrap_or(0), options.json)?;
            return Ok(plan.clone());
        }
        println!("No releasable changes pending; no branch or PR changed");
        emit_preview_outputs(plan, options.pr_number.unwrap_or(0), options.json)?;
        return Ok(plan.clone());
    };

    if dry_run {
        println!(
            "Would push generated branch `{}` from `{}@{}`",
            branch,
            plan.head_branch,
            short_sha(&plan.head_sha)
        );
        println!("Would update version files to {next_version}");
        println!(
            "Would prepend {} with the proposed release notes",
            config.release.changelog_file
        );
        println!(
            "Would create or update PR `{}` ({} -> {})",
            title, branch, plan.base_branch
        );
        emit_preview_outputs(plan, options.pr_number.unwrap_or(0), options.json)?;
        return Ok(plan.clone());
    }

    let Some(client) = client else {
        bail!(
            "missing GitHub token in {}: preview must push the promotion branch",
            config.github.token_env
        );
    };
    let token = std::env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    let origin_url = repo
        .remote_url("origin")?
        .context("origin remote is required for the versioned promotion flow")?;

    let clone_dir = tempfile::tempdir().context("failed to create temporary workspace")?;
    let clone_path = clone_dir.path().join("repo");
    crate::git::run_git(
        clone_dir.path(),
        vec![
            "clone".into(),
            repo.path().as_os_str().to_owned(),
            clone_path.as_os_str().to_owned(),
        ],
    )?;
    let auth_url = crate::github::authenticated_url(&origin_url, &token);
    crate::git::run_git(
        &clone_path,
        ["remote", "set-url", "origin", auth_url.as_str()],
    )?;
    crate::git::run_git(
        &clone_path,
        ["checkout", "-B", branch.as_str(), plan.head_sha.as_str()],
    )?;

    crate::analysis::update_version_files(&clone_path, &config.version_files, &next_version)?;
    crate::changelog::prepend_release_notes(
        &clone_path.join(&config.release.changelog_file),
        &plan.release_notes,
    )?;
    crate::github::refresh_lockfile(&clone_path, config, &config.version_files)?;

    crate::git::run_git(&clone_path, ["add", "."])?;
    let diff = crate::git::run_git(&clone_path, ["status", "--short"])?;
    if !diff.trim().is_empty() {
        // Deliberately non-conventional (no `type:` prefix) so the version
        // commit itself never contributes to a future bump or changelog and
        // promote-time recomputation matches the preview exactly.
        let commit_message = format!(
            "relx promote {} -> {} ({})",
            plan.head_branch,
            plan.base_branch,
            plan.tag_name.as_deref().unwrap_or("no release")
        );
        crate::git::run_git(
            &clone_path,
            crate::github::release_commit_args(config, commit_message.as_str()),
        )?;
    }
    crate::git::run_git(
        &clone_path,
        [
            "push",
            "--force",
            "origin",
            format!("HEAD:{}", branch).as_str(),
        ],
    )?;

    let pr = match client.find_open_pr(&branch, &plan.base_branch)? {
        Some(existing) => client.update_pr(existing.number, &title, &body)?,
        None => client.create_pr(&title, &branch, &plan.base_branch, &body)?,
    };
    plan.pr_number = Some(pr.number);

    println!(
        "Promotion PR ready: #{} {} ({} -> {})",
        pr.number, title, branch, plan.base_branch
    );
    println!(
        "Proposed release: {}{}",
        config.release.tag_prefix, next_version
    );
    emit_preview_outputs(plan, pr.number, options.json)?;
    Ok(plan.clone())
}

pub fn execute_promote(
    repo: &GitRepository,
    config: &Config,
    options: &PromoteOptions,
    dry_run: bool,
) -> Result<()> {
    if !config.promotion.enabled {
        bail!("promotion mode is not enabled; set [promotion].enabled = true");
    }
    config.validate()?;
    let client = github_client(repo, config)?;

    let production = config
        .promotion
        .production_branch_for(&config.release.branch);
    let head_branch = config.promotion.development_branch.clone();

    let details = match options.pr_number {
        Some(number) => client.get_pr(number)?,
        None => client
            .find_merged_pr(&head_branch, &production)?
            .context("no merged develop -> production PR found; pass --pr <number>")?,
    };

    if !details.merged {
        bail!(
            "PR #{} is not merged; promote runs after the promotion PR is merged",
            details.number
        );
    }
    if details.base.ref_name != production {
        bail!(
            "PR #{} targets `{}`, expected production branch `{production}`",
            details.number,
            details.base.ref_name
        );
    }
    if !config.promotion.is_promotion_head(&details.head.ref_name) {
        // Versioned path: the PR head is the generated relx/promote/* branch.
        let generated_prefix = format!(
            "{}/",
            config.promotion.release_branch_prefix.trim_end_matches('/')
        );
        if !details.head.ref_name.starts_with(&generated_prefix) {
            bail!(
                "PR #{} head `{}` is not the development branch, a hotfix branch, or a generated promotion branch",
                details.number,
                details.head.ref_name
            );
        }
    }
    let merge_sha = details.merge_commit_sha.clone().context(format!(
        "PR #{} has no merge commit recorded",
        details.number
    ))?;

    let comments = client.list_issue_comments(details.number)?;
    let marker = config.promotion.preview_marker.clone();
    let preview = match find_sticky_comment(&comments, &marker)
        .and_then(|sticky| parse_preview_comment(&sticky.body))
    {
        Some(preview) => preview,
        None => parse_preview_metadata(&details.body).with_context(|| {
            format!(
                "no preview found on PR #{}; run `relx release preview-pr --pr {}` first",
                details.number, details.number
            )
        })?,
    };

    ensure_sha_available(repo, &merge_sha)?;
    ensure_sha_available(repo, &preview.base_sha)?;
    if !repo.is_ancestor(&preview.source_sha, &merge_sha)? {
        bail!(
            "preview source {} is not an ancestor of the merged commit {}; refresh the preview before promoting",
            short_sha(&preview.source_sha),
            short_sha(&merge_sha)
        );
    }

    let current = plan_from_range(
        repo,
        config,
        &details.head.ref_name,
        &details.base.ref_name,
        &merge_sha,
        &preview.base_sha,
    )?;
    let current_tag = current.tag_name.clone().unwrap_or_default();
    if preview.version != current_tag {
        bail!(
            "PR #{} changed after its preview (previewed {}, now {}); refresh with `relx release preview-pr --pr {}` before promoting",
            details.number,
            preview.version,
            if current_tag.is_empty() {
                "no release".to_string()
            } else {
                current_tag
            },
            details.number
        );
    }
    if preview.digest != current.digest {
        bail!(
            "release notes for PR #{} changed after its preview; refresh with `relx release preview-pr --pr {}` before promoting",
            details.number,
            details.number
        );
    }
    let next_version = current.next_version.clone().context(format!(
        "PR #{} contains no releasable changes",
        details.number
    ))?;

    if !config.version_files.is_empty() {
        let prepared = crate::analysis::read_current_version(repo.path(), &config.version_files)?
            .with_context(|| {
            format!(
                "PR #{} merged without version files; cannot verify the prepared release",
                details.number
            )
        })?;
        if prepared != next_version.to_string() {
            bail!(
                "PR #{} prepared version {prepared} does not match previewed version {next_version}; refresh the preview before promoting",
                details.number
            );
        }
    }

    if dry_run {
        println!("Would tag {current_tag} at {}", short_sha(&merge_sha));
        println!("Would create or update GitHub Release {current_tag}");
        println!("{}", current.release_notes);
        emit_promote_outputs(false, &current_tag, &current_tag, options.json)?;
        return Ok(());
    }

    let release_created = create_tag_and_release(repo, config, &current, &merge_sha, &production)?;
    println!("Release tagged: {current_tag}");
    let version = current
        .next_version
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    emit_promote_outputs(release_created, &version, &current_tag, options.json)?;
    Ok(())
}

fn emit_promote_outputs(
    release_created: bool,
    version: &str,
    tag_name: &str,
    json: bool,
) -> Result<()> {
    if json {
        let payload = serde_json::json!({
            "release_created": release_created,
            "version": version,
            "tag_name": tag_name,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        print_output("release_created", &release_created.to_string());
        print_output("version", version);
        print_output("tag_name", tag_name);
    }
    emit_github_output("release_created", &release_created.to_string())?;
    emit_github_output("version", version)?;
    emit_github_output("tag_name", tag_name)?;
    Ok(())
}

fn create_tag_and_release(
    repo: &GitRepository,
    config: &Config,
    plan: &PromotionPlan,
    target_sha: &str,
    production_branch: &str,
) -> Result<bool> {
    let tag_name = plan.tag_name.clone().context("no tag pending")?;
    let title = config
        .release
        .release_name
        .replace("{tag_name}", &tag_name)
        .replace(
            "{version}",
            &plan
                .next_version
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        );

    if repo.rev_parse(&tag_name).is_ok() {
        return Ok(false);
    }

    let repo_ref = crate::github::detect_repo(repo, &config.github)?;
    let token = env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    let client = GitHubClient::new(&config.github.api_base, &token, repo_ref)?;

    crate::git::run_git(
        repo.path(),
        [
            "-c",
            &format!("user.name={}", config.github.commit_author),
            "-c",
            &format!("user.email={}", config.github.commit_email),
            "tag",
            "-a",
            &tag_name,
            "-m",
            &title,
            target_sha,
        ],
    )?;
    crate::git::run_git(repo.path(), ["push", "origin", &tag_name])?;

    match client.find_release_by_tag(&tag_name)? {
        Some(existing) => {
            client.update_release(existing.id, &title, &plan.release_notes)?;
        }
        None => {
            client.create_release(&tag_name, &title, &plan.release_notes, production_branch)?;
        }
    }

    Ok(true)
}

/// Forward-port production back into development after a hotfix. Creates
/// (or refreshes, when relx-managed) a production -> development PR so the
/// hotfix is not lost on the next promotion. No-ops when development
/// already contains production.
pub fn execute_forward_port(
    repo: &GitRepository,
    config: &Config,
    dry_run: bool,
) -> Result<Option<u64>> {
    if !config.promotion.enabled {
        bail!("promotion mode is not enabled; set [promotion].enabled = true");
    }
    config.validate()?;

    let production = config
        .promotion
        .production_branch_for(&config.release.branch);
    let development = config.promotion.development_branch.clone();
    let production_sha = repo
        .rev_parse(&production)
        .or_else(|_| repo.rev_parse(&format!("origin/{production}")))?;
    let development_sha = repo
        .rev_parse(&development)
        .or_else(|_| repo.rev_parse(&format!("origin/{development}")))?;

    if repo.is_ancestor(&production_sha, &development_sha)? {
        println!("{development} already contains {production}; nothing to forward-port");
        return Ok(None);
    }

    let title = format!("chore: forward-port {production} -> {development}");
    if dry_run {
        println!("Would ensure PR `{title}` exists");
        return Ok(None);
    }

    let client = github_client(repo, config)?;
    let body = format!(
        "{FORWARD_PORT_MARKER}\n## Forward-port\n\nBrings hotfix changes from `{production}@{}` into `{development}` so the next promotion includes them.\n",
        short_sha(&production_sha)
    );

    if let Some(open) = client.find_open_pr(&production, &development)? {
        let details = client.get_pr(open.number)?;
        if details.has_marker(FORWARD_PORT_MARKER) {
            client.update_pr(open.number, &title, &body)?;
        }
        println!("Forward-port PR ready: #{} {title}", open.number);
        return Ok(Some(open.number));
    }

    let created = client.create_pr(&title, &production, &development, &body)?;
    println!("Forward-port PR ready: #{} {title}", created.number);
    Ok(Some(created.number))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_preview_comment, parse_tag_version, release_digest, render_preview_comment,
        tag_pattern_matches,
    };
    use crate::changelog::PendingChangelog;
    use crate::config::Config;
    use crate::git::GitRepository;
    use crate::version::BumpLevel;
    use std::collections::BTreeMap;
    use std::process::Command;

    use super::{
        PROMOTION_PR_MARKER, ParsedPreview, PromotionPlan, parse_preview_metadata,
        promotion_branch_name, render_promotion_pr_body, resolve_active_baseline,
    };

    #[test]
    fn tag_pattern_supports_wildcards() {
        assert!(tag_pattern_matches("v*", "v0.3.0"));
        assert!(tag_pattern_matches("v0.*", "v0.3.0"));
        assert!(!tag_pattern_matches("v0.*", "v1.1.0"));
        assert!(tag_pattern_matches("release-?", "release-1"));
        assert!(!tag_pattern_matches("release-?", "release-12"));
    }

    #[test]
    fn tag_version_parsing_strips_prefix() {
        let version = parse_tag_version("v0.3.0", "v").expect("version");
        assert_eq!(version.to_string(), "0.3.0");
        assert!(parse_tag_version("v1.1.0", "v").is_some());
        assert!(parse_tag_version("0.3.0", "v").is_none());
    }

    #[test]
    fn preview_comment_round_trips() {
        let plan = PromotionPlan {
            head_branch: "develop".to_string(),
            base_branch: "main".to_string(),
            head_sha: "abc1234def5678".to_string(),
            base_sha: "def5678abc1234".to_string(),
            current_version: "0.2.0".parse().unwrap(),
            baseline_tag: None,
            next_version: Some("0.3.0".parse().unwrap()),
            tag_name: Some("v0.3.0".to_string()),
            bump: BumpLevel::Minor,
            changelog: PendingChangelog {
                sections: BTreeMap::from([("Added".to_string(), vec!["add search".to_string()])]),
                contributors: Vec::new(),
            },
            release_notes: "notes".to_string(),
            digest: "digest".to_string(),
            pr_number: None,
        };
        let body = render_preview_comment("<!-- relx-release-preview -->", &plan, "v0.3.0");
        assert!(body.contains("## Proposed release: v0.3.0"));
        assert!(body.contains("Release source: `develop@abc1234`"));
        let parsed = parse_preview_comment(&body).expect("parse");
        assert_eq!(parsed.version, "v0.3.0");
        assert_eq!(parsed.digest, "digest");
    }

    #[test]
    fn preview_metadata_round_trips_through_pr_body() {
        let plan = PromotionPlan {
            head_branch: "develop".to_string(),
            base_branch: "main".to_string(),
            head_sha: "abc1234def5678".to_string(),
            base_sha: "def5678abc1234".to_string(),
            current_version: "0.2.0".parse().unwrap(),
            baseline_tag: None,
            next_version: Some("0.3.0".parse().unwrap()),
            tag_name: Some("v0.3.0".to_string()),
            bump: BumpLevel::Minor,
            changelog: PendingChangelog {
                sections: BTreeMap::new(),
                contributors: Vec::new(),
            },
            release_notes: "notes".to_string(),
            digest: "digest".to_string(),
            pr_number: None,
        };
        let body = render_promotion_pr_body(&plan, Some("v0.3.0"));
        assert!(body.contains(PROMOTION_PR_MARKER));
        assert!(body.contains("Proposed release: **v0.3.0**"));
        assert!(!body.contains("Managed by"));
        let parsed = parse_preview_metadata(&body).expect("parse");
        assert_eq!(
            parsed,
            ParsedPreview {
                version: "v0.3.0".to_string(),
                source_sha: "abc1234def5678".to_string(),
                base_sha: "def5678abc1234".to_string(),
                digest: "digest".to_string(),
            }
        );
    }

    #[test]
    fn promotion_branch_name_is_stable_and_sanitized() {
        let config: Config = toml::from_str(
            r#"
            [promotion]
            enabled = true
            "#,
        )
        .expect("config");
        assert_eq!(
            promotion_branch_name(&config, "develop", "main"),
            "relx/promote/develop-main"
        );
        assert_eq!(
            promotion_branch_name(&config, "hotfix/login-loop", "main"),
            "relx/promote/hotfix-login-loop-main"
        );
    }

    #[test]
    fn digest_changes_with_notes() {
        let left = release_digest(
            Some("0.3.0"),
            &BTreeMap::from([("Added".to_string(), vec!["a".to_string()])]),
        );
        let right = release_digest(
            Some("0.3.0"),
            &BTreeMap::from([("Added".to_string(), vec!["b".to_string()])]),
        );
        assert_ne!(left, right);
    }

    #[test]
    fn active_baseline_prefers_configured_series() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        git(path, &["init", "-b", "main"]);
        git(path, &["config", "user.name", "Relx Test"]);
        git(path, &["config", "user.email", "relx@example.com"]);
        std::fs::write(path.join("notes.txt"), "initial\n").expect("write file");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "chore: initial commit"]);
        git(path, &["tag", "v1.0.0"]);
        git(path, &["tag", "v1.1.0"]);
        std::fs::write(path.join("notes.txt"), "restart\n").expect("write file");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "chore: restart line"]);
        git(path, &["tag", "v0.2.0"]);

        let repo = GitRepository::discover(path).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [promotion]
            enabled = true
            development_branch = "develop"
            tag_pattern = "v0.*"
            baseline_version = "0.2.0"
            "#,
        )
        .expect("config");

        let (current, tag) = resolve_active_baseline(&repo, &config).expect("baseline");
        assert_eq!(current.to_string(), "0.2.0");
        assert_eq!(tag.as_deref(), Some("v0.2.0"));
    }

    #[test]
    fn active_baseline_uses_bootstrap_without_matching_tags() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        git(path, &["init", "-b", "main"]);
        git(path, &["config", "user.name", "Relx Test"]);
        git(path, &["config", "user.email", "relx@example.com"]);
        std::fs::write(path.join("notes.txt"), "initial\n").expect("write file");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "chore: initial commit"]);
        git(path, &["tag", "v1.0.0"]);
        git(path, &["tag", "v1.1.0"]);

        let repo = GitRepository::discover(path).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [promotion]
            enabled = true
            development_branch = "develop"
            tag_pattern = "v0.*"
            baseline_version = "0.2.0"
            "#,
        )
        .expect("config");

        let (current, tag) = resolve_active_baseline(&repo, &config).expect("baseline");
        assert_eq!(current.to_string(), "0.2.0");
        assert_eq!(tag, None);
    }

    #[test]
    fn promotion_config_accepts_tag_only_repositories() {
        let config: Config = toml::from_str(
            r#"
            [promotion]
            enabled = true
            development_branch = "develop"
            tag_pattern = "v*"
            "#,
        )
        .expect("config");
        config.validate().expect("tag-only promotion validates");
    }

    fn git(path: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            // Never prompt for commit/tag signing in test repos; the local
            // environment may enable gpgsign globally.
            .env("GIT_CONFIG_COUNT", "3")
            .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
            .env("GIT_CONFIG_VALUE_0", "false")
            .env("GIT_CONFIG_KEY_1", "tag.gpgsign")
            .env("GIT_CONFIG_VALUE_1", "false")
            .env("GIT_CONFIG_KEY_2", "gpg.format")
            .env("GIT_CONFIG_VALUE_2", "openpgp")
            .env("GIT_EDITOR", "true")
            .current_dir(path)
            .status()
            .expect("git should run");
        assert!(status.success(), "git failed: {args:?}");
    }
}
