use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::{config::ReleaseReplacementConfig, workspace_plan::ReleaseWorkspacePlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacementOperation {
    pub package: String,
    pub file: String,
    pub search: String,
    pub replace: String,
    pub matches: usize,
}

pub fn planned_operations(
    root: &Path,
    replacements: &[ReleaseReplacementConfig],
    plan: &ReleaseWorkspacePlan,
) -> Result<Vec<ReplacementOperation>> {
    if replacements.is_empty() {
        return Ok(Vec::new());
    }
    let files = workspace_files(root)?;
    let mut operations = Vec::new();
    for replacement in replacements {
        for package in plan.packages.iter().filter(|package| {
            package.selected
                && (replacement.packages.is_empty() || replacement.packages.contains(&package.name))
        }) {
            let next_version = package
                .next_version
                .as_deref()
                .context("selected package has no next version")?;
            let search = expand(&replacement.search, package, next_version);
            let replace = expand(&replacement.replace, package, next_version);
            let matching_files = files
                .iter()
                .filter(|file| {
                    replacement
                        .files
                        .iter()
                        .any(|glob| glob_matches(glob, file))
                })
                .collect::<Vec<_>>();
            if matching_files.is_empty() {
                bail!(
                    "release replacement file glob matched no files for package {}",
                    package.name
                );
            }
            for file in matching_files {
                let bytes = fs::read(root.join(file))
                    .with_context(|| format!("failed to read replacement target {file}"))?;
                let matches = count_literal(&bytes, search.as_bytes());
                if matches != replacement.expected_matches {
                    bail!(
                        "release replacement for package {} in {} expected {} match(es), found {}",
                        package.name,
                        file,
                        replacement.expected_matches,
                        matches
                    );
                }
                operations.push(ReplacementOperation {
                    package: package.name.clone(),
                    file: file.clone(),
                    search: search.clone(),
                    replace: replace.clone(),
                    matches,
                });
            }
        }
    }
    Ok(operations)
}

pub fn apply(
    root: &Path,
    replacements: &[ReleaseReplacementConfig],
    plan: &ReleaseWorkspacePlan,
) -> Result<Vec<ReplacementOperation>> {
    let operations = planned_operations(root, replacements, plan)?;
    for operation in &operations {
        let path = root.join(&operation.file);
        let bytes = fs::read(&path)?;
        let updated = replace_literal(
            &bytes,
            operation.search.as_bytes(),
            operation.replace.as_bytes(),
        );
        fs::write(&path, updated)
            .with_context(|| format!("failed to write replacement target {}", path.display()))?;
    }
    Ok(operations)
}

fn expand(
    template: &str,
    package: &crate::workspace_plan::WorkspacePackagePlan,
    next_version: &str,
) -> String {
    template
        .replace("{name}", &package.name)
        .replace("{path}", &package.path)
        .replace("{current_version}", &package.current_version)
        .replace("{next_version}", next_version)
}

fn workspace_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() && entry.file_name() != ".git" {
            collect_files(root, &path, files)?;
        } else if file_type.is_file() {
            files.push(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn count_literal(bytes: &[u8], needle: &[u8]) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(offset) = bytes[start..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        count += 1;
        start += offset + needle.len();
    }
    count
}

fn replace_literal(bytes: &[u8], search: &[u8], replace: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(bytes.len());
    let mut start = 0;
    while let Some(offset) = bytes[start..]
        .windows(search.len())
        .position(|window| window == search)
    {
        let index = start + offset;
        result.extend_from_slice(&bytes[start..index]);
        result.extend_from_slice(replace);
        start = index + search.len();
    }
    result.extend_from_slice(&bytes[start..]);
    result
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
        let Some(after) = remainder.strip_prefix(parts[0]) else {
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
    pattern.ends_with('*') || remainder.ends_with(parts.last().expect("glob has a final segment"))
}

#[cfg(test)]
mod tests {
    use super::{apply, planned_operations};
    use crate::{
        config::ReleaseReplacementConfig,
        workspace_plan::{ReleaseWorkspacePlan, WorkspacePackagePlan},
    };
    use tempfile::tempdir;

    fn plan() -> ReleaseWorkspacePlan {
        ReleaseWorkspacePlan {
            schema_version: 1,
            ecosystem: "python".into(),
            release_mode: "release_set".into(),
            base_branch: "main".into(),
            packages: vec![
                WorkspacePackagePlan {
                    name: "phlo".into(),
                    path: ".".into(),
                    selected: true,
                    selection_reason: "test".into(),
                    current_version: "1.0.0".into(),
                    next_version: Some("1.1.0".into()),
                },
                WorkspacePackagePlan {
                    name: "phlo-api".into(),
                    path: "packages/api".into(),
                    selected: true,
                    selection_reason: "test".into(),
                    current_version: "2.0.0".into(),
                    next_version: Some("2.0.1".into()),
                },
            ],
        }
    }
    fn replacement(
        files: Vec<&str>,
        packages: Vec<&str>,
        search: &str,
        replace: &str,
    ) -> ReleaseReplacementConfig {
        ReleaseReplacementConfig {
            files: files.into_iter().map(str::to_string).collect(),
            packages: packages.into_iter().map(str::to_string).collect(),
            for_each: "selected_packages".into(),
            search: search.into(),
            replace: replace.into(),
            expected_matches: 1,
        }
    }

    #[test]
    fn replaces_json_and_yaml_for_multiple_selected_packages_without_changing_other_bytes() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("registry/support")).unwrap();
        std::fs::create_dir_all(dir.path().join("services")).unwrap();
        std::fs::write(
            dir.path().join("registry/support/v1.json"),
            "{\n  \"name\": \"phlo\", \"version\": \"1.0.0\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("services/api.yaml"),
            "image: ghcr.io/acme/phlo-api:2.0.0\n",
        )
        .unwrap();
        let replacements = vec![
            replacement(
                vec!["registry/**/*.json"],
                vec!["phlo"],
                "\"name\": \"{name}\", \"version\": \"{current_version}\"",
                "\"name\": \"{name}\", \"version\": \"{next_version}\"",
            ),
            replacement(
                vec!["services/*.yaml"],
                vec!["phlo-api"],
                "ghcr.io/acme/{name}:{current_version}",
                "ghcr.io/acme/{name}:{next_version}",
            ),
        ];
        apply(dir.path(), &replacements, &plan()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("registry/support/v1.json")).unwrap(),
            "{\n  \"name\": \"phlo\", \"version\": \"1.1.0\"\n}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("services/api.yaml")).unwrap(),
            "image: ghcr.io/acme/phlo-api:2.0.1\n"
        );
    }

    #[test]
    fn plans_and_applies_exact_literal_matches_and_rejects_stale_reruns() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("support.json"),
            "x \"name\": \"phlo\", \"version\": \"1.0.0\" y\n",
        )
        .unwrap();
        let mut plan = plan();
        plan.packages.truncate(1);
        let config = vec![replacement(
            vec!["*.json"],
            vec![],
            "\"name\": \"{name}\", \"version\": \"{current_version}\"",
            "\"name\": \"{name}\", \"version\": \"{next_version}\"",
        )];
        assert_eq!(
            planned_operations(dir.path(), &config, &plan).unwrap()[0].matches,
            1
        );
        apply(dir.path(), &config, &plan).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("support.json")).unwrap(),
            "x \"name\": \"phlo\", \"version\": \"1.1.0\" y\n"
        );
        assert!(apply(dir.path(), &config, &plan).is_err());
    }

    #[test]
    fn rejects_no_or_excess_matches() {
        let dir = tempdir().unwrap();
        let mut plan = plan();
        plan.packages.truncate(1);
        std::fs::write(dir.path().join("support.json"), "a a").unwrap();
        let config = vec![replacement(vec!["*.json"], vec![], "a", "b")];
        assert!(
            planned_operations(dir.path(), &config, &plan)
                .unwrap_err()
                .to_string()
                .contains("expected 1 match(es), found 2")
        );
        std::fs::write(dir.path().join("support.json"), "missing").unwrap();
        assert!(
            planned_operations(dir.path(), &config, &plan)
                .unwrap_err()
                .to_string()
                .contains("expected 1 match(es), found 0")
        );
    }
}
