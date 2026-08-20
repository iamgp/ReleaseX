use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};
use toml_edit::{Array, DocumentMut, Item, Value};

use crate::{config::WorkspaceDependenciesConfig, workspace_plan::ReleaseWorkspacePlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyOperation {
    pub path: String,
    pub before: String,
    pub after: String,
}

pub fn sync_python_workspace_dependencies(
    repo_root: &Path,
    plan: &ReleaseWorkspacePlan,
    dependencies: &WorkspaceDependenciesConfig,
) -> Result<Vec<DependencyOperation>> {
    if !dependencies.enabled {
        return Ok(Vec::new());
    }

    let mut operations = Vec::new();
    for rule in &dependencies.rules {
        let dependency = plan
            .packages
            .iter()
            .find(|package| package.name == rule.dependency)
            .with_context(|| {
                format!(
                    "workspace dependency rule references unknown package {}",
                    rule.dependency
                )
            })?;
        if rule.when == "dependency_selected" && !dependency.selected {
            continue;
        }
        let dependents = plan
            .packages
            .iter()
            .filter(|package| {
                !rule.dependents.is_empty()
                    && rule
                        .dependents
                        .iter()
                        .any(|pattern| path_matches(pattern, &package.path))
                    && (rule.when != "dependent_selected" || package.selected)
            })
            .collect::<Vec<_>>();
        if dependents.is_empty() {
            bail!(
                "workspace dependency rule for {} matched no dependents",
                rule.dependency
            );
        }
        let next_version = match dependency.next_version.as_ref() {
            Some(version) => version,
            None if rule.when == "always" => &dependency.current_version,
            None => bail!(
                "workspace dependency {} has no next version",
                dependency.name
            ),
        };
        let range = render_range(&rule.range, &dependency.current_version, next_version)?;
        for dependent in dependents {
            let relative = if dependent.path == "." {
                "pyproject.toml".to_string()
            } else {
                format!("{}/pyproject.toml", dependent.path)
            };
            let path = repo_root.join(&relative);
            if !path.exists() {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let mut parsed = raw
                .parse::<DocumentMut>()
                .with_context(|| format!("failed to parse {}", path.display()))?;
            let mut changed = false;
            let Some(project) = parsed.get_mut("project").and_then(Item::as_table_like_mut) else {
                continue;
            };
            if let Some(deps) = project.get_mut("dependencies").and_then(Item::as_array_mut) {
                changed |= rewrite_dependency_values(
                    deps,
                    &relative,
                    &rule.dependency,
                    &range,
                    &mut operations,
                )?;
            }
            if let Some(extras) = project
                .get_mut("optional-dependencies")
                .and_then(Item::as_table_like_mut)
            {
                for (_, deps) in extras
                    .iter_mut()
                    .filter_map(|(name, item)| item.as_array_mut().map(|deps| (name, deps)))
                {
                    changed |= rewrite_dependency_values(
                        deps,
                        &relative,
                        &rule.dependency,
                        &range,
                        &mut operations,
                    )?;
                }
            }
            if changed {
                fs::write(&path, parsed.to_string())
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
        }
    }
    Ok(operations)
}

fn rewrite_dependency_values(
    deps: &mut Array,
    path: &str,
    dependency: &str,
    range: &str,
    operations: &mut Vec<DependencyOperation>,
) -> Result<bool> {
    let mut changed = false;
    for value in deps.iter_mut() {
        let Some(before) = value.as_str() else {
            continue;
        };
        let Some(after) = rewrite_requirement(before, dependency, range)? else {
            continue;
        };
        if after != before {
            operations.push(DependencyOperation {
                path: path.to_string(),
                before: before.to_string(),
                after: after.clone(),
            });
            replace_string_value(value, after);
            changed = true;
        }
    }
    Ok(changed)
}

fn replace_string_value(value: &mut Value, replacement: String) {
    let decor = value.decor().clone();
    *value = Value::from(replacement);
    *value.decor_mut() = decor;
}

fn path_matches(pattern: &str, value: &str) -> bool {
    if pattern == value {
        return true;
    }
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 2 {
        return value.starts_with(parts[0]) && value.ends_with(parts[1]);
    }
    false
}

fn render_range(template: &str, current: &str, version: &str) -> Result<String> {
    let parsed: crate::version::Version = version.parse()?;
    Ok(template
        .replace("{version}", version)
        .replace("{current_version}", current)
        .replace("{major}", &parsed.major.to_string())
        .replace("{minor}", &parsed.minor.to_string())
        .replace("{patch}", &parsed.patch.to_string())
        .replace("{next_major}", &(parsed.major + 1).to_string())
        .replace(
            "{next_minor}",
            &format!("{}.{}", parsed.major, parsed.minor + 1),
        ))
}

fn rewrite_requirement(
    requirement: &str,
    dependency_name: &str,
    range: &str,
) -> Result<Option<String>> {
    let marker_start = requirement.find(';');
    let (base, marker) = match marker_start {
        Some(index) => (&requirement[..index], &requirement[index..]),
        None => (requirement, ""),
    };
    let name_end = base
        .char_indices()
        .find_map(|(index, ch)| {
            matches!(ch, '[' | ' ' | '\t' | '<' | '>' | '=' | '!' | '~' | '@').then_some(index)
        })
        .unwrap_or(base.len());
    let name = &base[..name_end];
    if normalize_distribution_name(name) != normalize_distribution_name(dependency_name) {
        return Ok(None);
    }
    if base.contains('@') {
        bail!("direct reference for workspace dependency {dependency_name} is not allowed");
    }
    let extras_end = base.find(['<', '>', '=', '!', '~']).unwrap_or(base.len());
    Ok(Some(format!(
        "{}{}{}",
        base[..extras_end].trim_end(),
        range,
        marker
    )))
}

fn normalize_distribution_name(value: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    for ch in value.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !previous_was_separator {
                normalized.push('-');
                previous_was_separator = true;
            }
        } else {
            normalized.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
        }
    }
    normalized
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrereleasePackage {
    pub name: String,
    pub version: String,
    pub root: String,
    pub reason: String,
}

pub fn sync_root_python_workspace_dependencies(
    repo_root: &Path,
    selected_versions: &BTreeMap<String, String>,
    sync_root_dependencies: bool,
    sync_root_extras: &[String],
) -> Result<bool> {
    if selected_versions.is_empty() {
        return Ok(false);
    }

    let pyproject_path = repo_root.join("pyproject.toml");
    let raw = fs::read_to_string(&pyproject_path)
        .with_context(|| format!("failed to read {}", pyproject_path.display()))?;
    let mut parsed = raw
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse {}", pyproject_path.display()))?;

    let mut changed = false;
    if sync_root_dependencies
        && let Some(deps) = parsed
            .get_mut("project")
            .and_then(Item::as_table_like_mut)
            .and_then(|project| project.get_mut("dependencies"))
            .and_then(Item::as_array_mut)
    {
        changed |= sync_dependency_array(deps, selected_versions);
    }

    if !sync_root_extras.is_empty()
        && let Some(optional_deps) = parsed
            .get_mut("project")
            .and_then(Item::as_table_like_mut)
            .and_then(|project| project.get_mut("optional-dependencies"))
            .and_then(Item::as_table_like_mut)
    {
        for extra in sync_root_extras {
            if let Some(deps) = optional_deps.get_mut(extra).and_then(Item::as_array_mut) {
                changed |= sync_dependency_array(deps, selected_versions);
            }
        }
    }

    if changed {
        fs::write(&pyproject_path, parsed.to_string())
            .with_context(|| format!("failed to write {}", pyproject_path.display()))?;
    }

    Ok(changed)
}

fn sync_dependency_array(deps: &mut Array, selected_versions: &BTreeMap<String, String>) -> bool {
    let mut changed = false;
    for dep in deps.iter_mut() {
        let Some(raw) = dep.as_str() else {
            continue;
        };
        let Some(updated) = rewrite_dependency_constraint(raw, selected_versions) else {
            continue;
        };
        if updated != raw {
            replace_string_value(dep, updated);
            changed = true;
        }
    }
    changed
}

fn rewrite_dependency_constraint(
    dependency: &str,
    selected_versions: &BTreeMap<String, String>,
) -> Option<String> {
    let marker_start = dependency.find(';');
    let (requirement, marker) = match marker_start {
        Some(index) => (&dependency[..index], &dependency[index..]),
        None => (dependency, ""),
    };
    let requirement = requirement.trim_end();
    if requirement.contains('@') {
        return Some(dependency.to_string());
    }

    let op_index = requirement
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '<' | '>' | '=' | '!' | '~').then_some(index));
    let name_with_extras = op_index
        .map(|index| requirement[..index].trim_end())
        .unwrap_or(requirement);
    let package_name = name_with_extras
        .split(['[', ' ', '\t'])
        .next()
        .unwrap_or(name_with_extras);
    let version = selected_versions.get(package_name)?;

    Some(format!("{name_with_extras}>={version}{marker}"))
}

