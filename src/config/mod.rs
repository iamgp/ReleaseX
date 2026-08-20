use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub project: ProjectConfig,
    #[serde(default)]
    pub release: ReleaseConfig,
    #[serde(default)]
    pub versioning: VersioningConfig,
    #[serde(default)]
    pub monorepo: MonorepoConfig,
    #[serde(default)]
    pub version_files: Vec<VersionFileConfig>,
    #[serde(default)]
    pub changelog: ChangelogConfig,
    #[serde(default)]
    pub publish: PublishConfig,
    #[serde(default)]
    pub github: GitHubConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub prerelease: PrereleaseConfig,
    #[serde(default)]
    pub ci: CiConfig,
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let config: Self =
            toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.release.branch.trim().is_empty() {
            bail!("release.branch must not be empty");
        }

        if self.release.tag_prefix.trim().is_empty() {
            bail!("release.tag_prefix must not be empty");
        }

        if !self.monorepo.enabled && self.version_files.is_empty() {
            bail!("at least one [[version_files]] entry is required");
        }

        if !matches!(
            self.monorepo.release_mode.as_str(),
            "unified" | "release_set" | "per_package"
        ) {
            bail!("monorepo.release_mode must be one of: unified, release_set, per_package");
        }

        for package in &self.monorepo.packages {
            if package.trim().is_empty() {
                bail!("monorepo.packages entries must not be empty");
            }
        }

        for version_file in &self.version_files {
            validate_version_file(version_file)?;
        }

        validate_workspace_config(&self.workspace)?;
        validate_transformers(&self.release.transformers)?;

        for channel in &self.channels {
            if channel.branch.trim().is_empty() {
                bail!("channels.branch must not be empty");
            }
            if let Some(prerelease) = &channel.prerelease
                && !matches!(prerelease.as_str(), "a" | "b" | "rc")
            {
                bail!("channels.prerelease must be one of: a, b, rc");
            }
        }

        let provider = self.publish.provider.trim();
        if provider.is_empty() {
            bail!("publish.provider must not be empty");
        }

        if !matches!(provider, "uv" | "twine" | "cargo" | "goreleaser") {
            bail!("publish.provider must be one of: uv, twine, cargo, goreleaser");
        }

        if self.publish.repository.trim().is_empty() {
            bail!("publish.repository must not be empty");
        }

        if self.publish.dist_dir.trim().is_empty() {
            bail!("publish.dist_dir must not be empty");
        }

        if let Some(url) = &self.publish.repository_url
            && url.trim().is_empty()
        {
            bail!("publish.repository_url must not be empty when provided");
        }

        for (field, value) in [
            ("publish.username_env", self.publish.username_env.as_deref()),
            ("publish.password_env", self.publish.password_env.as_deref()),
            ("publish.token_env", self.publish.token_env.as_deref()),
        ] {
            if matches!(value, Some(raw) if raw.trim().is_empty()) {
                bail!("{field} must not be empty when provided");
            }
        }

        Ok(())
    }

    pub fn section_for_commit_type(&self, commit_type: &str) -> Option<String> {
        match self.changelog.sections.get(commit_type) {
            Some(toml::Value::Boolean(false)) => None,
            Some(toml::Value::String(value)) => Some(value.clone()),
            Some(_) => None,
            None => Some(default_section_for_commit_type(commit_type).to_string()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub ecosystem: Option<Ecosystem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Python,
    Rust,
    Go,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub branch: String,
    #[serde(default)]
    pub publish: bool,
    #[serde(default)]
    pub prerelease: Option<String>,
    #[serde(default)]
    pub version_range: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonorepoConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default = "default_monorepo_release_mode")]
    pub release_mode: String,
}

impl MonorepoConfig {
    pub fn is_multi_package(&self) -> bool {
        self.enabled
    }
}

impl Default for MonorepoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            packages: Vec::new(),
            release_mode: default_monorepo_release_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseConfig {
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default = "default_tag_prefix")]
    pub tag_prefix: String,
    #[serde(default = "default_changelog_file")]
    pub changelog_file: String,
    #[serde(default = "default_pr_title")]
    pub pr_title: String,
    #[serde(default = "default_release_name")]
    pub release_name: String,
    #[serde(default)]
    pub transformers: Vec<ReleaseTransformerConfig>,
}

