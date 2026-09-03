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
    pub author: String,
    pub raw_author: String,
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
        if let Ok(branch) = run_git(self.path(), ["branch", "--show-current"])
            && !branch.trim().is_empty()
        {
            return Ok(branch);
        }

        let head = self.inner.head().context("failed to read HEAD")?;
        if let Some(name) = head.shorthand() {
            return Ok(name.to_string());
        }

        if let Some(name) = head.name()
            && let Some(branch) = name.strip_prefix("refs/heads/")
        {
            return Ok(branch.to_string());
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

    pub fn previous_tag_before_head(&self) -> Result<Option<String>> {
        let tags_at_head = run_git(self.path(), ["tag", "--points-at", "HEAD"])?;
        if tags_at_head.lines().next().is_none() {
            return Ok(None);
        }

        match run_git(self.path(), ["describe", "--tags", "--abbrev=0", "HEAD^"]) {
            Ok(tag) if !tag.trim().is_empty() => Ok(Some(tag)),
            Ok(_) => Ok(None),
            Err(_) => Ok(None),
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
                author: commit.author().name().unwrap_or("unknown").to_string(),
                raw_author: commit.author().name().unwrap_or("unknown").to_string(),
            });
        }

        commits.reverse();
        Ok(commits)
    }

    pub fn commits_since_tag(&self, tag: &str) -> Result<Vec<CommitSummary>> {
        let head = self.inner.head()?.peel_to_commit()?;
        let tag_object = self
            .inner
            .revparse_single(tag)
            .with_context(|| format!("tag '{}' not found", tag))?;
        let tag_commit = tag_object
            .peel_to_commit()
            .with_context(|| format!("tag '{}' does not point to a commit", tag))?;

        let mut revwalk = self.inner.revwalk().context("failed to create revwalk")?;
        revwalk.push(head.id())?;
        revwalk.hide(tag_commit.id())?;

        let mut commits = Vec::new();
        for oid in revwalk {
            let oid = oid?;
            let commit = self.inner.find_commit(oid)?;
            let changed_paths = changed_paths_for_commit(&self.inner, &commit)?;
            commits.push(CommitSummary {
                id: oid.to_string(),
                message: commit.message().unwrap_or_default().trim().to_string(),
                changed_paths,
                author: commit.author().name().unwrap_or("unknown").to_string(),
                raw_author: commit.author().name().unwrap_or("unknown").to_string(),
            });
        }

        commits.reverse();
        Ok(commits)
    }

    pub fn authors_before_latest_tag(&self) -> Result<Vec<String>> {
        let tag_commit = self
            .latest_tag()?
            .and_then(|tag| self.inner.revparse_single(&tag).ok())
            .and_then(|object| object.peel_to_commit().ok());

        let Some(tag_commit) = tag_commit else {
            return Ok(Vec::new());
        };

        let mut revwalk = self.inner.revwalk().context("failed to create revwalk")?;
        revwalk.push(tag_commit.id())?;

        let mut authors = std::collections::BTreeSet::new();
        for oid in revwalk {
            let oid = oid?;
            let commit = self.inner.find_commit(oid)?;
            if let Some(name) = commit.author().name() {
                authors.insert(name.to_string());
            }
        }

        Ok(authors.into_iter().collect())
    }

    pub fn remote_url(&self, name: &str) -> Result<Option<String>> {
        match self.inner.find_remote(name) {
            Ok(remote) => Ok(remote.url().map(str::to_string)),
            Err(_) => Ok(None),
        }
    }

    pub fn rev_parse(&self, reference: &str) -> Result<String> {
        let object = self
            .inner
            .revparse_single(reference)
            .with_context(|| format!("reference '{reference}' not found"))?;
        let commit = object
            .peel_to_commit()
            .with_context(|| format!("reference '{reference}' does not point to a commit"))?;
        Ok(commit.id().to_string())
    }

    pub fn list_tags(&self) -> Result<Vec<String>> {
        Ok(self
            .inner
            .tag_names(None)?
            .iter()
            .filter_map(|name| name.map(str::to_string))
            .collect())
    }

    /// Commits reachable from `head_ref` that are not reachable from `base_ref`,
    /// ordered oldest-first. This is the `base..head` range used to model what a
    /// promotion PR would introduce onto the production branch.
    pub fn commits_in_range(
        &self,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<Vec<CommitSummary>> {
        let head = self
            .inner
            .revparse_single(head_ref)
            .with_context(|| format!("reference '{head_ref}' not found"))?
            .peel_to_commit()
            .with_context(|| format!("reference '{head_ref}' does not point to a commit"))?;
        let base = self
            .inner
            .revparse_single(base_ref)
            .with_context(|| format!("reference '{base_ref}' not found"))?
            .peel_to_commit()
            .with_context(|| format!("reference '{base_ref}' does not point to a commit"))?;

        let mut revwalk = self.inner.revwalk().context("failed to create revwalk")?;
        revwalk.push(head.id())?;
        revwalk.hide(base.id())?;

        let mut commits = Vec::new();
        for oid in revwalk {
            let oid = oid?;
            let commit = self.inner.find_commit(oid)?;
            let changed_paths = changed_paths_for_commit(&self.inner, &commit)?;
            commits.push(CommitSummary {
                id: oid.to_string(),
                message: commit.message().unwrap_or_default().trim().to_string(),
                changed_paths,
                author: commit.author().name().unwrap_or("unknown").to_string(),
                raw_author: commit.author().name().unwrap_or("unknown").to_string(),
            });
        }

        commits.reverse();
        Ok(commits)
    }

    pub fn is_ancestor(&self, ancestor_ref: &str, descendant_ref: &str) -> Result<bool> {
        let ancestor = self
            .inner
            .revparse_single(ancestor_ref)
            .with_context(|| format!("reference '{ancestor_ref}' not found"))?
            .peel_to_commit()
            .with_context(|| format!("reference '{ancestor_ref}' does not point to a commit"))?
            .id();
        let descendant = self
            .inner
            .revparse_single(descendant_ref)
            .with_context(|| format!("reference '{descendant_ref}' not found"))?
            .peel_to_commit()
            .with_context(|| format!("reference '{descendant_ref}' does not point to a commit"))?
            .id();
        match self.inner.merge_base(ancestor, descendant) {
            Ok(base) => Ok(base == ancestor),
            Err(_) => Ok(false),
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
