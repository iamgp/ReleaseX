use std::{
    collections::BTreeMap,
    env, fs,
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use openssl::sha::sha256;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::tempdir;

use crate::{
    analysis::{self, ReleaseAnalysis},
    changelog, channels,
    config::{Config, Ecosystem, GitHubConfig, VersionFileConfig},
    ecosystem,
    git::{GitRepository, run_git},
    prerelease::{
        PrereleasePackage, build_explicit_install_command, sync_python_workspace_dependencies,
        sync_root_python_workspace_dependencies, validate_root_wheel_metadata,
    },
    replacements,
    version::Suffix,
    workspace_plan::ReleaseWorkspacePlan,
};

pub(crate) fn authenticated_url(origin_url: &str, token: &str) -> String {
    if let Some(rest) = origin_url.strip_prefix("https://") {
        format!("https://x-access-token:{token}@{rest}")
    } else {
        origin_url.to_string()
    }
}

pub(crate) fn release_commit_args(config: &Config, message: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("user.name={}", config.github.commit_author),
        "-c".to_string(),
        format!("user.email={}", config.github.commit_email),
        "commit".to_string(),
        "-m".to_string(),
        message.to_string(),
    ]
}

fn release_tag_args(config: &Config, tag_name: &str, message: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("user.name={}", config.github.commit_author),
        "-c".to_string(),
        format!("user.email={}", config.github.commit_email),
        "tag".to_string(),
        "-a".to_string(),
        tag_name.to_string(),
        "-m".to_string(),
        message.to_string(),
    ]
}

pub(crate) fn refresh_lockfile(
    clone_path: &Path,
    config: &Config,
    version_files: &[VersionFileConfig],
) -> Result<()> {
    let detected = ecosystem::detect(clone_path, Some(config));
    match detected {
        Ecosystem::Rust if clone_path.join("Cargo.lock").exists() => {
            let output = std::process::Command::new("cargo")
                .args(["generate-lockfile"])
                .current_dir(clone_path)
                .output()
                .context("failed to run cargo generate-lockfile")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("cargo generate-lockfile failed: {}", stderr.trim());
            }
            sync_cargo_lock_package_versions(clone_path, version_files)?;
        }
        Ecosystem::Python if clone_path.join("uv.lock").exists() => {
            let output = std::process::Command::new("uv")
                .args(["lock"])
                .current_dir(clone_path)
                .output()
                .context("failed to run uv lock")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("uv lock failed: {}", stderr.trim());
            }
        }
        _ => {}
    }
    Ok(())
}

fn sync_cargo_lock_package_versions(
    repo_root: &Path,
    version_files: &[VersionFileConfig],
) -> Result<()> {
    let cargo_tomls = version_files
        .iter()
        .filter(|vf| vf.path.ends_with("Cargo.toml"))
        .collect::<Vec<_>>();
    if cargo_tomls.is_empty() {
        return Ok(());
    }

    let lock_path = repo_root.join("Cargo.lock");
    let mut package_versions = BTreeMap::new();
    for version_file in cargo_tomls {
        let cargo_toml_path = repo_root.join(&version_file.path);
        let raw = fs::read_to_string(&cargo_toml_path)
            .with_context(|| format!("failed to read {}", cargo_toml_path.display()))?;
        let parsed: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse {}", cargo_toml_path.display()))?;
        let package = parsed
            .get("package")
            .and_then(toml::Value::as_table)
            .with_context(|| format!("missing [package] in {}", cargo_toml_path.display()))?;
        let name = package
            .get("name")
            .and_then(toml::Value::as_str)
            .with_context(|| format!("missing package.name in {}", cargo_toml_path.display()))?;
        let version = package
            .get("version")
            .and_then(toml::Value::as_str)
            .with_context(|| format!("missing package.version in {}", cargo_toml_path.display()))?;
        package_versions.insert(name.to_string(), version.to_string());
    }

    let raw_lock = fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read {}", lock_path.display()))?;
    let mut lines = raw_lock
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut changed = false;
    let mut index = 0;

    while index < lines.len() {
        if lines[index] != "[[package]]" {
            index += 1;
            continue;
        }

        let block_start = index;
        index += 1;
        let mut block_end = index;
        while block_end < lines.len() && lines[block_end] != "[[package]]" {
            block_end += 1;
        }

        let mut name: Option<String> = None;
        let mut version_index: Option<usize> = None;
        let mut has_source = false;
        for (line_index, line) in lines
            .iter()
            .enumerate()
            .take(block_end)
            .skip(block_start + 1)
        {
            if let Some(value) = line.strip_prefix("name = ") {
                name = Some(value.trim_matches('"').to_string());
            } else if line.starts_with("version = ") {
                version_index = Some(line_index);
            } else if line.starts_with("source = ") {
                has_source = true;
            }
        }

        if !has_source
            && let (Some(name), Some(version_index)) = (name.as_deref(), version_index)
            && let Some(target_version) = package_versions.get(name)
        {
            let desired_line = format!("version = \"{target_version}\"");
            if lines[version_index] != desired_line {
                lines[version_index] = desired_line;
                changed = true;
            }
        }

        index = block_end;
    }

    if changed {
        fs::write(&lock_path, format!("{}\n", lines.join("\n")))
            .with_context(|| format!("failed to write {}", lock_path.display()))?;
    }

    Ok(())
}

fn sync_prerelease_workspace_dependencies(
    repo_root: &Path,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    if !python_prerelease_workspace_applies(config, analysis) {
        return Ok(());
    }

    let selected_versions = selected_workspace_package_versions(analysis);
    if selected_versions.is_empty() {
        return Ok(());
    }

    sync_root_python_workspace_dependencies(
        repo_root,
        &selected_versions,
        config.prerelease.workspace.sync_root_dependencies,
        &config.prerelease.workspace.sync_root_extras,
    )?;

    Ok(())
}

fn sync_workspace_dependencies(
    repo_root: &Path,
    config: &Config,
    analysis: &ReleaseAnalysis,
    base_branch: &str,
) -> Result<()> {
    if config.project.ecosystem != Some(Ecosystem::Python) {
        return Ok(());
    }
    let workspace_plan = ReleaseWorkspacePlan::from_analysis(
        analysis,
        config.project.ecosystem,
        base_branch.to_string(),
    );
    sync_python_workspace_dependencies(repo_root, &workspace_plan, &config.workspace.dependencies)?;
    Ok(())
}

fn apply_release_replacements(
    repo_root: &Path,
    config: &Config,
    analysis: &ReleaseAnalysis,
    base_branch: &str,
) -> Result<Vec<replacements::ReplacementOperation>> {
    let plan = ReleaseWorkspacePlan::from_analysis(
        analysis,
        config.project.ecosystem,
        base_branch.to_string(),
    );
    replacements::apply(repo_root, &config.release.replacements, &plan)
}

#[derive(Debug, Deserialize)]
struct TransformerResult {
    schema_version: u32,
    #[serde(default)]
    changed_files: Vec<String>,
}

