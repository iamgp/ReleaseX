use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result};

use crate::{
    config::{ChangelogConfig, Config},
    conventional_commits::ConventionalCommit,
    git::CommitSummary,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributorInfo {
    pub name: String,
    pub commit_count: usize,
    pub first_contribution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChangelog {
    pub sections: BTreeMap<String, Vec<String>>,
    pub contributors: Vec<ContributorInfo>,
}

const DEFAULT_BOT_PATTERNS: &[&str] = &["dependabot", "renovate", "github-actions", "[bot]"];

impl PendingChangelog {
    pub fn from_commits(config: &Config, commits: &[ConventionalCommit]) -> Self {
        let mut sections = BTreeMap::new();

        for commit in commits {
            if commit.breaking {
                sections
                    .entry("Breaking Changes".to_string())
                    .or_insert_with(Vec::new)
                    .push(commit.description.clone());
            }

            let Some(section) = config.section_for_commit_type(&commit.commit_type) else {
                continue;
            };

            sections
                .entry(section)
                .or_insert_with(Vec::new)
                .push(commit.description.clone());
        }

        Self {
            sections,
            contributors: Vec::new(),
        }
    }

    pub fn add_contributors(
        &mut self,
        commits: &[CommitSummary],
        known_authors: &BTreeSet<String>,
        changelog_config: &ChangelogConfig,
    ) {
        let bot_patterns: Vec<String> = if changelog_config.bot_patterns.is_empty() {
            DEFAULT_BOT_PATTERNS.iter().map(|s| s.to_string()).collect()
        } else {
            changelog_config.bot_patterns.clone()
        };

        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for commit in commits {
            *counts.entry(commit.author.clone()).or_insert(0) += 1;
        }

        let mut contributors = Vec::new();
        for (name, commit_count) in &counts {
            if changelog_config.exclude_bots {
                let lower = name.to_lowercase();
                if bot_patterns
                    .iter()
                    .any(|p| lower.contains(&p.to_lowercase()))
                {
                    continue;
                }
            }

            contributors.push(ContributorInfo {
                name: name.clone(),
                commit_count: *commit_count,
                first_contribution: !commits.iter().filter(|commit| commit.author == *name).any(
                    |commit| {
                        known_authors.contains(name) || known_authors.contains(&commit.raw_author)
                    },
                ),
            });
        }

        contributors.sort_by(|a, b| {
            b.commit_count
                .cmp(&a.commit_count)
                .then(a.name.cmp(&b.name))
        });
        self.contributors = contributors;
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

pub fn next_release_heading(version: &str, date: &str) -> String {
    format!("## [{version}] - {date}")
}

/// Web base URL derived from the API base, so links point at the human UI
/// (including GitHub Enterprise) rather than the API.
pub fn web_base(api_base: &str) -> String {
    let trimmed = api_base.trim_end_matches('/');
    if trimmed == "https://api.github.com" {
        return "https://github.com".to_string();
    }
    trimmed
        .strip_suffix("/api/v3")
        .unwrap_or(trimmed)
        .to_string()
}

/// Turn bare `#123` references into issue links. Existing markdown links
/// and already-qualified paths are left alone.
pub fn linkify_github_refs(text: &str, web_base: &str, owner: &str, repo: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '#'
            && (index == 0 || !matches!(chars[index - 1], '[' | '/' | '#' | '0'..='9'))
        {
            let mut end = index + 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            if end > index + 1 {
                let number: String = chars[index + 1..end].iter().collect();
                output.push_str(&format!(
                    "[#{number}]({web_base}/{owner}/{repo}/issues/{number})"
                ));
                index = end;
                continue;
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}

/// Append a compare-range footer linking the previous tag to the new one.
pub fn append_compare_link(
    notes: &str,
    web_base: &str,
    owner: &str,
    repo: &str,
    base_tag: &str,
    head_tag: &str,
) -> String {
    format!(
        "{notes}\n\n---\nFull Changelog: {web_base}/{owner}/{repo}/compare/{base_tag}...{head_tag}"
    )
}

pub fn render_release_notes(
    version: &str,
    date: &str,
    changelog: &PendingChangelog,
    first_contribution_emoji: &str,
) -> String {
    let mut output = String::new();
    output.push_str(&next_release_heading(version, date));
    output.push('\n');

    for (section, entries) in &changelog.sections {
        output.push('\n');
        output.push_str(&format!("### {section}\n"));
        for entry in entries {
            output.push_str(&format!("- {entry}\n"));
        }
    }

    if !changelog.contributors.is_empty() {
        output.push_str("\n### Contributors\n");
        output.push_str("Thanks to our contributors for this release:\n");
        for contributor in &changelog.contributors {
            if contributor.first_contribution {
                output.push_str(&format!(
                    "- {} @{} — first contribution!\n",
                    first_contribution_emoji, contributor.name
                ));
            } else {
                output.push_str(&format!(
                    "- @{} ({} {})\n",
                    contributor.name,
                    contributor.commit_count,
                    if contributor.commit_count == 1 {
                        "commit"
                    } else {
                        "commits"
                    }
                ));
            }
        }
    }

    output.trim_end().to_string()
}

pub fn prepend_release_notes(path: &Path, release_notes: &str) -> Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };

    let updated = if existing.trim().is_empty() {
        format!("# Changelog\n\n{release_notes}\n")
    } else if let Some(header_end) = existing.find('\n') {
        let (head, tail) = existing.split_at(header_end + 1);
        format!("{head}\n{release_notes}\n\n{}", tail.trim_start())
    } else {
        format!("{existing}\n\n{release_notes}\n")
    };

    fs::write(path, updated).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
    };

    use crate::config::Config;

    use anyhow::Result;
    use tempfile::tempdir;

    use super::{
        PendingChangelog, append_compare_link, linkify_github_refs, next_release_heading,
        prepend_release_notes, render_release_notes, web_base,
    };

    #[test]
    fn builds_heading() {
        assert_eq!(
            next_release_heading("1.2.0", "2026-03-16"),
            "## [1.2.0] - 2026-03-16"
        );
    }

    #[test]
    fn linkifies_issue_refs_without_touching_links() {
        let linked = linkify_github_refs(
            "add search (#42) and [#7](https://example.com) plus /issues/9",
            "https://github.com",
            "acme",
            "demo",
        );
        assert!(linked.contains("[#42](https://github.com/acme/demo/issues/42)"));
        assert!(linked.contains("[#7](https://example.com)"));
        assert!(linked.contains("/issues/9"));
    }

    #[test]
    fn web_base_handles_github_enterprise() {
        assert_eq!(web_base("https://api.github.com"), "https://github.com");
        assert_eq!(
            web_base("https://ghe.example.com/api/v3"),
            "https://ghe.example.com"
        );
    }

    #[test]
    fn compare_link_appends_range() {
        let notes = append_compare_link(
            "notes",
            "https://github.com",
            "acme",
            "demo",
            "v0.6.0",
            "v0.7.0",
        );
        assert!(notes.contains("compare/v0.6.0...v0.7.0"));
    }

    #[test]
    fn groups_commit_sections_from_config() {
        let config = toml::from_str::<Config>(
            r#"
            [[version_files]]
            path = "pyproject.toml"
            key = "project.version"

            [changelog.sections]
            feat = "Added"
            fix = "Fixed"
            docs = false
            "#,
        )
        .expect("config");
        let commits = vec![
            crate::conventional_commits::ConventionalCommit::parse_message("feat: add search")
                .expect("parse"),
            crate::conventional_commits::ConventionalCommit::parse_message("docs: update readme")
                .expect("parse"),
        ];

        let changelog = PendingChangelog::from_commits(&config, &commits);
        assert_eq!(
            changelog.sections.get("Added"),
            Some(&vec!["add search".to_string()])
        );
        assert!(!changelog.sections.contains_key("docs"));
    }

    #[test]
    fn renders_release_notes() {
        let changelog = PendingChangelog {
            sections: BTreeMap::from([("Added".to_string(), vec!["ship it".to_string()])]),
            contributors: Vec::new(),
        };

        let notes = render_release_notes("1.2.0", "2026-03-16", &changelog, "🎉");
        assert!(notes.contains("## [1.2.0] - 2026-03-16"));
        assert!(notes.contains("### Added"));
        assert!(notes.contains("- ship it"));
    }

    #[test]
    fn contributor_first_contribution_uses_raw_author_fallback() {
        let mut changelog = PendingChangelog {
            sections: BTreeMap::new(),
            contributors: Vec::new(),
        };
        let commits = vec![crate::git::CommitSummary {
            id: "abc123".to_string(),
            message: "fix: example".to_string(),
            changed_paths: vec!["src/lib.rs".to_string()],
            author: "iamgp".to_string(),
            raw_author: "Gareth Price".to_string(),
        }];
        let known_authors = BTreeSet::from(["Gareth Price".to_string()]);
        let config: Config = toml::from_str(
            r#"
            [[version_files]]
            path = "pyproject.toml"
            key = "project.version"
            "#,
        )
        .expect("config");

        changelog.add_contributors(&commits, &known_authors, &config.changelog);

        assert_eq!(changelog.contributors.len(), 1);
        assert!(!changelog.contributors[0].first_contribution);
        assert_eq!(changelog.contributors[0].name, "iamgp");
    }

    #[test]
    fn prepends_release_notes_after_heading() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("CHANGELOG.md");
        fs::write(&path, "# Changelog\n\n## [0.1.0] - 2026-01-01\n")?;

        prepend_release_notes(&path, "## [0.2.0] - 2026-03-16\n\n### Added\n- feature")?;

        let content = fs::read_to_string(path)?;
        assert!(content.starts_with("# Changelog\n\n## [0.2.0] - 2026-03-16"));
        assert!(content.contains("## [0.1.0] - 2026-01-01"));
        Ok(())
    }
}
