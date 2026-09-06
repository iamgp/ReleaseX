use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail};
use openssl::sha::sha256;
use serde::{Deserialize, Serialize};

use crate::{
    analysis::{PackageReleaseAnalysis, ReleaseAnalysis},
    config::Config,
    git::GitRepository,
    version::Version,
};

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub ecosystem: String,
    pub release_mode: String,
    pub preparation_base: PreparationBase,
    pub source_digest: String,
    pub covered_paths: Vec<String>,
    pub shared_bookkeeping: Vec<String>,
    pub packages: Vec<ManifestPackage>,
    #[serde(default)]
    pub required_dependency_changes: Vec<RequiredDependencyChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparationBase {
    pub ref_name: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPackage {
    pub name: String,
    pub path: String,
    pub selected: bool,
    pub selection_reason: String,
    pub current_version: String,
    pub next_version: Option<String>,
    pub bump: String,
    pub baseline: ManifestBaseline,
    pub release_tag: String,
    pub changes: Vec<ManifestChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestBaseline {
    pub kind: String,
    pub reference: Option<String>,
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestChange {
    pub id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredDependencyChange {
    pub package: String,
    pub path: String,
    pub dependency: String,
    pub declared_range: String,
    pub required_version: String,
    pub reason: String,
}

impl ReleaseManifest {
    pub fn from_analysis(
        repo: &GitRepository,
        config: &Config,
        analysis: &ReleaseAnalysis,
        base_ref: &str,
        repo_root: &Path,
    ) -> Result<Self> {
        let selected = analysis.package_plan.selected_packages();
        if selected.is_empty() {
            bail!("no releasable package set is pending from the current commit set");
        }

        let mut shared_bookkeeping = vec![config.release.plan_file.clone()];
        if !shared_bookkeeping.contains(&config.release.changelog_file)
            && analysis.package_plan.release_mode != "per_package"
        {
            shared_bookkeeping.push(config.release.changelog_file.clone());
        }

        let mut covered = BTreeSet::new();
        for path in &shared_bookkeeping {
            if repo_root.join(path).exists() {
                covered.insert(path.clone());
            }
        }
        for package in &selected {
            collect_package_files(repo_root, &package.root, &mut covered)?;
            if analysis.package_plan.release_mode == "per_package" {
                let changelog = if package.root == "." {
                    config.release.changelog_file.clone()
                } else {
                    format!("{}/{}", package.root, config.release.changelog_file)
                };
                if repo_root.join(&changelog).exists() {
                    covered.insert(changelog);
                }
            }
        }

        let covered_paths = covered.into_iter().collect::<Vec<_>>();
        let hashed_paths = covered_paths
            .iter()
            .filter(|path| *path != &config.release.plan_file)
            .cloned()
            .collect::<Vec<_>>();
        let source_digest = digest_paths(repo_root, &hashed_paths)?;
        let required_dependency_changes = selected
            .iter()
            .flat_map(|package| package.required_dependency_changes.iter())
            .map(|change| RequiredDependencyChange {
                package: change.package.clone(),
                path: change.path.clone(),
                dependency: change.dependency.clone(),
                declared_range: change.declared_range.clone(),
                required_version: change.required_version.clone(),
                reason: change.reason.clone(),
            })
            .collect();

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            ecosystem: ecosystem_name(config),
            release_mode: analysis.package_plan.release_mode.clone(),
            preparation_base: PreparationBase {
                ref_name: base_ref.to_string(),
                commit: repo.rev_parse("HEAD")?,
            },
            source_digest,
            covered_paths,
            shared_bookkeeping,
            packages: analysis
                .package_plan
                .packages
                .iter()
                .map(manifest_package)
                .collect(),
            required_dependency_changes,
        })
    }

    pub fn selected_packages(&self) -> Vec<&ManifestPackage> {
        self.packages
            .iter()
            .filter(|package| package.selected)
            .collect()
    }

    pub fn write(&self, repo_root: &Path, path: &str) -> Result<()> {
        let destination = repo_root.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let rendered = serde_json::to_string_pretty(self)?;
        fs::write(&destination, format!("{rendered}\n"))
            .with_context(|| format!("failed to write {}", destination.display()))?;
        Ok(())
    }

    pub fn load(repo_root: &Path, path: &str) -> Result<Self> {
        let destination = repo_root.join(path);
        let raw = fs::read_to_string(&destination).with_context(|| {
            format!(
                "release manifest {} is missing; run `relx release plan` or `relx release prepare` before tagging",
                destination.display()
            )
        })?;
        let manifest: Self = serde_json::from_str(&raw).with_context(|| {
            format!(
                "release manifest {} is malformed or was tampered with",
                destination.display()
            )
        })?;
        if manifest.schema_version != SCHEMA_VERSION {
            bail!(
                "release manifest schema {} is incompatible with relx (expected {SCHEMA_VERSION})",
                manifest.schema_version
            );
        }
        Ok(manifest)
    }

    pub fn validate_against_tree(&self, repo_root: &Path, config: &Config) -> Result<()> {
        if self.release_mode != config.monorepo.release_mode
            && config.monorepo.enabled
            && self.release_mode != "single"
        {
            bail!(
                "release manifest release_mode `{}` does not match config `{}`",
                self.release_mode,
                config.monorepo.release_mode
            );
        }

        let hashed_paths = self
            .covered_paths
            .iter()
            .filter(|path| *path != &config.release.plan_file)
            .cloned()
            .collect::<Vec<_>>();
        let digest = digest_paths(repo_root, &hashed_paths)?;
        if digest != self.source_digest {
            bail!(
                "merged source does not match the reviewed release manifest (source digest {digest} != {}). The plan is stale or the prepared tree was altered. Re-run `relx release prepare` from the current base.",
                self.source_digest
            );
        }

        for package in self.selected_packages() {
            let Some(next_version) = package.next_version.as_deref() else {
                bail!(
                    "release manifest selected package `{}` has no next version",
                    package.name
                );
            };
            let expected: Version = next_version.parse()?;
            let current = crate::analysis::read_current_version(
                repo_root,
                &version_files_for_package(repo_root, package)?,
            )?;
            match current {
                Some(value) => {
                    let parsed: Version = value.parse()?;
                    if parsed != expected {
                        bail!(
                            "package `{}` source version {parsed} does not match reviewed plan {expected}",
                            package.name
                        );
                    }
                }
                None => bail!(
                    "package `{}` is missing from the merged tree despite being in the reviewed plan",
                    package.name
                ),
            }
        }
        Ok(())
    }
}

fn manifest_package(package: &PackageReleaseAnalysis) -> ManifestPackage {
    ManifestPackage {
        name: package.name.clone(),
        path: package.root.clone(),
        selected: package.selected,
        selection_reason: package.selection_reason.clone(),
        current_version: package.current_version.to_string(),
        next_version: package.next_version.as_ref().map(ToString::to_string),
        bump: package.bump.as_str().to_string(),
        baseline: ManifestBaseline {
            kind: package.baseline.kind.clone(),
            reference: package.baseline.reference.clone(),
            commit: package.baseline.commit.clone(),
        },
        release_tag: package.release_tag.clone(),
        changes: package
            .commits
            .iter()
            .map(|commit| ManifestChange {
                id: commit.id.clone(),
                message: commit.message.clone(),
            })
            .collect(),
    }
}

fn version_files_for_package(
    repo_root: &Path,
    package: &ManifestPackage,
) -> Result<Vec<crate::config::VersionFileConfig>> {
    let package_path = if package.path == "." {
        repo_root.to_path_buf()
    } else {
        repo_root.join(&package.path)
    };
    crate::analysis::detect_package_version_files_for_manifest(repo_root, &package_path)
}

fn collect_package_files(
    repo_root: &Path,
    package_root: &str,
    files: &mut BTreeSet<String>,
) -> Result<()> {
    let root = if package_root == "." {
        repo_root.to_path_buf()
    } else {
        repo_root.join(package_root)
    };
    collect_files(repo_root, &root, files)
}

fn collect_files(repo_root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" || name == "target" || name == "node_modules" || name == ".venv" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_files(repo_root, &path, files)?;
        } else {
            files.insert(
                path.strip_prefix(repo_root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

pub fn digest_paths(repo_root: &Path, paths: &[String]) -> Result<String> {
    let mut payload = Vec::new();
    for path in paths {
        payload.extend_from_slice(path.as_bytes());
        payload.push(0);
        let file = repo_root.join(path);
        if file.is_file() {
            payload.extend(fs::read(&file)?);
        }
        payload.push(0);
    }
    Ok(hex_digest(&sha256(&payload)))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn ecosystem_name(config: &Config) -> String {
    match config.project.ecosystem {
        Some(crate::config::Ecosystem::Python) => "python",
        Some(crate::config::Ecosystem::Rust) => "rust",
        Some(crate::config::Ecosystem::Go) => "go",
        Some(crate::config::Ecosystem::TypeScript) => "typescript",
        None => "unknown",
    }
    .to_string()
}
