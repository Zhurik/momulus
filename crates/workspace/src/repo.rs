//! Bare clone cache: `data/repos/<owner>/<repo>.git`.

use std::path::{Path, PathBuf};

use momulus_core::{Error, Result};

use crate::diff::DiffIndex;
use crate::git::Git;
use crate::worktree::Worktree;

/// On-disk repository cache.
#[derive(Debug, Clone)]
pub struct RepoCache {
    root: PathBuf,
    git: Git,
}

impl RepoCache {
    pub fn new(root: impl Into<PathBuf>, git: Git) -> RepoCache {
        RepoCache {
            root: root.into(),
            git,
        }
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of a repository's bare clone.
    pub fn bare_path(&self, owner: &str, repo: &str) -> PathBuf {
        self.root.join(owner).join(format!("{repo}.git"))
    }

    /// Clones the repository when missing and fetches the required refs.
    ///
    /// `refspecs` come from the caller because platforms name their PR refs
    /// differently (on GitHub it is `refs/pull/<n>/head`).
    pub async fn sync(
        &self,
        owner: &str,
        repo: &str,
        clone_url: &str,
        refspecs: &[String],
        token: Option<&str>,
    ) -> Result<PathBuf> {
        let bare = self.bare_path(owner, repo);
        if !bare.join("HEAD").exists() {
            if bare.exists() {
                // Leftovers of a failed clone are only in the way.
                std::fs::remove_dir_all(&bare)?;
            }
            if let Some(parent) = bare.parent() {
                std::fs::create_dir_all(parent)?;
            }
            self.git
                .run_with_token(
                    None,
                    [
                        "clone",
                        "--bare",
                        "--quiet",
                        clone_url,
                        &bare.to_string_lossy(),
                    ],
                    token,
                )
                .await?;
        }

        let mut args = vec![
            "fetch".to_string(),
            "--quiet".to_string(),
            "--prune".to_string(),
            "--no-tags".to_string(),
            "origin".to_string(),
        ];
        if refspecs.is_empty() {
            args.push("+refs/heads/*:refs/heads/*".to_string());
        } else {
            args.extend(refspecs.iter().cloned());
        }
        self.git.run_with_token(Some(&bare), args, token).await?;

        Ok(bare)
    }

    /// Whether the cache contains this commit.
    pub async fn has_commit(&self, bare: &Path, sha: &str) -> Result<bool> {
        let result = self
            .git
            .run(Some(bare), ["cat-file", "-e", &format!("{sha}^{{commit}}")])
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(Error::Git(_)) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// PR diff: head changes relative to the merge base with the base branch.
    pub async fn pr_diff(&self, bare: &Path, base: &str, head: &str) -> Result<String> {
        let out = self
            .git
            .run(
                Some(bare),
                [
                    "diff",
                    "--no-color",
                    "--find-renames",
                    "--unified=3",
                    "--merge-base",
                    base,
                    head,
                ],
            )
            .await?;
        Ok(out.stdout)
    }

    /// The parsed PR diff.
    pub async fn pr_diff_index(&self, bare: &Path, base: &str, head: &str) -> Result<DiffIndex> {
        DiffIndex::parse(&self.pr_diff(bare, base, head).await?)
    }

    /// Working copy at the PR head commit; removed when the returned value is dropped.
    pub async fn worktree(&self, bare: &Path, dest: &Path, sha: &str) -> Result<Worktree> {
        Worktree::create(&self.git, bare, dest, sha).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_path_follows_layout() {
        let cache = RepoCache::new("/data/repos", Git::default());
        assert_eq!(
            cache.bare_path("acme", "blog"),
            PathBuf::from("/data/repos/acme/blog.git")
        );
    }
}
