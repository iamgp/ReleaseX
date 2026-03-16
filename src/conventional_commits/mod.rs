use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConventionalCommit {
    pub commit_type: String,
    pub description: String,
    pub breaking: bool,
}

impl FromStr for ConventionalCommit {
    type Err = ConventionalCommitError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (head, description) = value
            .split_once(':')
            .ok_or(ConventionalCommitError::MissingDelimiter)?;
        let description = description.trim();
        if description.is_empty() {
            return Err(ConventionalCommitError::MissingDescription);
        }

        let breaking = head.ends_with('!');
        let commit_type = head
            .trim_end_matches('!')
            .split_once('(')
            .map(|(ty, _)| ty)
            .unwrap_or(head.trim_end_matches('!'))
            .trim();
        if commit_type.is_empty() {
            return Err(ConventionalCommitError::MissingType);
        }

        Ok(Self {
            commit_type: commit_type.to_string(),
            description: description.to_string(),
            breaking,
        })
    }
}

impl ConventionalCommit {
    pub fn parse_message(message: &str) -> Result<Self, ConventionalCommitError> {
        let mut lines = message.lines();
        let subject = lines
            .next()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .ok_or(ConventionalCommitError::MissingDescription)?;
        let mut commit = Self::from_str(subject)?;

        for line in lines {
            let line = line.trim();
            if line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:") {
                commit.breaking = true;
                break;
            }
        }

        Ok(commit)
    }
}

#[derive(Debug, Error)]
pub enum ConventionalCommitError {
    #[error("commit message must contain ':' delimiter")]
    MissingDelimiter,
    #[error("commit type is missing")]
    MissingType,
    #[error("commit description is missing")]
    MissingDescription,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::ConventionalCommit;

    #[test]
    fn parses_breaking_commit() {
        let commit = ConventionalCommit::from_str("feat!: break api").expect("should parse");
        assert_eq!(commit.commit_type, "feat");
        assert!(commit.breaking);
    }

    #[test]
    fn parses_scope_and_footer_breaking_change() {
        let commit = ConventionalCommit::parse_message(
            "feat(api): add endpoint\n\nBREAKING CHANGE: removed old endpoint",
        )
        .expect("should parse");
        assert_eq!(commit.commit_type, "feat");
        assert!(commit.breaking);
    }
}
