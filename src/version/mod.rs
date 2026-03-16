use std::{fmt, str::FromStr};

use anyhow::{Result, bail};

use crate::conventional_commits::ConventionalCommit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreRelease {
    Alpha(u64),
    Beta(u64),
    Rc(u64),
}

impl PreRelease {
    fn order_key(&self) -> (u8, u64) {
        match self {
            Self::Alpha(n) => (0, *n),
            Self::Beta(n) => (1, *n),
            Self::Rc(n) => (2, *n),
        }
    }

    fn bump(&self) -> Self {
        match self {
            Self::Alpha(n) => Self::Alpha(n + 1),
            Self::Beta(n) => Self::Beta(n + 1),
            Self::Rc(n) => Self::Rc(n + 1),
        }
    }
}

impl PartialOrd for PreRelease {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PreRelease {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order_key().cmp(&other.order_key())
    }
}

impl fmt::Display for PreRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Alpha(n) => write!(f, "a{n}"),
            Self::Beta(n) => write!(f, "b{n}"),
            Self::Rc(n) => write!(f, "rc{n}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suffix {
    Pre(PreRelease),
    Post(u64),
    Dev(u64),
}

impl PartialOrd for Suffix {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Suffix {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order_key().cmp(&other.order_key())
    }
}

impl Suffix {
    fn order_key(&self) -> (u8, u8, u64) {
        match self {
            Self::Dev(n) => (0, 0, *n),
            Self::Pre(pre) => {
                let (kind, n) = pre.order_key();
                (1, kind, n)
            }
            Self::Post(n) => (3, 0, *n),
        }
    }
}

impl fmt::Display for Suffix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pre(pre) => write!(f, "{pre}"),
            Self::Post(n) => write!(f, ".post{n}"),
            Self::Dev(n) => write!(f, ".dev{n}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub suffix: Option<Suffix>,
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let base =
            (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if base != std::cmp::Ordering::Equal {
            return base;
        }
        match (&self.suffix, &other.suffix) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(Suffix::Post(_))) => std::cmp::Ordering::Less,
            (Some(Suffix::Post(_)), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}

impl Version {
    pub fn base(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch,
            suffix: None,
        }
    }

    pub fn bump_major(&self) -> Self {
        Self {
            major: self.major + 1,
            minor: 0,
            patch: 0,
            suffix: None,
        }
    }

    pub fn bump_minor(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor + 1,
            patch: 0,
            suffix: None,
        }
    }

    pub fn bump_patch(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch + 1,
            suffix: None,
        }
    }

    pub fn bump_pre(&self, kind: &str) -> Result<Self> {
        match &self.suffix {
            Some(Suffix::Pre(pre)) => {
                let same_kind = matches!(
                    (kind, pre),
                    ("a", PreRelease::Alpha(_))
                        | ("b", PreRelease::Beta(_))
                        | ("rc", PreRelease::Rc(_))
                );
                if same_kind {
                    return Ok(Self {
                        major: self.major,
                        minor: self.minor,
                        patch: self.patch,
                        suffix: Some(Suffix::Pre(pre.bump())),
                    });
                }
                let new_pre = match kind {
                    "a" => PreRelease::Alpha(1),
                    "b" => PreRelease::Beta(1),
                    "rc" => PreRelease::Rc(1),
                    _ => bail!("unknown pre-release kind: {kind}"),
                };
                if new_pre < *pre {
                    bail!("cannot go from {} to {kind} pre-release", pre);
                }
                Ok(Self {
                    major: self.major,
                    minor: self.minor,
                    patch: self.patch,
                    suffix: Some(Suffix::Pre(new_pre)),
                })
            }
            _ => {
                let new_pre = match kind {
                    "a" => PreRelease::Alpha(1),
                    "b" => PreRelease::Beta(1),
                    "rc" => PreRelease::Rc(1),
                    _ => bail!("unknown pre-release kind: {kind}"),
                };
                Ok(Self {
                    major: self.major,
                    minor: self.minor,
                    patch: self.patch,
                    suffix: Some(Suffix::Pre(new_pre)),
                })
            }
        }
    }

    pub fn bump_post(&self) -> Self {
        let n = match &self.suffix {
            Some(Suffix::Post(n)) => n + 1,
            _ => 1,
        };
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch,
            suffix: Some(Suffix::Post(n)),
        }
    }

    pub fn bump_dev(&self) -> Self {
        let n = match &self.suffix {
            Some(Suffix::Dev(n)) => n + 1,
            _ => 1,
        };
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch,
            suffix: Some(Suffix::Dev(n)),
        }
    }

    pub fn finalize(&self) -> Self {
        self.base()
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
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(suffix) = &self.suffix {
            write!(f, "{suffix}")?;
        }
        Ok(())
    }
}

impl FromStr for Version {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (base, suffix) = parse_suffix(value);
        let parts: Vec<_> = base.split('.').collect();
        if parts.len() != 3 {
            bail!("version must contain major.minor.patch");
        }

        Ok(Self {
            major: parts[0].parse()?,
            minor: parts[1].parse()?,
            patch: parts[2].parse()?,
            suffix,
        })
    }
}

