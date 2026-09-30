//! Resolves clients and tokens per repository: every installation has its own token.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use momulus_core::{GitAccess, PrRef, Result};
use octocrab::Octocrab;
use tokio::sync::Mutex;

use crate::AppAuth;
use crate::backoff::Backoff;
use crate::error::from_octocrab;
use crate::models::Installation;

/// Where the Publisher gets a client for a particular PR.
#[async_trait]
pub trait ClientProvider: Send + Sync {
    async fn client(&self, pr: &PrRef) -> Result<Octocrab>;
}

/// One client for everything — for a single repository and for tests.
pub struct FixedClient(pub Octocrab);

#[async_trait]
impl ClientProvider for FixedClient {
    async fn client(&self, _pr: &PrRef) -> Result<Octocrab> {
        Ok(self.0.clone())
    }
}

/// An App that can find the installation covering a repository.
pub struct GithubApp {
    auth: Arc<AppAuth>,
    /// owner/repo → installation id.
    installations: Mutex<HashMap<String, u64>>,
    backoff: Backoff,
}

impl GithubApp {
    pub fn new(auth: Arc<AppAuth>) -> Arc<GithubApp> {
        Arc::new(GithubApp {
            auth,
            installations: Mutex::new(HashMap::new()),
            backoff: Backoff::default(),
        })
    }

    /// The installation the repository belongs to; the result is cached.
    pub async fn installation_id(&self, owner: &str, repo: &str) -> Result<u64> {
        let key = format!("{owner}/{repo}");
        if let Some(id) = self.installations.lock().await.get(&key) {
            return Ok(*id);
        }

        let route = format!("/repos/{owner}/{repo}/installation");
        let installation: Installation = self
            .backoff
            .retry(&route, || async {
                self.auth
                    .app_client()
                    .get(&route, None::<&()>)
                    .await
                    .map_err(from_octocrab)
            })
            .await?;

        self.installations.lock().await.insert(key, installation.id);
        Ok(installation.id)
    }

    /// Installation token for git operations on this repository.
    pub async fn token(&self, pr: &PrRef) -> Result<String> {
        let id = self.installation_id(&pr.owner, &pr.repo).await?;
        self.auth.installation_token(id).await
    }
}

#[async_trait]
impl ClientProvider for GithubApp {
    async fn client(&self, pr: &PrRef) -> Result<Octocrab> {
        let id = self.installation_id(&pr.owner, &pr.repo).await?;
        self.auth.installation_client(id)
    }
}

#[async_trait]
impl GitAccess for GithubApp {
    async fn git_token(&self, pr: &PrRef) -> Result<Option<String>> {
        Ok(Some(self.token(pr).await?))
    }

    fn refspecs(&self, pr: &PrRef) -> Vec<String> {
        pr_refspecs(pr)
    }
}

/// What has to be fetched to get a GitHub PR's head and its base branch.
///
/// The PR head lives in `refs/pull/<n>/head` — this works for forks too; the
/// base branch is needed to compute the diff.
pub fn pr_refspecs(pr: &PrRef) -> Vec<String> {
    vec![
        format!("+refs/pull/{n}/head:refs/pull/{n}/head", n = pr.number),
        format!("+refs/heads/{base}:refs/heads/{base}", base = pr.base_ref),
    ]
}

/// Git access through a fixed token — the personal access token setup.
#[derive(Debug, Clone)]
pub struct StaticGitAccess {
    token: Option<String>,
}

impl StaticGitAccess {
    pub fn new(token: impl Into<String>) -> StaticGitAccess {
        StaticGitAccess {
            token: Some(token.into()),
        }
    }

    /// No credentials at all — for local remotes in tests.
    pub fn anonymous() -> StaticGitAccess {
        StaticGitAccess { token: None }
    }
}

#[async_trait]
impl GitAccess for StaticGitAccess {
    async fn git_token(&self, _pr: &PrRef) -> Result<Option<String>> {
        Ok(self.token.clone())
    }

    fn refspecs(&self, pr: &PrRef) -> Vec<String> {
        pr_refspecs(pr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrRef {
        PrRef {
            platform: "github".into(),
            owner: "acme".into(),
            repo: "blog".into(),
            number: 42,
            head_sha: "abc".into(),
            head_ref: "feature".into(),
            base_ref: "main".into(),
            head_repo: "acme/blog".into(),
            clone_url: "https://github.com/acme/blog.git".into(),
        }
    }

    #[tokio::test]
    async fn static_access_hands_out_the_token_and_the_same_refspecs() {
        let access = StaticGitAccess::new("ghp_token");
        assert_eq!(
            access.git_token(&pr()).await.unwrap().as_deref(),
            Some("ghp_token")
        );
        assert_eq!(
            access.refspecs(&pr()),
            vec![
                "+refs/pull/42/head:refs/pull/42/head".to_string(),
                "+refs/heads/main:refs/heads/main".to_string(),
            ]
        );
        assert_eq!(access.base_rev(&pr()), "refs/heads/main");
    }

    #[tokio::test]
    async fn anonymous_access_has_no_token() {
        assert!(
            StaticGitAccess::anonymous()
                .git_token(&pr())
                .await
                .unwrap()
                .is_none()
        );
    }
}
