use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use git2::{DescribeFormatOptions, DescribeOptions, Repository};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    pub id: String,
    pub message: String,
    pub changed_paths: Vec<String>,
}

pub struct GitRepository {
    inner: Repository,
    root: PathBuf,
}

impl GitRepository {
    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        let inner = Repository::discover(path).context("unable to find git repository")?;
        let root = inner
            .workdir()
            .map(Path::to_path_buf)
            .or_else(|| inner.path().parent().map(Path::to_path_buf))
            .context("repository has no accessible working directory")?;

        Ok(Self { inner, root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn current_branch(&self) -> Result<String> {
        if let Ok(branch) = run_git(self.path(), ["branch", "--show-current"]) {
            if !branch.trim().is_empty() {
                return Ok(branch);
            }
        }

        let head = self.inner.head().context("failed to read HEAD")?;
        if let Some(name) = head.shorthand() {
            return Ok(name.to_string());
        }

        if let Some(name) = head.name() {
            if let Some(branch) = name.strip_prefix("refs/heads/") {
                return Ok(branch.to_string());
            }
        }

        Ok("HEAD".to_string())
    }

    pub fn latest_tag(&self) -> Result<Option<String>> {
        let description = self
            .inner
            .describe(
                DescribeOptions::new()
                    .describe_tags()
                    .show_commit_oid_as_fallback(false),
            )
            .ok();

        match description {
            Some(description) => Ok(Some(
                description
                    .format(Some(DescribeFormatOptions::new().abbreviated_size(0)))
                    .context("failed to format tag description")?,
            )),
            None => Ok(None),
        }
    }

    pub fn commits_since_latest_tag(&self) -> Result<Vec<CommitSummary>> {
        let head = self.inner.head()?.peel_to_commit()?;
        let last_tag_commit = self
            .latest_tag()?
            .and_then(|tag| self.inner.revparse_single(&tag).ok())
            .and_then(|object| object.peel_to_commit().ok());

        let mut revwalk = self.inner.revwalk().context("failed to create revwalk")?;
        revwalk.push(head.id())?;
        if let Some(tag_commit) = last_tag_commit {
            revwalk.hide(tag_commit.id())?;
        }

        let mut commits = Vec::new();
        for oid in revwalk {
            let oid = oid?;
            let commit = self.inner.find_commit(oid)?;
            let changed_paths = changed_paths_for_commit(&self.inner, &commit)?;
            commits.push(CommitSummary {
                id: oid.to_string(),
                message: commit.message().unwrap_or_default().trim().to_string(),
                changed_paths,
            });
        }

        commits.reverse();
        Ok(commits)
    }

    pub fn remote_url(&self, name: &str) -> Result<Option<String>> {
        match self.inner.find_remote(name) {
            Ok(remote) => Ok(remote.url().map(str::to_string)),
            Err(_) => Ok(None),
        }
    }
}

fn changed_paths_for_commit(inner: &Repository, commit: &git2::Commit<'_>) -> Result<Vec<String>> {
    let tree = commit.tree().context("failed to read commit tree")?;
    let parent_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(
            commit
                .parent(0)
                .context("failed to read commit parent")?
                .tree()
                .context("failed to read parent tree")?,
        )
    };
    let diff = inner
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .context("failed to diff commit tree")?;
    let mut paths = Vec::new();
    for delta in diff.deltas() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|path| path.to_string_lossy().replace('\\', "/"));
        if let Some(path) = path {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub fn run_git<I, S>(repo_path: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to run git in {}", repo_path.display()))?;

    if !output.status.success() {
        bail!(
            "git command failed in {}: {}",
            repo_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