fn run_transformers(
    repo_root: &Path,
    config: &Config,
    analysis: &ReleaseAnalysis,
    base_branch: &str,
) -> Result<()> {
    let plan = ReleaseWorkspacePlan::from_analysis(
        analysis,
        config.project.ecosystem,
        base_branch.to_string(),
    );
    let plan_json = serde_json::to_vec(&plan)?;
    for transformer in &config.release.transformers {
        let before = workspace_snapshot(repo_root)?;
        let mut command = std::process::Command::new(&transformer.command[0]);
        command
            .args(&transformer.command[1..])
            .current_dir(repo_root)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start transformer {}", transformer.name))?;
        let mut stdin = child
            .stdin
            .take()
            .context("transformer stdin unavailable")?;
        let writer = std::thread::spawn({
            let plan_json = plan_json.clone();
            move || stdin.write_all(&plan_json)
        });
        let stdout = child
            .stdout
            .take()
            .context("transformer stdout unavailable")?;
        let stdout_reader = std::thread::spawn(move || read_stream(stdout));
        let stderr = child
            .stderr
            .take()
            .context("transformer stderr unavailable")?;
        let stderr_reader = std::thread::spawn(move || read_stream(stderr));
        let deadline = Instant::now() + Duration::from_secs(transformer.timeout_seconds);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                bail!(
                    "transformer {} timed out after {} seconds",
                    transformer.name,
                    transformer.timeout_seconds
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("transformer stdin writer panicked"))??;
        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow::anyhow!("transformer stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| anyhow::anyhow!("transformer stderr reader panicked"))??;
        if !status.success() {
            bail!(
                "transformer {} failed: {}",
                transformer.name,
                String::from_utf8_lossy(&stderr).trim()
            );
        }
        let result: TransformerResult = serde_json::from_slice(&stdout).with_context(|| {
            format!(
                "transformer {} must emit a JSON result on stdout",
                transformer.name
            )
        })?;
        if result.schema_version != 1 {
            bail!(
                "transformer {} returned unsupported schema version {}",
                transformer.name,
                result.schema_version
            );
        }
        let after = workspace_snapshot(repo_root)?;
        let actual = changed_snapshot_paths(&before, &after);
        let reported = result
            .changed_files
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if actual != reported {
            bail!(
                "transformer {} reported changed files that do not match its workspace changes",
                transformer.name
            );
        }
        for path in &actual {
            if !transformer
                .outputs
                .iter()
                .any(|pattern| glob_matches(pattern, path))
            {
                bail!(
                    "transformer {} modified undeclared output {}",
                    transformer.name,
                    path
                );
            }
        }
    }
    Ok(())
}

fn read_stream(mut stream: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn workspace_snapshot(repo_root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    snapshot_directory(repo_root, repo_root, &mut files)?;
    Ok(files)
}

fn snapshot_directory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if entry.file_name() != ".git" {
                snapshot_directory(root, &path, files)?;
            }
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, sha256(&fs::read(path)?).to_vec());
        }
    }
    Ok(())
}

fn changed_snapshot_paths(
    before: &BTreeMap<String, Vec<u8>>,
    after: &BTreeMap<String, Vec<u8>>,
) -> std::collections::BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.split('/').collect::<Vec<_>>();
    let path = path.split('/').collect::<Vec<_>>();
    glob_segments_match(&pattern, &path)
}

fn glob_segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern {
        [] => path.is_empty(),
        ["**", rest @ ..] => {
            (0..=path.len()).any(|index| glob_segments_match(rest, &path[index..]))
        }
        [segment, rest @ ..] => {
            path.first()
                .is_some_and(|part| segment_match(segment, part))
                && glob_segments_match(rest, &path[1..])
        }
    }
}

fn segment_match(pattern: &str, value: &str) -> bool {
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return pattern == value;
    }
    let mut remainder = value;
    if !pattern.starts_with('*') {
        let first = parts[0];
        let Some(after) = remainder.strip_prefix(first) else {
            return false;
        };
        remainder = after;
    }
    for part in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        let Some(index) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[index + part.len()..];
    }
    pattern.ends_with('*') || remainder.ends_with(parts.last().expect("non-empty glob parts"))
}

fn verify_prerelease_workspace(
    repo_root: &Path,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    if !python_prerelease_workspace_applies(config, analysis) {
        return Ok(());
    }

    let selected_versions = selected_workspace_package_versions(analysis);
    if selected_versions.is_empty() {
        return Ok(());
    }

    let build_dist = tempdir().context("failed to create prerelease verification dist dir")?;
    let dist_path = if config.prerelease.verify.build {
        run_uv_build(repo_root, ".", build_dist.path())?;
        for package in analysis
            .package_plan
            .selected_packages()
            .into_iter()
            .filter(|package| package.root != ".")
        {
            run_uv_build(repo_root, &package.root, build_dist.path())?;
        }
        build_dist.path().to_path_buf()
    } else {
        repo_root.join(&config.publish.dist_dir)
    };

    if config.prerelease.verify.inspect_wheel_metadata
        && !config.prerelease.workspace.sync_root_extras.is_empty()
    {
        let root = analysis
            .package_plan
            .selected_packages()
            .into_iter()
            .find(|package| package.root == ".")
            .context("prerelease workspace verification requires selected root package")?;
        let version = root
            .next_version
            .as_ref()
            .context("selected root package has no next version")?;
        let wheel = find_wheel(&dist_path, &root.name, &version.to_string())?;
        let metadata = read_wheel_metadata(&wheel)?;
        validate_root_wheel_metadata(
            &metadata,
            &selected_versions,
            &config.prerelease.workspace.sync_root_extras,
        )?;
    }

    Ok(())
}

fn python_prerelease_workspace_applies(config: &Config, analysis: &ReleaseAnalysis) -> bool {
    let selected_packages = analysis.package_plan.selected_packages();
    let is_prerelease = selected_packages.iter().any(|package| {
        package
            .next_version
            .as_ref()
            .is_some_and(|version| matches!(version.suffix, Some(Suffix::Pre(_))))
    });
    let is_finalize = selected_packages
        .iter()
        .any(|package| package.selection_reason == "finalize prerelease package");

    config.prerelease.enabled
        && config.project.ecosystem == Some(Ecosystem::Python)
        && config.monorepo.enabled
        && analysis.package_plan.release_mode == "release_set"
        && (is_prerelease || is_finalize)
        && selected_packages.iter().any(|package| package.root == ".")
}

fn selected_workspace_package_versions(analysis: &ReleaseAnalysis) -> BTreeMap<String, String> {
    analysis
        .package_plan
        .selected_packages()
        .into_iter()
        .filter(|package| package.root != ".")
        .filter_map(|package| {
            let version = package.next_version.as_ref()?;
            Some((package.name.clone(), version.to_string()))
        })
        .collect()
}

fn run_uv_build(repo_root: &Path, package_root: &str, dist_dir: &Path) -> Result<()> {
    let mut command = std::process::Command::new("uv");
    command.arg("build");
    if package_root != "." {
        command.arg("--directory").arg(package_root);
    }
    command.arg("--out-dir").arg(dist_dir);
    let output = command
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("failed to run uv build for {package_root}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("uv build failed for {package_root}: {}", stderr.trim());
    }
    Ok(())
}

fn find_wheel(dist_dir: &Path, package_name: &str, version: &str) -> Result<std::path::PathBuf> {
    let normalized = normalize_wheel_distribution(package_name);
    let prefix = format!("{normalized}-{version}-");
    let entries = fs::read_dir(dist_dir)
        .with_context(|| format!("failed to read build artifacts from {}", dist_dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_name.starts_with(&prefix) && file_name.ends_with(".whl") {
            return Ok(path);
        }
    }

    bail!(
        "no root wheel found for {package_name} {version} in {}",
        dist_dir.display()
    )
}

fn normalize_wheel_distribution(package_name: &str) -> String {
    let mut normalized = String::new();
    let mut in_separator = false;
    for ch in package_name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !in_separator {
                normalized.push('_');
                in_separator = true;
            }
            continue;
        }

        in_separator = false;
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push('_');
        }
    }
    normalized
}