pub fn validate_root_wheel_metadata(
    metadata: &str,
    selected_versions: &BTreeMap<String, String>,
    extras: &[String],
) -> Result<()> {
    if extras.is_empty() {
        return Ok(());
    }

    let lower_metadata = metadata.to_ascii_lowercase();
    for line in lower_metadata.lines() {
        let Some(package) = metadata_requirement_package(line) else {
            continue;
        };
        let Some(version) = selected_versions.get(package) else {
            continue;
        };
        let Some(extra) = extras
            .iter()
            .find(|extra| metadata_line_matches_extra(line, extra))
        else {
            continue;
        };
        let expected_dependency = format!("{package}>={version}");
        if !line.contains(&expected_dependency.to_ascii_lowercase()) {
            bail!("missing wheel metadata dependency {expected_dependency} for extra {extra}");
        }
    }

    Ok(())
}

fn metadata_requirement_package(line: &str) -> Option<&str> {
    let requirement = line
        .trim_start()
        .strip_prefix("requires-dist:")?
        .trim_start();
    let name_end = requirement
        .char_indices()
        .find_map(|(index, ch)| {
            matches!(ch, '[' | ' ' | '\t' | '<' | '>' | '=' | '!' | '~' | ';').then_some(index)
        })
        .unwrap_or(requirement.len());
    if name_end == 0 {
        None
    } else {
        Some(&requirement[..name_end])
    }
}