impl Default for ReleaseConfig {
    fn default() -> Self {
        Self {
            branch: default_branch(),
            tag_prefix: default_tag_prefix(),
            changelog_file: default_changelog_file(),
            pr_title: default_pr_title(),
            release_name: default_release_name(),
            transformers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersioningConfig {
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default = "default_initial_version")]
    pub initial_version: String,
}

impl Default for VersioningConfig {
    fn default() -> Self {
        Self {
            strategy: default_strategy(),
            initial_version: default_initial_version(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionFileConfig {
    pub path: String,
    pub key: Option<String>,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogConfig {
    #[serde(default)]
    pub sections: BTreeMap<String, toml::Value>,
    #[serde(default = "default_contributors_enabled")]
    pub contributors: bool,
    #[serde(default = "default_first_contribution_emoji")]
    pub first_contribution_emoji: String,
    #[serde(default = "default_exclude_bots")]
    pub exclude_bots: bool,
    #[serde(default)]
    pub bot_patterns: Vec<String>,
}

impl Default for ChangelogConfig {
    fn default() -> Self {
        Self {
            sections: BTreeMap::new(),
            contributors: default_contributors_enabled(),
            first_contribution_emoji: default_first_contribution_emoji(),
            exclude_bots: default_exclude_bots(),
            bot_patterns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_publish_provider")]
    pub provider: String,
    #[serde(default = "default_publish_repository")]
    pub repository: String,
    #[serde(default)]
    pub repository_url: Option<String>,
    #[serde(default = "default_publish_dist_dir")]
    pub dist_dir: String,
    #[serde(default)]
    pub trusted_publishing: bool,
    #[serde(default)]
    pub oidc: bool,
    #[serde(default)]
    pub skip_published: bool,
    #[serde(default)]
    pub username_env: Option<String>,
    #[serde(default)]
    pub password_env: Option<String>,
    #[serde(default)]
    pub token_env: Option<String>,
}

impl Default for PublishConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_publish_provider(),
            repository: default_publish_repository(),
            repository_url: None,
            dist_dir: default_publish_dist_dir(),
            trusted_publishing: false,
            oidc: false,
            skip_published: false,
            username_env: None,
            password_env: None,
            token_env: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubConfig {
    pub owner: Option<String>,
    pub repo: Option<String>,
    #[serde(default = "default_github_api_base")]
    pub api_base: String,
    #[serde(default = "default_github_token_env")]
    pub token_env: String,
    #[serde(default = "default_release_branch_prefix")]
    pub release_branch_prefix: String,
    #[serde(default = "default_pending_label")]
    pub pending_label: String,
    #[serde(default = "default_tagged_label")]
    pub tagged_label: String,
    #[serde(default = "default_commit_author")]
    pub commit_author: String,
    #[serde(default = "default_commit_email")]
    pub commit_email: String,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            owner: None,
            repo: None,
            api_base: default_github_api_base(),
            token_env: default_github_token_env(),
            release_branch_prefix: default_release_branch_prefix(),
            pending_label: default_pending_label(),
            tagged_label: default_tagged_label(),
            commit_author: default_commit_author(),
            commit_email: default_commit_email(),
        }
    }
}

fn validate_version_file(version_file: &VersionFileConfig) -> Result<()> {
    if version_file.path.trim().is_empty() {
        bail!("version file path must not be empty");
    }

    if version_file.key.is_none() && version_file.pattern.is_none() {
        bail!(
            "version file {} must define either `key` or `pattern`",
            version_file.path
        );
    }

    Ok(())
}

fn default_branch() -> String {
    "main".to_string()
}

fn default_tag_prefix() -> String {
    "v".to_string()
}

fn default_changelog_file() -> String {
    "CHANGELOG.md".to_string()
}

fn default_pr_title() -> String {
    "chore(release): {version}".to_string()
}

fn default_release_name() -> String {
    "{tag_name}".to_string()
}

fn default_strategy() -> String {
    "conventional_commits".to_string()
}

fn default_initial_version() -> String {
    "0.1.0".to_string()
}

fn default_publish_provider() -> String {
    "uv".to_string()
}

fn default_publish_repository() -> String {
    "pypi".to_string()
}

fn default_publish_dist_dir() -> String {
    "dist".to_string()
}

fn default_monorepo_release_mode() -> String {
    "unified".to_string()
}

fn default_github_api_base() -> String {
    "https://api.github.com".to_string()
}

fn default_github_token_env() -> String {
    "GITHUB_TOKEN".to_string()
}

fn default_release_branch_prefix() -> String {
    "relx/release".to_string()
}

fn default_pending_label() -> String {
    "autorelease: pending".to_string()
}

fn default_tagged_label() -> String {
    "autorelease: tagged".to_string()
}

fn default_commit_author() -> String {
    "github-actions[bot]".to_string()
}

fn default_commit_email() -> String {
    "41898282+github-actions[bot]@users.noreply.github.com".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub cascade_bumps: bool,
    #[serde(default)]
    pub dependencies: WorkspaceDependenciesConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceDependenciesConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<WorkspaceDependencyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceDependencyRule {
    pub dependency: String,
    #[serde(default)]
    pub dependents: Vec<String>,
    #[serde(default = "default_dependency_when")]
    pub when: String,
    pub range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseTransformerConfig {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default = "default_transformer_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
}

fn default_transformer_timeout_seconds() -> u64 {
    60
}

fn default_dependency_when() -> String {
    "dependency_selected".to_string()
}

fn validate_workspace_config(workspace: &WorkspaceConfig) -> Result<()> {
    for rule in &workspace.dependencies.rules {
        if rule.dependency.trim().is_empty() || rule.range.trim().is_empty() {
            bail!("workspace.dependencies rules require dependency and range");
        }
        if !matches!(
            rule.when.as_str(),
            "dependency_selected" | "always" | "dependent_selected"
        ) {
            bail!(
                "workspace.dependencies.rules.when must be dependency_selected, dependent_selected, or always"
            );
        }
        for variable in template_variables(&rule.range) {
            if !matches!(
                variable.as_str(),
                "version"
                    | "current_version"
                    | "major"
                    | "minor"
                    | "patch"
                    | "next_major"
                    | "next_minor"
            ) {
                bail!("unknown workspace dependency range template variable {{{variable}}}");
            }
        }
    }
    Ok(())
}

fn validate_transformers(transformers: &[ReleaseTransformerConfig]) -> Result<()> {
    let mut names = std::collections::BTreeSet::new();
    for transformer in transformers {
        if transformer.name.trim().is_empty() || transformer.command.is_empty() {
            bail!("release transformers require a unique name and non-empty command");
        }
        if transformer.timeout_seconds == 0 {
            bail!("release transformer timeout_seconds must be greater than zero");
        }
        if !names.insert(&transformer.name) {
            bail!(
                "release transformer names must be unique: {}",
                transformer.name
            );
        }
        for path in transformer.inputs.iter().chain(&transformer.outputs) {
            let path = Path::new(path);
            if path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                bail!(
                    "release transformer paths must be workspace-relative: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn template_variables(template: &str) -> Vec<String> {
    template
        .match_indices('{')
        .filter_map(|(start, _)| {
            template[start + 1..]
                .find('}')
                .map(|end| template[start + 1..start + 1 + end].to_string())
        })
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrereleaseConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub workspace: PrereleaseWorkspaceConfig,
    #[serde(default)]
    pub verify: PrereleaseVerifyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrereleaseWorkspaceConfig {
    #[serde(default = "default_prerelease_include_root")]
    pub include_root: bool,
    #[serde(default = "default_prerelease_sync_root_dependencies")]
    pub sync_root_dependencies: bool,
    #[serde(default)]
    pub sync_root_extras: Vec<String>,
}

impl Default for PrereleaseWorkspaceConfig {
    fn default() -> Self {
        Self {
            include_root: default_prerelease_include_root(),
            sync_root_dependencies: default_prerelease_sync_root_dependencies(),
            sync_root_extras: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrereleaseVerifyConfig {
    #[serde(default = "default_prerelease_verify_lock")]
    pub lock: bool,
    #[serde(default = "default_prerelease_verify_build")]
    pub build: bool,
    #[serde(default = "default_prerelease_verify_inspect_wheel_metadata")]
    pub inspect_wheel_metadata: bool,
    #[serde(default = "default_prerelease_verify_emit_install_command")]
    pub emit_install_command: bool,
}

impl Default for PrereleaseVerifyConfig {
    fn default() -> Self {
        Self {
            lock: default_prerelease_verify_lock(),
            build: default_prerelease_verify_build(),
            inspect_wheel_metadata: default_prerelease_verify_inspect_wheel_metadata(),
            emit_install_command: default_prerelease_verify_emit_install_command(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiConfig {
    #[serde(default = "default_ci_provider")]
    pub provider: String,
    #[serde(default = "default_ci_workflow_path")]
    pub workflow_path: String,
}

impl Default for CiConfig {
    fn default() -> Self {
        Self {
            provider: default_ci_provider(),
            workflow_path: default_ci_workflow_path(),
        }
    }
}

fn default_ci_provider() -> String {
    "github".to_string()
}

fn default_ci_workflow_path() -> String {
    ".github/workflows/release.yml".to_string()
}

fn default_prerelease_include_root() -> bool {
    true
}

fn default_prerelease_sync_root_dependencies() -> bool {
    true
}

fn default_prerelease_verify_lock() -> bool {
    true
}

fn default_prerelease_verify_build() -> bool {
    true
}

fn default_prerelease_verify_inspect_wheel_metadata() -> bool {
    true
}

fn default_prerelease_verify_emit_install_command() -> bool {
    true
}

fn default_contributors_enabled() -> bool {
    true
}

fn default_first_contribution_emoji() -> String {
    "🎉".to_string()
}

fn default_exclude_bots() -> bool {
    true
}

fn default_section_for_commit_type(commit_type: &str) -> &'static str {
    match commit_type {
        "feat" => "Added",
        "fix" => "Fixed",
        "refactor" | "perf" => "Changed",
        _ => "Changed",
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn config_requires_version_files() {
        let config = Config {
            version_files: Vec::new(),
            ..toml::from_str("").expect("default config")
        };

        let error = config.validate().expect_err("validation should fail");
        assert!(error.to_string().contains("version_files"));
    }

    #[test]
    fn config_rejects_unknown_publish_provider() {
        let config = Config {
            version_files: vec![super::VersionFileConfig {
                path: "pyproject.toml".to_string(),
                key: Some("project.version".to_string()),
                pattern: None,
            }],
            publish: super::PublishConfig {
                provider: "poetry".to_string(),
                ..Default::default()
            },
            ..toml::from_str("").expect("default config")
        };

        let error = config.validate().expect_err("validation should fail");
        assert!(error.to_string().contains("publish.provider"));
    }

    #[test]
    fn monorepo_config_allows_empty_top_level_version_files() {
        let config = Config {
            monorepo: super::MonorepoConfig {
                enabled: true,
                packages: vec!["packages/core".to_string()],
                release_mode: "per_package".to_string(),
            },
            version_files: Vec::new(),
            ..toml::from_str("").expect("default config")
        };

        config.validate().expect("validation should pass");
    }

    #[test]
    fn monorepo_config_accepts_release_set_mode() {
        let config = Config {
            monorepo: super::MonorepoConfig {
                enabled: true,
                packages: vec!["packages/core".to_string()],
                release_mode: "release_set".to_string(),
            },
            version_files: Vec::new(),
            ..toml::from_str("").expect("default config")
        };

        config.validate().expect("validation should pass");
    }

    #[test]
    fn github_config_defaults_release_commit_identity() {
        let config: Config = toml::from_str("").expect("default config");
        assert_eq!(config.github.commit_author, "github-actions[bot]");
        assert_eq!(
            config.github.commit_email,
            "41898282+github-actions[bot]@users.noreply.github.com"
        );
    }

    #[test]
    fn github_config_allows_custom_release_commit_identity() {
        let config: Config = toml::from_str(
            r#"
            [github]
            commit_author = "release-bot"
            commit_email = "release-bot@example.com"
            "#,
        )
        .expect("config");
        assert_eq!(config.github.commit_author, "release-bot");
        assert_eq!(config.github.commit_email, "release-bot@example.com");
    }

    #[test]
    fn prerelease_config_defaults_are_non_invasive() {
        let config: Config = toml::from_str("").expect("default config");

        assert!(!config.prerelease.enabled);
        assert!(config.prerelease.workspace.include_root);
        assert!(config.prerelease.workspace.sync_root_dependencies);
        assert!(config.prerelease.workspace.sync_root_extras.is_empty());
        assert!(config.prerelease.verify.lock);
        assert!(config.prerelease.verify.build);
        assert!(config.prerelease.verify.inspect_wheel_metadata);
        assert!(config.prerelease.verify.emit_install_command);
    }

    #[test]
    fn prerelease_config_parses_phlo_style_workspace_settings() {
        let config: Config = toml::from_str(
            r#"
            [prerelease]
            enabled = true

            [prerelease.workspace]
            sync_root_extras = ["defaults", "core-services"]

            [prerelease.verify]
            emit_install_command = true
            "#,
        )
        .expect("config");

        assert!(config.prerelease.enabled);
        assert_eq!(
            config.prerelease.workspace.sync_root_extras,
            vec!["defaults".to_string(), "core-services".to_string()]
        );
        assert!(config.prerelease.verify.emit_install_command);
    }
}