fn parse_suffix(value: &str) -> (&str, Option<Suffix>) {
    if let Some((base, n)) = value.rsplit_once(".post")
        && let Ok(n) = n.parse::<u64>()
    {
        return (base, Some(Suffix::Post(n)));
    }
    if let Some((base, n)) = value.rsplit_once(".dev")
        && let Ok(n) = n.parse::<u64>()
    {
        return (base, Some(Suffix::Dev(n)));
    }
    for prefix in ["rc", "a", "b"] {
        if let Some(pos) = value.rfind(prefix) {
            let base = &value[..pos];
            let n_str = &value[pos + prefix.len()..];
            if let Ok(n) = n_str.parse::<u64>() {
                if base.ends_with('.') || base.is_empty() {
                    continue;
                }
                let pre = match prefix {
                    "a" => PreRelease::Alpha(n),
                    "b" => PreRelease::Beta(n),
                    "rc" => PreRelease::Rc(n),
                    _ => unreachable!(),
                };
                return (base, Some(Suffix::Pre(pre)));
            }
        }
    }
    (value, None)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::conventional_commits::ConventionalCommit;

    use super::{BumpLevel, PreRelease, Suffix, Version};

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

    #[test]
    fn parses_plain_version() {
        let v = Version::from_str("1.2.3").expect("parse");
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert_eq!(v.suffix, None);
    }

    #[test]
    fn parses_alpha_version() {
        let v = Version::from_str("1.2.3a1").expect("parse");
        assert_eq!(v.suffix, Some(Suffix::Pre(PreRelease::Alpha(1))));
        assert_eq!(v.to_string(), "1.2.3a1");
    }

    #[test]
    fn parses_beta_version() {
        let v = Version::from_str("1.2.3b2").expect("parse");
        assert_eq!(v.suffix, Some(Suffix::Pre(PreRelease::Beta(2))));
        assert_eq!(v.to_string(), "1.2.3b2");
    }

    #[test]
    fn parses_rc_version() {
        let v = Version::from_str("1.2.3rc1").expect("parse");
        assert_eq!(v.suffix, Some(Suffix::Pre(PreRelease::Rc(1))));
        assert_eq!(v.to_string(), "1.2.3rc1");
    }

    #[test]
    fn parses_post_version() {
        let v = Version::from_str("1.2.3.post1").expect("parse");
        assert_eq!(v.suffix, Some(Suffix::Post(1)));
        assert_eq!(v.to_string(), "1.2.3.post1");
    }

    #[test]
    fn parses_dev_version() {
        let v = Version::from_str("1.2.3.dev1").expect("parse");
        assert_eq!(v.suffix, Some(Suffix::Dev(1)));
        assert_eq!(v.to_string(), "1.2.3.dev1");
    }

    #[test]
    fn bump_pre_alpha_from_release() {
        let v = Version::from_str("1.1.0").expect("parse");
        assert_eq!(v.bump_pre("a").unwrap().to_string(), "1.1.0a1");
    }

    #[test]
    fn bump_pre_alpha_increments() {
        let v = Version::from_str("1.1.0a1").expect("parse");
        assert_eq!(v.bump_pre("a").unwrap().to_string(), "1.1.0a2");
    }

    #[test]
    fn bump_pre_alpha_to_rc() {
        let v = Version::from_str("1.1.0a2").expect("parse");
        assert_eq!(v.bump_pre("rc").unwrap().to_string(), "1.1.0rc1");
    }

    #[test]
    fn finalize_pre_release() {
        let v = Version::from_str("1.1.0rc1").expect("parse");
        assert_eq!(v.finalize().to_string(), "1.1.0");
    }

    #[test]
    fn bump_post_from_release() {
        let v = Version::from_str("1.2.3").expect("parse");
        assert_eq!(v.bump_post().to_string(), "1.2.3.post1");
    }

    #[test]
    fn bump_post_increments() {
        let v = Version::from_str("1.2.3.post1").expect("parse");
        assert_eq!(v.bump_post().to_string(), "1.2.3.post2");
    }

    #[test]
    fn bump_dev_from_release() {
        let v = Version::from_str("1.2.3").expect("parse");
        assert_eq!(v.bump_dev().to_string(), "1.2.3.dev1");
    }

    #[test]
    fn bump_dev_increments() {
        let v = Version::from_str("1.2.3.dev1").expect("parse");
        assert_eq!(v.bump_dev().to_string(), "1.2.3.dev2");
    }

    #[test]
    fn ordering_pre_release_before_release() {
        let alpha = Version::from_str("1.0.0a1").expect("parse");
        let release = Version::from_str("1.0.0").expect("parse");
        assert!(alpha < release);
    }

    #[test]
    fn ordering_post_release_after_release() {
        let release = Version::from_str("1.0.0").expect("parse");
        let post = Version::from_str("1.0.0.post1").expect("parse");
        assert!(post > release);
    }

    #[test]
    fn ordering_dev_before_alpha() {
        let dev = Version::from_str("1.0.0.dev1").expect("parse");
        let alpha = Version::from_str("1.0.0a1").expect("parse");
        assert!(dev < alpha);
    }

    #[test]
    fn ordering_alpha_beta_rc() {
        let a = Version::from_str("1.0.0a1").expect("parse");
        let b = Version::from_str("1.0.0b1").expect("parse");
        let rc = Version::from_str("1.0.0rc1").expect("parse");
        assert!(a < b);
        assert!(b < rc);
    }

    #[test]
    fn bump_pre_rejects_downgrade() {
        let v = Version::from_str("1.0.0rc1").expect("parse");
        assert!(v.bump_pre("a").is_err());
    }

    #[test]
    fn bump_major_clears_suffix() {
        let v = Version::from_str("1.0.0a1").expect("parse");
        assert_eq!(v.bump_major().to_string(), "2.0.0");
    }
}