fn metadata_line_matches_extra(line: &str, extra: &str) -> bool {
    let expected_extra_single = format!("extra == '{extra}'").to_ascii_lowercase();
    let expected_extra_double = format!("extra == \"{extra}\"").to_ascii_lowercase();
    line.contains(&expected_extra_single) || line.contains(&expected_extra_double)
}

pub fn build_explicit_install_command(
    root_name: &str,
    root_version: &str,
    extra: &str,
    packages: &[PrereleasePackage],
) -> String {
    let mut lines = vec![format!(
        "uv pip install --prerelease explicit \\\n  \"{root_name}[{extra}]=={root_version}\""
    )];
    for package in packages.iter().filter(|package| package.root != ".") {
        lines.push(format!(" \\\n  \"{}=={}\"", package.name, package.version));
    }
    lines.join("")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use tempfile::tempdir;

    use super::{
        PrereleasePackage, build_explicit_install_command, sync_python_workspace_dependencies,
        sync_root_python_workspace_dependencies, validate_root_wheel_metadata,
    };
    use crate::{
        config::WorkspaceDependenciesConfig,
        workspace_plan::{ReleaseWorkspacePlan, WorkspacePackagePlan},
    };

    #[test]
    fn generalized_sync_preserves_pyproject_formatting_and_comments() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("packages/provider")).expect("provider dir");
        let original = r#"# Provider package manifest
[build-system]
requires=["hatchling"] # compact layout is intentional

[project]
name = "provider"
# Keep runtime dependencies grouped in this order.
dependencies = [
    "httpx>=0.27", # external dependency
    "core[cli]>=0.12.1,<0.13; python_version >= '3.11'", # workspace dependency
]

