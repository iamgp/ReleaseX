use std::{fmt, str::FromStr};

use anyhow::{Result, bail};

use crate::conventional_commits::ConventionalCommit;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn bump_major(&self) -> Self {
        Self {
            major: self.major + 1,
            minor: 0,
            patch: 0,
        }
    }

    pub fn bump_minor(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor + 1,
            patch: 0,
        }
    }

    pub fn bump_patch(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch + 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BumpLevel {
    None,
    Patch,
    Minor,
    Major,
}

impl BumpLevel {
    pub fn from_commits(commits: &[ConventionalCommit]) -> Self {
        commits.iter().fold(Self::None, |level, commit| {
            level.max(Self::from_commit(commit))
        })
    }

    pub fn from_commit(commit: &ConventionalCommit) -> Self {
        if commit.breaking {
            return Self::Major;
        }

        match commit.commit_type.as_str() {
            "feat" => Self::Minor,
            "fix" => Self::Patch,
            _ => Self::None,
        }
    }

    pub fn apply(self, version: &Version) -> Option<Version> {
        match self {
            Self::None => None,
            Self::Patch => Some(version.bump_patch()),
            Self::Minor => Some(version.bump_minor()),
            Self::Major => Some(version.bump_major()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let parts: Vec<_> = value.split('.').collect();
        if parts.len() != 3 {
            bail!("version must contain major.minor.patch");
        }

        Ok(Self {
            major: parts[0].parse()?,
            minor: parts[1].parse()?,
            patch: parts[2].parse()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::conventional_commits::ConventionalCommit;

    use super::{BumpLevel, Version};

    #[test]
    fn bumps_minor() {
        let version = Version::from_str("1.2.3").expect("valid version");
        assert_eq!(version.bump_minor().to_string(), "1.3.0");
    }

    #[test]
    fn selects_major_bump_from_breaking_commits() {
        let commits = vec![
            ConventionalCommit::parse_message("fix: patch").expect("parse"),
            ConventionalCommit::parse_message("feat!: break api").expect("parse"),
        ];

        assert_eq!(BumpLevel::from_commits(&commits), BumpLevel::Major);
    }
}