fn read_wheel_metadata(wheel_path: &Path) -> Result<String> {
    let output = std::process::Command::new("unzip")
        .arg("-p")
        .arg(wheel_path)
        .arg("*.dist-info/METADATA")
        .output()
        .with_context(|| format!("failed to inspect {}", wheel_path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "failed to read wheel metadata from {}: {}",
            wheel_path.display(),
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

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

pub fn build_release_pr_plan(
    config: &Config,
    analysis: &ReleaseAnalysis,
    current_branch: &str,
) -> Result<ReleasePrPlan> {
    let release_label = release_label(analysis)?;
    let release_notes_label = release_notes_label(analysis)?;
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
    let base = channels::release_base_branch(config, current_branch);
    let branch = format!(
        "{}/{}",
        config.github.release_branch_prefix.trim_end_matches('/'),
        release_branch_suffix(analysis, &base)?
    );
    let date = today_utc();
    let release_notes = changelog::render_release_notes(
        &release_notes_label,
        &date,
        &analysis.changelog,
        &config.changelog.first_contribution_emoji,
    );
    let prerelease_body = prerelease_pr_body(config, analysis)?;
    let body = format!("## Release summary\n\n{release_notes}") + &prerelease_body;

    Ok(ReleasePrPlan {
        version,
        branch,
        base,
        title,
        body,
        labels: vec![config.github.pending_label.clone()],
        release_notes,
    })
}

/// Linkify `#123` references and append a compare-range footer. Best
/// effort: without a detectable repo the notes pass through unchanged.
pub(crate) fn enrich_release_notes(
    repo: &GitRepository,
    config: &Config,
    notes: &str,
    base_tag: Option<&str>,
    head_tag: &str,
) -> String {
    let Ok(repo_ref) = detect_repo(repo, &config.github) else {
        return notes.to_string();
    };
    let base = changelog::web_base(&config.github.api_base);
    let linked = changelog::linkify_github_refs(notes, &base, &repo_ref.owner, &repo_ref.name);
    match base_tag {
        Some(base_tag) => changelog::append_compare_link(
            &linked,
            &base,
            &repo_ref.owner,
            &repo_ref.name,
            base_tag,
            head_tag,
        ),
        None => linked,
    }
}

fn enrich_pr_plan(repo: &GitRepository, config: &Config, plan: &mut ReleasePrPlan) {
    let base_tag = repo.latest_tag().ok().flatten();
    let head_tag = format!("{}{}", config.release.tag_prefix, plan.version);
    plan.release_notes = enrich_release_notes(
        repo,
        config,
        &plan.release_notes,
        base_tag.as_deref(),
        &head_tag,
    );
    plan.body = enrich_release_notes(repo, config, &plan.body, None, &head_tag);
}

fn enrich_tag_plan(repo: &GitRepository, config: &Config, plan: &mut ReleaseTagPlan) {
    let base_tag = repo.latest_tag().ok().flatten();
    plan.release_notes = enrich_release_notes(
        repo,
        config,
        &plan.release_notes,
        base_tag.as_deref(),
        &plan.tag_name,
    );
}

fn prerelease_pr_body(config: &Config, analysis: &ReleaseAnalysis) -> Result<String> {
    if !config.prerelease.enabled || analysis.package_plan.release_mode != "release_set" {
        return Ok(String::new());
    }

    let packages = selected_prerelease_packages(analysis);
    if packages.is_empty() {
        return Ok(String::new());
    }

    let is_beta = analysis
        .package_plan
        .selected_packages()
        .iter()
        .any(|package| {
            package
                .next_version
                .as_ref()
                .is_some_and(|version| matches!(version.suffix, Some(Suffix::Pre(_))))
        });
    let is_finalize = !is_beta
        && analysis
            .package_plan
            .selected_packages()
            .iter()
            .any(|package| package.selection_reason == "finalize prerelease package");

    if !is_beta && !is_finalize {
        return Ok(String::new());
    }

    let heading = if is_finalize {
        "Finalize Prerelease"
    } else {
        "Prerelease"
    };
    let mut body = format!("\n\n## {heading}\n\n");
    if is_beta {
        let root = packages
            .iter()
            .find(|package| package.root == ".")
            .context("prerelease release set has no root package")?;
        body.push_str(&format!("Beta release: `{}`\n\n", root.version));
    }

    body.push_str("## Packages\n\n| Package | Version | Reason |\n|---|---:|---|\n");
    for package in &packages {
        body.push_str(&format!(
            "| {} | {} | {} |\n",
            package.name, package.version, package.reason
        ));
    }

    if is_beta
        && config.prerelease.verify.emit_install_command
        && !config.prerelease.workspace.sync_root_extras.is_empty()
    {
        let root = packages
            .iter()
            .find(|package| package.root == ".")
            .context("prerelease release set has no root package")?;
        body.push_str("\n## PyPI Verification Command\n\n```bash\n");
        for (index, extra) in config
            .prerelease
            .workspace
            .sync_root_extras
            .iter()
            .enumerate()
        {
            if index > 0 {
                body.push_str("\n\n");
            }
            let command =
                build_explicit_install_command(&root.name, &root.version, extra, &packages);
            body.push_str(&command);
        }
        body.push_str("\n```\n");
    }

    Ok(body)
}

fn selected_prerelease_packages(analysis: &ReleaseAnalysis) -> Vec<PrereleasePackage> {
    analysis
        .package_plan
        .selected_packages()
        .into_iter()
        .filter_map(|package| {
            let version = package.next_version.as_ref()?;
            Some(PrereleasePackage {
                name: package.name.clone(),
                version: version.to_string(),
                root: package.root.clone(),
                reason: package.selection_reason.clone(),
            })
        })
        .collect()
}

pub fn build_release_tag_plan(
    config: &Config,
    repo: &GitRepository,
    analysis: &ReleaseAnalysis,
) -> Result<ReleaseTagPlan> {
    let release_label = release_label(analysis)?;
    let release_notes_label = release_notes_label(analysis)?;
    let version = if analysis.package_plan.release_mode == "release_set" {
        release_set_root_version(analysis)?.unwrap_or(release_set_title_label(analysis)?)
    } else {
        analysis
            .next_version
            .clone()
            .or_else(|| analysis.bump.apply(&analysis.current_version))
            .map(|version| version.to_string())
            .unwrap_or_else(|| release_label.clone())
    };
    let tag_name = if analysis.package_plan.release_mode == "release_set" {
        if let Some(version) = release_set_root_version(analysis)? {
            format!("{}{}", config.release.tag_prefix, version)
        } else {
            format!(
                "{}{}",
                config.release.tag_prefix,
                monorepo_release_slug(analysis)?
            )
        }
    } else if config.monorepo.enabled && analysis.package_plan.release_mode != "unified" {
        format!(
            "{}{}",
            config.release.tag_prefix,
            monorepo_release_slug(analysis)?
        )
    } else {
        format!("{}{}", config.release.tag_prefix, version)
    };
    let title = config
        .release
        .release_name
        .replace("{tag_name}", &tag_name)
        .replace("{version}", &version);
    Ok(ReleaseTagPlan {
        version,
        title,
        target: repo.current_branch()?,
        release_notes: changelog::render_release_notes(
            &release_notes_label,
            &today_utc(),
            &analysis.changelog,
            &config.changelog.first_contribution_emoji,
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
    let current_branch = repo.current_branch()?;
    let mut plan = build_release_pr_plan(config, analysis, &current_branch)?;
    enrich_pr_plan(repo, config, &mut plan);
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
    let auth_url = authenticated_url(&origin_url, &token);
    run_git(
        &clone_path,
        ["remote", "set-url", "origin", auth_url.as_str()],
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
    apply_release_replacements(&clone_path, config, analysis, &plan.base)?;
    changelog::prepend_release_notes(
        &clone_path.join(&config.release.changelog_file),
        &plan.release_notes,
    )?;
    refresh_lockfile(&clone_path, config, &config.version_files)?;

    run_git(&clone_path, ["add", "."])?;
    let diff = run_git(&clone_path, ["status", "--short"])?;
    if diff.trim().is_empty() {
        bail!("release PR would not change any files");
    }

    run_git(
        &clone_path,
        release_commit_args(config, plan.title.as_str()),
    )?;
    run_git(
        &clone_path,
        [
            "push",
            "--force",
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

/// Runs the complete local release-set mutation pipeline in an isolated clone.
/// It deliberately performs no remote or GitHub operation.
pub fn prepare_release_workspace_check(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let current_branch = repo.current_branch()?;
    let mut plan = build_release_pr_plan(config, analysis, &current_branch)?;
    enrich_pr_plan(repo, config, &mut plan);
    let clone_dir = tempdir().context("failed to create temporary workspace")?;
    let clone_path = clone_dir.path().join("repo");
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
        [
            "checkout",
            "-B",
            "relx-prepare-check",
            format!("origin/{}", plan.base).as_str(),
        ],
    )?;

    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        bail!("no releasable packages found");
    }
    for package in selected {
        let next_version = package
            .next_version
            .as_ref()
            .context("selected package has no next version")?;
        analysis::update_version_files(&clone_path, &package.version_files, next_version)?;
    }
    sync_workspace_dependencies(&clone_path, config, analysis, &plan.base)?;
    sync_prerelease_workspace_dependencies(&clone_path, config, analysis)?;
    apply_release_replacements(&clone_path, config, analysis, &plan.base)?;
    run_transformers(&clone_path, config, analysis, &plan.base)?;
    changelog::prepend_release_notes(
        &clone_path.join(&config.release.changelog_file),
        &plan.release_notes,
    )?;
    let version_files = analysis
        .package_plan
        .selected_packages()
        .into_iter()
        .flat_map(|package| package.version_files.iter().cloned())
        .collect::<Vec<_>>();
    if !python_prerelease_workspace_applies(config, analysis) || config.prerelease.verify.lock {
        refresh_lockfile(&clone_path, config, &version_files)?;
    }
    verify_prerelease_workspace(&clone_path, config, analysis)?;
    println!("Release workspace prepared and validated locally; no branch or PR was changed.");
    Ok(())
}

pub fn execute_monorepo_release_pr(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        println!("No releasable packages found in monorepo; release PR not created.");
        return Ok(());
    }

    if monorepo_single_pr_mode(config.monorepo.release_mode.as_str()) {
        execute_monorepo_unified_pr(repo, config, analysis, &selected)?;
    } else {
        for package in &selected {
            let package_analysis = single_package_analysis(analysis, package);
            execute_monorepo_per_package_pr(repo, config, &package_analysis, package)?;
        }
    }

    Ok(())
}

fn execute_monorepo_unified_pr(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
    selected: &[&analysis::PackageReleaseAnalysis],
) -> Result<()> {
    let current_branch = repo.current_branch()?;
    let mut plan = build_release_pr_plan(config, analysis, &current_branch)?;
    enrich_pr_plan(repo, config, &mut plan);
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
    let auth_url = authenticated_url(&origin_url, &token);
    run_git(
        &clone_path,
        ["remote", "set-url", "origin", auth_url.as_str()],
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

    for package in selected {
        let next_version = package
            .next_version
            .as_ref()
            .context("selected package has no next version")?;
        analysis::update_version_files(&clone_path, &package.version_files, next_version)?;
    }
    sync_workspace_dependencies(&clone_path, config, analysis, &plan.base)?;
    sync_prerelease_workspace_dependencies(&clone_path, config, analysis)?;
    apply_release_replacements(&clone_path, config, analysis, &plan.base)?;
    run_transformers(&clone_path, config, analysis, &plan.base)?;
    changelog::prepend_release_notes(
        &clone_path.join(&config.release.changelog_file),
        &plan.release_notes,
    )?;
    let version_files = selected
        .iter()
        .flat_map(|package| package.version_files.iter().cloned())
        .collect::<Vec<_>>();
    if !python_prerelease_workspace_applies(config, analysis) || config.prerelease.verify.lock {
        refresh_lockfile(&clone_path, config, &version_files)?;
    }
    verify_prerelease_workspace(&clone_path, config, analysis)?;

    run_git(&clone_path, ["add", "."])?;
    let diff = run_git(&clone_path, ["status", "--short"])?;
    if diff.trim().is_empty() {
        bail!("release PR would not change any files");
    }

    run_git(
        &clone_path,
        release_commit_args(config, plan.title.as_str()),
    )?;
    run_git(
        &clone_path,
        [
            "push",
            "--force",
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

fn execute_monorepo_per_package_pr(
    repo: &GitRepository,
    config: &Config,
    package_analysis: &ReleaseAnalysis,
    package: &analysis::PackageReleaseAnalysis,
) -> Result<()> {
    let current_branch = repo.current_branch()?;
    let mut plan = build_release_pr_plan(config, package_analysis, &current_branch)?;
    enrich_pr_plan(repo, config, &mut plan);
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
    let auth_url = authenticated_url(&origin_url, &token);
    run_git(
        &clone_path,
        ["remote", "set-url", "origin", auth_url.as_str()],
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

    let next_version = package
        .next_version
        .as_ref()
        .context("selected package has no next version")?;
    analysis::update_version_files(&clone_path, &package.version_files, next_version)?;
    apply_release_replacements(&clone_path, config, package_analysis, &plan.base)?;

    let changelog_path = if package.root == "." {
        config.release.changelog_file.clone()
    } else {
        format!("{}/{}", package.root, config.release.changelog_file)
    };
    changelog::prepend_release_notes(&clone_path.join(&changelog_path), &plan.release_notes)?;
    refresh_lockfile(&clone_path, config, &package.version_files)?;

    run_git(&clone_path, ["add", "."])?;
    let diff = run_git(&clone_path, ["status", "--short"])?;
    if diff.trim().is_empty() {
        println!(
            "Skipping {} — release PR would not change any files",
            package.name
        );
        return Ok(());
    }

    run_git(
        &clone_path,
        release_commit_args(config, plan.title.as_str()),
    )?;
    run_git(
        &clone_path,
        [
            "push",
            "--force",
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

    println!(
        "Release PR ready for {}: #{} {}",
        package.name, pr.number, plan.title
    );
    println!("Branch: {}", plan.branch);
    Ok(())
}

pub fn execute_release_tag(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let mut plan = build_release_tag_plan(config, repo, analysis)?;
    enrich_tag_plan(repo, config, &mut plan);
    let repo_ref = detect_repo(repo, &config.github)?;
    let token = env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    let client = GitHubClient::new(&config.github.api_base, &token, repo_ref)?;

    run_git(
        repo.path(),
        release_tag_args(config, plan.tag_name.as_str(), plan.title.as_str()),
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

pub fn execute_monorepo_release_tag(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        bail!("no releasable packages found in monorepo");
    }

    if monorepo_single_tag_mode(config.monorepo.release_mode.as_str()) {
        return execute_release_tag(repo, config, analysis);
    }

    let repo_ref = detect_repo(repo, &config.github)?;
    let token = env::var(&config.github.token_env)
        .with_context(|| format!("missing GitHub token in {}", config.github.token_env))?;
    let client = GitHubClient::new(&config.github.api_base, &token, repo_ref)?;

    for package in &selected {
        let package_analysis = single_package_analysis(analysis, package);
        let mut plan = build_release_tag_plan(config, repo, &package_analysis)?;
        enrich_tag_plan(repo, config, &mut plan);

        run_git(
            repo.path(),
            release_tag_args(config, plan.tag_name.as_str(), plan.title.as_str()),
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

        println!("Release tagged for {}: {}", package.name, plan.tag_name);
    }

    Ok(())
}

fn single_package_analysis(
    analysis: &ReleaseAnalysis,
    package: &analysis::PackageReleaseAnalysis,
) -> ReleaseAnalysis {
    ReleaseAnalysis {
        current_version: package.current_version.clone(),
        next_version: package.next_version.clone(),
        bump: package.bump,
        commits: package.commits.clone(),
        changelog: package.changelog.clone(),
        package_plan: analysis::PackagePlan {
            release_mode: "single".to_string(),
            discovery_source: analysis.package_plan.discovery_source.clone(),
            packages: vec![analysis::PackageReleaseAnalysis {
                name: package.name.clone(),
                root: package.root.clone(),
                current_version: package.current_version.clone(),
                next_version: package.next_version.clone(),
                bump: package.bump,
                changelog: package.changelog.clone(),
                version_files: package.version_files.clone(),
                commits: package.commits.clone(),
                changed_paths: package.changed_paths.clone(),
                selected: true,
                selection_reason: package.selection_reason.clone(),
            }],
        },
    }
}

pub fn print_release_pr_dry_run(
    repo: &GitRepository,
    config: &Config,
    analysis: &ReleaseAnalysis,
) -> Result<()> {
    let repo_ref = detect_repo(repo, &config.github)?;
    let current_branch = repo.current_branch()?;
    let mut plan = build_release_pr_plan(config, analysis, &current_branch)?;
    enrich_pr_plan(repo, config, &mut plan);
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
    let workspace_plan =
        ReleaseWorkspacePlan::from_analysis(analysis, config.project.ecosystem, plan.base.clone());
    println!("Workspace plan:");
    println!("{}", serde_json::to_string_pretty(&workspace_plan)?);
    if config.workspace.dependencies.enabled {
        println!(
            "Would synchronize {} declared workspace dependency rule(s) before refreshing lockfiles",
            config.workspace.dependencies.rules.len()
        );
    }
    let replacement_operations = replacements::planned_operations(
        repo.path(),
        &config.release.replacements,
        &workspace_plan,
    )?;
    for operation in replacement_operations {
        println!(
            "Would replace {} literal match(es) for package `{}` in `{}`: `{}` -> `{}`",
            operation.matches,
            operation.package,
            operation.file,
            operation.search,
            operation.replace
        );
    }
    for transformer in &config.release.transformers {
        println!(
            "Would run transformer `{}`: {}",
            transformer.name,
            transformer.command.join(" ")
        );
    }
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
    let mut plan = build_release_tag_plan(config, repo, analysis)?;
    enrich_tag_plan(repo, config, &mut plan);
    if config.monorepo.enabled {
        println!(
            "Would tag this release set for {} mode: {}",
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

fn release_branch_suffix(analysis: &ReleaseAnalysis, base_branch: &str) -> Result<String> {
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

    if analysis.package_plan.release_mode == "unified"
        || analysis.package_plan.release_mode == "release_set"
    {
        return Ok(format!(
            "monorepo/{}",
            stable_monorepo_branch_slug(base_branch, &analysis.package_plan.release_mode)
        ));
    }

    Ok(format!("per-package/{}", selected.len()))
}

fn stable_monorepo_branch_slug(base_branch: &str, release_mode: &str) -> String {
    format!(
        "{}-{}",
        sanitize_label(base_branch).trim_matches('-'),
        sanitize_label(release_mode).trim_matches('-')
    )
}

fn monorepo_release_slug(analysis: &ReleaseAnalysis) -> Result<String> {
    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        bail!("no releasable package set is pending from the current commit set");
    }

    let readable = sanitize_label(
        &selected
            .iter()
            .take(2)
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>()
            .join("-"),
    );
    let readable = truncate_label(&readable, 24);
    let digest_input = selected_package_summaries(analysis).join("|");
    let digest = short_digest(&digest_input);

    Ok(format!("{}pkgs-{}-{}", selected.len(), readable, digest))
}

fn monorepo_pr_title(config: &Config, analysis: &ReleaseAnalysis) -> Result<String> {
    let selected = selected_package_summaries(analysis);
    if selected.is_empty() {
        bail!("no releasable package set is pending from the current commit set");
    }

    if analysis.package_plan.release_mode == "unified" {
        return Ok(format!("chore(release): {}", selected.join(", ")));
    }

    if analysis.package_plan.release_mode == "release_set" {
        let label = release_set_title_label(analysis)?;
        return Ok(config.release.pr_title.replace("{version}", &label));
    }

    Ok(format!(
        "{} package release set",
        config
            .release
            .pr_title
            .replace("{version}", &format!("{} packages", selected.len()))
    ))
}

fn monorepo_single_pr_mode(mode: &str) -> bool {
    matches!(mode, "unified" | "release_set")
}

fn monorepo_single_tag_mode(mode: &str) -> bool {
    matches!(mode, "unified" | "release_set")
}

fn release_notes_label(analysis: &ReleaseAnalysis) -> Result<String> {
    if analysis.package_plan.release_mode == "release_set" {
        return release_set_title_label(analysis);
    }

    release_label(analysis)
}

fn release_set_title_label(analysis: &ReleaseAnalysis) -> Result<String> {
    let selected = analysis.package_plan.selected_packages();
    if selected.is_empty() {
        bail!("no releasable package set is pending from the current commit set");
    }

    let root_package = selected.iter().find(|package| package.root == ".");
    if selected.len() == 1 {
        let package = selected[0];
        let version = package
            .next_version
            .as_ref()
            .context("selected package has no next version")?;
        return Ok(format!("{} {}", package.name, version));
    }

    if let Some(package) = root_package {
        let version = package
            .next_version
            .as_ref()
            .context("selected package has no next version")?;
        return Ok(format!(
            "{} {} + {} packages",
            package.name,
            version,
            selected.len() - 1
        ));
    }

    Ok(format!("{} packages", selected.len()))
}

fn release_set_root_version(analysis: &ReleaseAnalysis) -> Result<Option<String>> {
    let selected = analysis.package_plan.selected_packages();
    let Some(root_package) = selected.iter().find(|package| package.root == ".") else {
        return Ok(None);
    };

    let version = root_package
        .next_version
        .as_ref()
        .context("selected package has no next version")?;
    Ok(Some(version.to_string()))
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

fn truncate_label(value: &str, max_len: usize) -> String {
    value.chars().take(max_len).collect()
}

fn short_digest(value: &str) -> String {
    hex_digest(&sha256(value.as_bytes()))[..12].to_string()
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
pub struct PullRequest {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub html_url: String,
    pub head: Option<PullRequestHead>,
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub id: u64,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestHead {
    pub sha: String,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestBranchRef {
    #[serde(rename = "ref", default)]
    pub ref_name: String,
    pub sha: String,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestDetails {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
    #[serde(default)]
    pub body: String,
    pub head: PullRequestBranchRef,
    pub base: PullRequestBranchRef,
}

impl PullRequestDetails {
    pub fn has_marker(&self, marker: &str) -> bool {
        self.body.contains(marker)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct IssueComment {
    pub id: u64,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestReview {
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct CombinedStatus {
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct CommitDetails {
    pub author: Option<GitHubUser>,
    pub committer: Option<GitHubUser>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubUser {
    pub login: String,
}

pub struct GitHubClient {
    api_base: String,
    token: String,
    repo: RepoRef,
}

impl GitHubClient {
    pub fn new(api_base: &str, token: &str, repo: RepoRef) -> Result<Self> {
        Ok(Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            repo,
        })
    }

    pub fn find_open_pr(
        &self,
        head_branch: &str,
        base_branch: &str,
    ) -> Result<Option<PullRequest>> {
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

    pub fn create_pr(
        &self,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
    ) -> Result<PullRequest> {
        self.post(
            &format!(
                "{}/repos/{}/{}/pulls",
                self.api_base, self.repo.owner, self.repo.name
            ),
            &json!({ "title": title, "head": head, "base": base, "body": body }),
        )
    }

    pub fn update_pr(&self, number: u64, title: &str, body: &str) -> Result<PullRequest> {
        self.patch(
            &format!(
                "{}/repos/{}/{}/pulls/{}",
                self.api_base, self.repo.owner, self.repo.name, number
            ),
            &json!({ "title": title, "body": body }),
        )
    }

    pub fn ensure_label(&self, name: &str) -> Result<()> {
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
            &json!({ "name": name, "color": "ededed", "description": "Managed by relx" }),
        )?;
        Ok(())
    }

    pub fn add_labels(&self, number: u64, labels: &[String]) -> Result<()> {
        let _: serde_json::Value = self.post(
            &format!(
                "{}/repos/{}/{}/issues/{}/labels",
                self.api_base, self.repo.owner, self.repo.name, number
            ),
            &json!({ "labels": labels }),
        )?;
        Ok(())
    }

    pub fn find_release_by_tag(&self, tag: &str) -> Result<Option<Release>> {
        let url = format!(
            "{}/repos/{}/{}/releases/tags/{}",
            self.api_base, self.repo.owner, self.repo.name, tag
        );
        match self.get_raw(&url) {
            Ok(response) => Ok(Some(parse_json(response)?)),
            Err(_) => Ok(None),
        }
    }

    pub fn list_reviews(&self, number: u64) -> Result<Vec<PullRequestReview>> {
        self.get(&format!(
            "{}/repos/{}/{}/pulls/{}/reviews",
            self.api_base, self.repo.owner, self.repo.name, number
        ))
    }

    pub fn combined_status(&self, reference: &str) -> Result<CombinedStatus> {
        self.get(&format!(
            "{}/repos/{}/{}/commits/{}/status",
            self.api_base, self.repo.owner, self.repo.name, reference
        ))
    }

    pub fn find_merged_pr(
        &self,
        head_branch: &str,
        base_branch: &str,
    ) -> Result<Option<PullRequestDetails>> {
        let url = format!(
            "{}/repos/{}/{}/pulls?state=closed&head={}:{}&base={}&sort=updated&direction=desc&per_page=10",
            self.api_base,
            self.repo.owner,
            self.repo.name,
            self.repo.owner,
            head_branch,
            base_branch
        );
        let prs: Vec<PullRequest> = self.get(&url)?;
        for pr in prs {
            let details = self.get_pr(pr.number)?;
            if details.merged {
                return Ok(Some(details));
            }
        }
        Ok(None)
    }

    pub fn commit_details(&self, sha: &str) -> Result<CommitDetails> {
        self.get(&format!(
            "{}/repos/{}/{}/commits/{}",
            self.api_base, self.repo.owner, self.repo.name, sha
        ))
    }

    pub fn get_pr(&self, number: u64) -> Result<PullRequestDetails> {
        self.get(&format!(
            "{}/repos/{}/{}/pulls/{}",
            self.api_base, self.repo.owner, self.repo.name, number
        ))
    }

    pub fn list_issue_comments(&self, number: u64) -> Result<Vec<IssueComment>> {
        self.get(&format!(
            "{}/repos/{}/{}/issues/{}/comments?per_page=100",
            self.api_base, self.repo.owner, self.repo.name, number
        ))
    }

    pub fn create_issue_comment(&self, number: u64, body: &str) -> Result<IssueComment> {
        self.post(
            &format!(
                "{}/repos/{}/{}/issues/{}/comments",
                self.api_base, self.repo.owner, self.repo.name, number
            ),
            &json!({ "body": body }),
        )
    }

    pub fn update_issue_comment(&self, comment_id: u64, body: &str) -> Result<IssueComment> {
        self.patch(
            &format!(
                "{}/repos/{}/{}/issues/comments/{}",
                self.api_base, self.repo.owner, self.repo.name, comment_id
            ),
            &json!({ "body": body }),
        )
    }

    pub fn token_scopes(&self) -> Result<Vec<String>> {
        let url = format!("{}/user", self.api_base);
        let response = ureq::get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "relx")
            .call();

        match response {
            Ok(response) => {
                let scopes = response
                    .header("X-OAuth-Scopes")
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|scope| !scope.is_empty())
                    .map(ToString::to_string)
                    .collect();
                Ok(scopes)
            }
            Err(ureq::Error::Status(status, response)) => {
                let body = response.into_string().unwrap_or_default();
                bail!("GitHub API request failed with status {status}: {body}")
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn create_release(
        &self,
        tag: &str,
        name: &str,
        body: &str,
        target: &str,
    ) -> Result<Release> {
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

    pub fn update_release(&self, release_id: u64, name: &str, body: &str) -> Result<Release> {
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
            .set("User-Agent", "relx")
            .call();
        read_response(response)
    }

    fn post<T: for<'de> Deserialize<'de>, B: Serialize>(&self, url: &str, body: &B) -> Result<T> {
        let response = ureq::post(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "relx")
            .send_json(body);
        parse_json(read_response(response)?)
    }

    fn patch<T: for<'de> Deserialize<'de>, B: Serialize>(&self, url: &str, body: &B) -> Result<T> {
        let response = ureq::request("PATCH", url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "relx")
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
    use super::{
        build_release_pr_plan, build_release_tag_plan, normalize_wheel_distribution,
        parse_remote_url, single_package_analysis,
    };
    use crate::{
        analysis::{PackagePlan, PackageReleaseAnalysis, ReleaseAnalysis},
        changelog::PendingChangelog,
        config::Config,
        git::GitRepository,
        version::{BumpLevel, PreRelease, Suffix, Version},
    };
    use std::{collections::BTreeMap, fs};
    use tempfile::tempdir;

    #[test]
    fn parses_common_github_remote_formats() {
        assert_eq!(
            parse_remote_url("git@github.com:acme/relx.git"),
            Some(super::RepoRef {
                owner: "acme".into(),
                name: "relx".into()
            })
        );
        assert_eq!(
            parse_remote_url("https://github.com/acme/relx.git"),
            Some(super::RepoRef {
                owner: "acme".into(),
                name: "relx".into()
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

        let plan = build_release_pr_plan(&config, &analysis, "main").expect("plan");
        assert_eq!(plan.branch, "relx/release/v1.2.0");
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
        run(dir.path(), &["git", "config", "user.name", "Relx Test"]);
        run(
            dir.path(),
            &["git", "config", "user.email", "relx@example.com"],
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

        let plan = build_release_pr_plan(&config, &analysis, "main").expect("plan");
        assert_eq!(plan.branch, "relx/release/monorepo/main-unified");
        assert!(plan.branch.len() < 64, "{}", plan.branch);
        assert!(plan.title.contains("core 1.2.0"), "{}", plan.title);
        assert!(plan.title.contains("cli 0.5.1"), "{}", plan.title);
    }

    #[test]
    fn builds_release_set_monorepo_release_pr_plan() {
        let config: Config = toml::from_str(
            r#"
            [release]
            pr_title = "chore(release): {version}"

            [monorepo]
            enabled = true
            release_mode = "release_set"
            packages = [".", "packages/delta"]
            "#,
        )
        .expect("config");
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
                        commits: Vec::new(),
                        changed_paths: vec!["pyproject.toml".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                    PackageReleaseAnalysis {
                        name: "phlo-delta".to_string(),
                        root: "packages/delta".to_string(),
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
                        commits: Vec::new(),
                        changed_paths: vec!["packages/delta/src/mod.py".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                ],
            },
        };

        let plan = build_release_pr_plan(&config, &analysis, "main").expect("plan");
        assert_eq!(plan.branch, "relx/release/monorepo/main-release-set");
        assert_eq!(plan.title, "chore(release): phlo 0.7.3 + 1 packages");
        assert!(
            plan.body.contains("## [phlo 0.7.3 + 1 packages] - "),
            "{}",
            plan.body
        );
    }

    #[test]
    fn monorepo_release_pr_noops_when_no_packages_are_releasable() {
        let dir = tempdir().expect("tempdir");
        run(dir.path(), &["git", "init", "-b", "main"]);
        let repo = GitRepository::discover(dir.path()).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [monorepo]
            enabled = true
            release_mode = "release_set"
            "#,
        )
        .expect("config");
        let analysis = ReleaseAnalysis {
            current_version: Version {
                major: 0,
                minor: 8,
                patch: 3,
                suffix: None,
            },
            next_version: None,
            bump: BumpLevel::None,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::new(),
                contributors: Vec::new(),
            },
            package_plan: PackagePlan {
                release_mode: "release_set".to_string(),
                discovery_source: "test".to_string(),
                packages: vec![PackageReleaseAnalysis {
                    name: "phlo-api".to_string(),
                    root: "packages/phlo-api".to_string(),
                    current_version: Version {
                        major: 0,
                        minor: 3,
                        patch: 1,
                        suffix: None,
                    },
                    next_version: None,
                    bump: BumpLevel::None,
                    changelog: PendingChangelog {
                        sections: BTreeMap::new(),
                        contributors: Vec::new(),
                    },
                    version_files: Vec::new(),
                    commits: Vec::new(),
                    changed_paths: vec![
                        "packages/phlo-api/src/phlo_api/observatory_api/v2.py".to_string(),
                    ],
                    selected: false,
                    selection_reason: "no releasable package changes detected since the latest tag"
                        .to_string(),
                }],
            },
        };

        super::execute_monorepo_release_pr(&repo, &config, &analysis).expect("no-op release pr");
    }

    #[test]
    fn release_set_monorepo_branch_stays_stable_as_package_set_grows() {
        let config: Config = toml::from_str(
            r#"
            [release]
            pr_title = "chore(release): {version}"

            [monorepo]
            enabled = true
            release_mode = "release_set"
            packages = [".", "packages/dbt", "packages/lineage"]
            "#,
        )
        .expect("config");

        let root_only = ReleaseAnalysis {
            current_version: Version {
                major: 0,
                minor: 7,
                patch: 7,
                suffix: None,
            },
            next_version: Some(Version {
                major: 0,
                minor: 7,
                patch: 8,
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
                packages: vec![PackageReleaseAnalysis {
                    name: "phlo".to_string(),
                    root: ".".to_string(),
                    current_version: Version {
                        major: 0,
                        minor: 7,
                        patch: 7,
                        suffix: None,
                    },
                    next_version: Some(Version {
                        major: 0,
                        minor: 7,
                        patch: 8,
                        suffix: None,
                    }),
                    bump: BumpLevel::Patch,
                    changelog: PendingChangelog {
                        sections: BTreeMap::new(),
                        contributors: Vec::new(),
                    },
                    version_files: Vec::new(),
                    commits: Vec::new(),
                    changed_paths: vec!["pyproject.toml".to_string()],
                    selected: true,
                    selection_reason: "test".to_string(),
                }],
            },
        };
        let expanded = ReleaseAnalysis {
            package_plan: PackagePlan {
                release_mode: "release_set".to_string(),
                discovery_source: "test".to_string(),
                packages: vec![
                    root_only.package_plan.packages[0].clone(),
                    PackageReleaseAnalysis {
                        name: "phlo-dbt".to_string(),
                        root: "packages/dbt".to_string(),
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
                        commits: Vec::new(),
                        changed_paths: vec!["packages/dbt/src/phlo_dbt/cli.py".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                    PackageReleaseAnalysis {
                        name: "phlo-lineage".to_string(),
                        root: "packages/lineage".to_string(),
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
                        commits: Vec::new(),
                        changed_paths: vec![
                            "packages/lineage/src/phlo_lineage/store.py".to_string(),
                        ],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                ],
            },
            ..root_only.clone()
        };

        let initial_plan = build_release_pr_plan(&config, &root_only, "main").expect("root plan");
        let expanded_plan =
            build_release_pr_plan(&config, &expanded, "main").expect("expanded plan");

        assert_eq!(
            initial_plan.branch,
            "relx/release/monorepo/main-release-set"
        );
        assert_eq!(expanded_plan.branch, initial_plan.branch);
        assert_eq!(initial_plan.title, "chore(release): phlo 0.7.8");
        assert_eq!(
            expanded_plan.title,
            "chore(release): phlo 0.7.8 + 2 packages"
        );
    }

    #[test]
    fn prerelease_release_set_pr_body_includes_package_table_and_explicit_install_command() {
        let config: Config = toml::from_str(
            r#"
            [release]
            pr_title = "chore(release): {version}"

            [monorepo]
            enabled = true
            release_mode = "release_set"

            [prerelease]
            enabled = true

            [prerelease.workspace]
            sync_root_extras = ["defaults"]
            "#,
        )
        .expect("config");
        let analysis = prerelease_release_set_analysis(false);

        let plan = build_release_pr_plan(&config, &analysis, "beta").expect("plan");

        assert!(plan.body.contains("## Prerelease"), "{}", plan.body);
        assert!(
            plan.body
                .contains("| phlo-iceberg | 0.3.1b2 | changed since latest tag |"),
            "{}",
            plan.body
        );
        assert!(
            plan.body.contains("uv pip install --prerelease explicit"),
            "{}",
            plan.body
        );
        assert!(
            plan.body.contains("\"phlo[defaults]==0.8.1b6\""),
            "{}",
            plan.body
        );
        assert!(
            plan.body.contains("\"phlo-iceberg==0.3.1b2\""),
            "{}",
            plan.body
        );
        assert!(!plan.body.contains("--prerelease allow"));
    }

    #[test]
    fn prerelease_release_set_pr_body_includes_all_configured_extras() {
        let config: Config = toml::from_str(
            r#"
            [release]
            pr_title = "chore(release): {version}"

            [monorepo]
            enabled = true
            release_mode = "release_set"

            [prerelease]
            enabled = true

            [prerelease.workspace]
            sync_root_extras = ["defaults", "core-services"]
            "#,
        )
        .expect("config");
        let analysis = prerelease_release_set_analysis(false);

        let plan = build_release_pr_plan(&config, &analysis, "beta").expect("plan");

        assert!(plan.body.contains("\"phlo[defaults]==0.8.1b6\""));
        assert!(plan.body.contains("\"phlo[core-services]==0.8.1b6\""));
    }

    #[test]
    fn finalize_release_set_pr_body_calls_out_prerelease_finalization() {
        let config: Config = toml::from_str(
            r#"
            [release]
            pr_title = "chore(release): {version}"

            [monorepo]
            enabled = true
            release_mode = "release_set"

            [prerelease]
            enabled = true
            "#,
        )
        .expect("config");
        let analysis = prerelease_release_set_analysis(true);

        let plan = build_release_pr_plan(&config, &analysis, "main").expect("plan");

        assert!(
            plan.body.contains("## Finalize Prerelease"),
            "{}",
            plan.body
        );
        assert!(
            plan.body
                .contains("| phlo-iceberg | 0.3.1 | finalize prerelease package |"),
            "{}",
            plan.body
        );
        assert!(!plan.body.contains("--prerelease explicit"));
    }

    #[test]
    fn wheel_distribution_normalization_collapses_separator_runs() {
        assert_eq!(
            normalize_wheel_distribution("Phlo---Iceberg.Core"),
            "phlo_iceberg_core"
        );
    }

    #[test]
    fn prerelease_workspace_hooks_skip_normal_stable_release_sets() {
        let config: Config = toml::from_str(
            r#"
            [project]
            ecosystem = "python"

            [monorepo]
            enabled = true
            release_mode = "release_set"

            [prerelease]
            enabled = true
            "#,
        )
        .expect("config");
        let mut analysis = prerelease_release_set_analysis(false);
        analysis.next_version.as_mut().expect("next version").suffix = None;
        for package in &mut analysis.package_plan.packages {
            package
                .next_version
                .as_mut()
                .expect("package next version")
                .suffix = None;
            package.selection_reason = "changed since latest tag".to_string();
        }

        assert!(!super::python_prerelease_workspace_applies(
            &config, &analysis
        ));
    }

    #[test]
    fn builds_bounded_monorepo_release_tag_plan() {
        let dir = tempdir().expect("tempdir");
        run(dir.path(), &["git", "init"]);
        run(dir.path(), &["git", "checkout", "-b", "main"]);
        run(dir.path(), &["git", "config", "user.name", "Relx Test"]);
        run(
            dir.path(),
            &["git", "config", "user.email", "relx@example.com"],
        );
        run(dir.path(), &["git", "add", "."]);
        run(
            dir.path(),
            &["git", "commit", "--allow-empty", "-m", "feat: initial"],
        );

        let repo = GitRepository::discover(dir.path()).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [release]
            tag_prefix = "v"

            [monorepo]
            enabled = true
            release_mode = "unified"
            packages = ["packages/core", "packages/cli"]
            "#,
        )
        .expect("config");
        let analysis = monorepo_analysis();

        let plan = build_release_tag_plan(&config, &repo, &analysis).expect("plan");
        assert_eq!(plan.tag_name, "v1.2.0");
    }

    #[test]
    fn builds_bounded_per_package_monorepo_release_tag_plan() {
        let dir = tempdir().expect("tempdir");
        run(dir.path(), &["git", "init"]);
        run(dir.path(), &["git", "checkout", "-b", "main"]);
        run(dir.path(), &["git", "config", "user.name", "Relx Test"]);
        run(
            dir.path(),
            &["git", "config", "user.email", "relx@example.com"],
        );
        run(dir.path(), &["git", "add", "."]);
        run(
            dir.path(),
            &["git", "commit", "--allow-empty", "-m", "feat: initial"],
        );

        let repo = GitRepository::discover(dir.path()).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [release]
            tag_prefix = "v"

            [monorepo]
            enabled = true
            release_mode = "per_package"
            packages = ["packages/core", "packages/cli"]
            "#,
        )
        .expect("config");
        let package = &monorepo_analysis().package_plan.packages[0];
        let analysis = single_package_analysis(&monorepo_analysis(), package);

        let plan = build_release_tag_plan(&config, &repo, &analysis).expect("plan");
        assert!(
            plan.tag_name.starts_with("v1pkgs-core-"),
            "{}",
            plan.tag_name
        );
        assert!(plan.tag_name.len() < 64, "{}", plan.tag_name);
    }

    #[test]
    fn builds_root_version_release_set_monorepo_release_tag_plan() {
        let dir = tempdir().expect("tempdir");
        run(dir.path(), &["git", "init"]);
        run(dir.path(), &["git", "checkout", "-b", "main"]);
        run(dir.path(), &["git", "config", "user.name", "Relx Test"]);
        run(
            dir.path(),
            &["git", "config", "user.email", "relx@example.com"],
        );
        run(dir.path(), &["git", "add", "."]);
        run(
            dir.path(),
            &["git", "commit", "--allow-empty", "-m", "feat: initial"],
        );

        let repo = GitRepository::discover(dir.path()).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [release]
            tag_prefix = "v"
            release_name = "{tag_name}"

            [monorepo]
            enabled = true
            release_mode = "release_set"
            packages = [".", "packages/delta"]
            "#,
        )
        .expect("config");
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
                        commits: Vec::new(),
                        changed_paths: vec!["pyproject.toml".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                    PackageReleaseAnalysis {
                        name: "phlo-delta".to_string(),
                        root: "packages/delta".to_string(),
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
                        commits: Vec::new(),
                        changed_paths: vec!["packages/delta/src/mod.py".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                ],
            },
        };

        let plan = build_release_tag_plan(&config, &repo, &analysis).expect("plan");
        assert_eq!(plan.tag_name, "v0.7.3");
        assert_eq!(plan.title, plan.tag_name);
        assert!(
            plan.release_notes
                .contains("## [phlo 0.7.3 + 1 packages] - "),
            "{}",
            plan.release_notes
        );
    }

    #[test]
    fn builds_bounded_package_only_release_set_monorepo_release_tag_plan() {
        let dir = tempdir().expect("tempdir");
        run(dir.path(), &["git", "init"]);
        run(dir.path(), &["git", "checkout", "-b", "main"]);
        run(dir.path(), &["git", "config", "user.name", "Relx Test"]);
        run(
            dir.path(),
            &["git", "config", "user.email", "relx@example.com"],
        );
        run(dir.path(), &["git", "add", "."]);
        run(
            dir.path(),
            &["git", "commit", "--allow-empty", "-m", "feat: initial"],
        );

        let repo = GitRepository::discover(dir.path()).expect("repo");
        let config: Config = toml::from_str(
            r#"
            [release]
            tag_prefix = "v"
            release_name = "{tag_name}"

            [monorepo]
            enabled = true
            release_mode = "release_set"
            packages = ["packages/delta", "packages/minio"]
            "#,
        )
        .expect("config");
        let analysis = ReleaseAnalysis {
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
                        name: "phlo-delta".to_string(),
                        root: "packages/delta".to_string(),
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
                        commits: Vec::new(),
                        changed_paths: vec!["packages/delta/src/mod.py".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                    PackageReleaseAnalysis {
                        name: "phlo-minio".to_string(),
                        root: "packages/minio".to_string(),
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
                        commits: Vec::new(),
                        changed_paths: vec!["packages/minio/src/mod.py".to_string()],
                        selected: true,
                        selection_reason: "test".to_string(),
                    },
                ],
            },
        };

        let plan = build_release_tag_plan(&config, &repo, &analysis).expect("plan");
        assert!(
            plan.tag_name.starts_with("v2pkgs-phlo-delta-phlo-minio-"),
            "{}",
            plan.tag_name
        );
        assert!(plan.tag_name.len() < 64, "{}", plan.tag_name);
        assert_eq!(plan.title, plan.tag_name);
        assert!(
            plan.release_notes.contains("## [2 packages] - "),
            "{}",
            plan.release_notes
        );
    }

    fn sample_analysis() -> ReleaseAnalysis {
        ReleaseAnalysis {
            current_version: Version {
                major: 1,
                minor: 1,
                patch: 0,
                suffix: None,
            },
            next_version: Some(Version {
                major: 1,
                minor: 2,
                patch: 0,
                suffix: None,
            }),
            bump: BumpLevel::Minor,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::from([("Added".to_string(), vec!["search".to_string()])]),
                contributors: Vec::new(),
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
                        suffix: None,
                    },
                    next_version: Some(Version {
                        major: 1,
                        minor: 2,
                        patch: 0,
                        suffix: None,
                    }),
                    bump: BumpLevel::Minor,
                    changelog: PendingChangelog {
                        sections: BTreeMap::from([(
                            "Added".to_string(),
                            vec!["search".to_string()],
                        )]),
                        contributors: Vec::new(),
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

    fn prerelease_release_set_analysis(finalize: bool) -> ReleaseAnalysis {
        let root_current_suffix = finalize.then_some(Suffix::Pre(PreRelease::Beta(5)));
        let package_current_suffix = finalize.then_some(Suffix::Pre(PreRelease::Beta(1)));
        ReleaseAnalysis {
            current_version: Version {
                major: 0,
                minor: 8,
                patch: 1,
                suffix: root_current_suffix.clone(),
            },
            next_version: Some(Version {
                major: 0,
                minor: 8,
                patch: 1,
                suffix: if finalize {
                    None
                } else {
                    Some(Suffix::Pre(PreRelease::Beta(6)))
                },
            }),
            bump: if finalize {
                BumpLevel::None
            } else {
                BumpLevel::Patch
            },
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
                            minor: 8,
                            patch: 1,
                            suffix: root_current_suffix,
                        },
                        next_version: Some(Version {
                            major: 0,
                            minor: 8,
                            patch: 1,
                            suffix: if finalize {
                                None
                            } else {
                                Some(Suffix::Pre(PreRelease::Beta(6)))
                            },
                        }),
                        bump: BumpLevel::Patch,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::new(),
                        changed_paths: Vec::new(),
                        selected: true,
                        selection_reason: if finalize {
                            "finalize prerelease package".to_string()
                        } else {
                            "root prerelease".to_string()
                        },
                    },
                    PackageReleaseAnalysis {
                        name: "phlo-iceberg".to_string(),
                        root: "packages/iceberg".to_string(),
                        current_version: Version {
                            major: 0,
                            minor: 3,
                            patch: 1,
                            suffix: package_current_suffix,
                        },
                        next_version: Some(Version {
                            major: 0,
                            minor: 3,
                            patch: 1,
                            suffix: if finalize {
                                None
                            } else {
                                Some(Suffix::Pre(PreRelease::Beta(2)))
                            },
                        }),
                        bump: BumpLevel::Patch,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
                        },
                        version_files: Vec::new(),
                        commits: Vec::new(),
                        changed_paths: vec!["packages/iceberg/src/mod.py".to_string()],
                        selected: true,
                        selection_reason: if finalize {
                            "finalize prerelease package".to_string()
                        } else {
                            "changed since latest tag".to_string()
                        },
                    },
                ],
            },
        }
    }

    fn monorepo_analysis() -> ReleaseAnalysis {
        ReleaseAnalysis {
            current_version: Version {
                major: 1,
                minor: 1,
                patch: 0,
                suffix: None,
            },
            next_version: None,
            bump: BumpLevel::Minor,
            commits: Vec::new(),
            changelog: PendingChangelog {
                sections: BTreeMap::from([(
                    "Added".to_string(),
                    vec!["core: search".to_string(), "cli: status".to_string()],
                )]),
                contributors: Vec::new(),
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
                            suffix: None,
                        },
                        next_version: Some(Version {
                            major: 1,
                            minor: 2,
                            patch: 0,
                            suffix: None,
                        }),
                        bump: BumpLevel::Minor,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
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
                            suffix: None,
                        },
                        next_version: Some(Version {
                            major: 0,
                            minor: 5,
                            patch: 1,
                            suffix: None,
                        }),
                        bump: BumpLevel::Patch,
                        changelog: PendingChangelog {
                            sections: BTreeMap::new(),
                            contributors: Vec::new(),
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

    #[test]
    fn output_globs_do_not_cross_path_segments_or_overlap() {
        assert!(super::glob_matches(
            "registry/*.json",
            "registry/support.json"
        ));
        assert!(!super::glob_matches(
            "registry/*.json",
            "registry/nested/support.json"
        ));
        assert!(super::glob_matches(
            "packages/**/service*.yaml",
            "packages/api/src/service.yaml"
        ));
        assert!(!super::glob_matches("aa*aa", "aaa"));
    }
}