[project.optional-dependencies]
test = [
  "pytest>=8",
  "core>=0.12.1,<0.13", # workspace test helper
]
docs=["sphinx>=7"]
"#;
        fs::write(
            dir.path().join("packages/provider/pyproject.toml"),
            original,
        )
        .expect("write pyproject");
        let plan = ReleaseWorkspacePlan {
            schema_version: 1,
            ecosystem: "python".to_string(),
            release_mode: "release_set".to_string(),
            base_branch: "main".to_string(),
            packages: vec![
                WorkspacePackagePlan {
                    name: "core".to_string(),
                    path: ".".to_string(),
                    selected: true,
                    selection_reason: "selected".to_string(),
                    current_version: "0.12.1".to_string(),
                    next_version: Some("0.13.0".to_string()),
                },
                WorkspacePackagePlan {
                    name: "provider".to_string(),
                    path: "packages/provider".to_string(),
                    selected: false,
                    selection_reason: "unchanged".to_string(),
                    current_version: "0.1.0".to_string(),
                    next_version: None,
                },
            ],
        };
        let config: WorkspaceDependenciesConfig = toml::from_str(
            r#"enabled = true
[[rules]]
dependency = "core"
dependents = ["packages/*"]
range = ">={version},<{next_minor}"
"#,
        )
        .expect("config");
        let operations =
            sync_python_workspace_dependencies(dir.path(), &plan, &config).expect("sync");
        let updated = fs::read_to_string(dir.path().join("packages/provider/pyproject.toml"))
            .expect("read pyproject");
        assert_eq!(operations.len(), 2);
        assert_eq!(
            updated,
            original
                .replace(
                    "core[cli]>=0.12.1,<0.13; python_version >= '3.11'",
                    "core[cli]>=0.13.0,<0.14; python_version >= '3.11'",
                )
                .replace("core>=0.12.1,<0.13", "core>=0.13.0,<0.14")
        );
    }

    #[test]
    fn normalizes_pep_503_distribution_names_when_rewriting() {
        let updated = super::rewrite_requirement("phlo_core>=0.12", "phlo-core", ">=0.13")
            .expect("rewrite")
            .expect("matched dependency");
        assert_eq!(updated, "phlo_core>=0.13");
    }

    #[test]
    fn generalized_sync_rejects_pep_508_direct_references() {
        let error = super::rewrite_requirement(
            "core[cli] @ git+https://example.com/core.git; python_version >= '3.11'",
            "core",
            ">=0.13",
        )
        .expect_err("direct references must be rejected");

        assert_eq!(
            error.to_string(),
            "direct reference for workspace dependency core is not allowed"
        );
    }

    #[test]
    fn prerelease_root_sync_preserves_pyproject_formatting_and_comments() {
        let dir = tempdir().expect("tempdir");
        let pyproject = dir.path().join("pyproject.toml");
        let original = r#"# Root distribution for the workspace
[project]
name = "phlo"
version = "0.8.1b5"
dependencies = [
  "phlo-core>=0.1.0", # unchanged workspace package
  "phlo-iceberg>=0.1.0; python_version >= '3.11'", # selected package
  "sqlalchemy>=2.0", # third party
]

[project.optional-dependencies]
defaults = [
  "phlo-iceberg>=0.1.0", # selected extra
  "phlo-dagster>=0.3.1b2",
  "duckdb>=1.0",
]
core-services = [ "phlo-minio>=0.1.0" ] # compact array
docs = [
  "phlo-iceberg>=0.1.0",
]
"#;
        fs::write(&pyproject, original).expect("write pyproject");
        let packages = BTreeMap::from([
            ("phlo-iceberg".to_string(), "0.3.1b1".to_string()),
            ("phlo-minio".to_string(), "0.3.1b1".to_string()),
        ]);

        let changed = sync_root_python_workspace_dependencies(
            dir.path(),
            &packages,
            true,
            &["defaults".to_string(), "core-services".to_string()],
        )
        .expect("sync dependencies");

        let updated = fs::read_to_string(&pyproject).expect("read updated pyproject");
        assert!(changed);
        assert_eq!(
            updated,
            original
                .replace(
                    "phlo-iceberg>=0.1.0; python_version >= '3.11'",
                    "phlo-iceberg>=0.3.1b1; python_version >= '3.11'",
                )
                .replacen("phlo-iceberg>=0.1.0", "phlo-iceberg>=0.3.1b1", 1)
                .replace("phlo-minio>=0.1.0", "phlo-minio>=0.3.1b1")
        );
    }

    #[test]
    fn skips_pep_508_direct_references_when_syncing_dependencies() {
        let dir = tempdir().expect("tempdir");
        let pyproject = dir.path().join("pyproject.toml");
        fs::write(
            &pyproject,
            r#"
[project]
name = "phlo"
version = "0.8.1b5"
dependencies = [
  "phlo-local @ file:///tmp/phlo-local",
  "phlo-iceberg>=0.1.0",
]
"#,
        )
        .expect("write pyproject");
        let packages = BTreeMap::from([
            ("phlo-local".to_string(), "0.2.0b1".to_string()),
            ("phlo-iceberg".to_string(), "0.3.1b1".to_string()),
        ]);

        let changed = sync_root_python_workspace_dependencies(dir.path(), &packages, true, &[])
            .expect("sync dependencies");

        let updated = fs::read_to_string(&pyproject).expect("read updated pyproject");
        assert!(changed);
        assert!(updated.contains("\"phlo-local @ file:///tmp/phlo-local\""));
        assert!(updated.contains("\"phlo-iceberg>=0.3.1b1\""));
    }

    #[test]
    fn validates_root_wheel_metadata_for_configured_extras() {
        let metadata = r#"
Metadata-Version: 2.3
Name: phlo
Version: 0.8.1b5
Provides-Extra: defaults
Requires-Dist: phlo-iceberg>=0.3.1b1; extra == 'defaults'
"#;
        let packages = BTreeMap::from([("phlo-iceberg".to_string(), "0.3.1b1".to_string())]);

        validate_root_wheel_metadata(metadata, &packages, &["defaults".to_string()])
            .expect("metadata should validate");
    }

    #[test]
    fn validates_only_workspace_packages_declared_for_each_configured_extra() {
        let metadata = r#"
Metadata-Version: 2.3
Name: phlo
Version: 0.8.1b5
Provides-Extra: defaults
Requires-Dist: phlo-iceberg>=0.3.1b1; extra == 'defaults'
Provides-Extra: core-services
Requires-Dist: phlo-minio>=0.3.1b1; extra == 'core-services'
"#;
        let packages = BTreeMap::from([
            ("phlo-iceberg".to_string(), "0.3.1b1".to_string()),
            ("phlo-minio".to_string(), "0.3.1b1".to_string()),
        ]);

        validate_root_wheel_metadata(
            metadata,
            &packages,
            &["defaults".to_string(), "core-services".to_string()],
        )
        .expect("metadata should validate");
    }

    #[test]
    fn fails_root_wheel_metadata_when_expected_extra_constraint_is_missing() {
        let metadata = r#"
Metadata-Version: 2.3
Name: phlo
Version: 0.8.1b5
Provides-Extra: defaults
Requires-Dist: phlo-iceberg>=0.1.0; extra == 'defaults'
"#;
        let packages = BTreeMap::from([("phlo-iceberg".to_string(), "0.3.1b1".to_string())]);

        let error = validate_root_wheel_metadata(metadata, &packages, &["defaults".to_string()])
            .expect_err("metadata should fail");

        assert!(
            error
                .to_string()
                .contains("missing wheel metadata dependency phlo-iceberg>=0.3.1b1")
        );
    }

    #[test]
    fn builds_explicit_pypi_verification_command_for_beta_packages() {
        let command = build_explicit_install_command(
            "phlo",
            "0.8.1b6",
            "defaults",
            &[
                PrereleasePackage {
                    name: "phlo".to_string(),
                    version: "0.8.1b6".to_string(),
                    root: ".".to_string(),
                    reason: "root prerelease".to_string(),
                },
                PrereleasePackage {
                    name: "phlo-iceberg".to_string(),
                    version: "0.3.1b2".to_string(),
                    root: "packages/iceberg".to_string(),
                    reason: "changed since latest tag".to_string(),
                },
            ],
        );

        assert_eq!(
            command,
            "uv pip install --prerelease explicit \\\n  \"phlo[defaults]==0.8.1b6\" \\\n  \"phlo-iceberg==0.3.1b2\""
        );
        assert!(!command.contains("--prerelease allow"));
    }
}
