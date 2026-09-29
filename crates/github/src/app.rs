//! Резолвер клиентов и токенов по репозиторию: у каждой установки свой токен.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use llm_bot_core::{GitAccess, PrRef, Result};
use octocrab::Octocrab;
use tokio::sync::Mutex;

use crate::AppAuth;
use crate::backoff::Backoff;
use crate::error::from_octocrab;
use crate::models::Installation;

/// Откуда Publisher берёт клиент для конкретного PR.
#[async_trait]
pub trait ClientProvider: Send + Sync {
    async fn client(&self, pr: &PrRef) -> Result<Octocrab>;
}

/// Один и тот же клиент на всё — для одного репозитория и для тестов.
pub struct FixedClient(pub Octocrab);

#[async_trait]
impl ClientProvider for FixedClient {
    async fn client(&self, _pr: &PrRef) -> Result<Octocrab> {
        Ok(self.0.clone())
    }
}

/// Приложение, которое умеет найти установку по репозиторию.
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

    /// Установка, в которую входит репозиторий; результат кэшируется.
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

    /// Токен установки для git-операций с этим репозиторием.
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
        // head PR лежит в refs/pull/<n>/head — работает и для форков;
        // базовая ветка нужна, чтобы посчитать diff.
        vec![
            format!("+refs/pull/{n}/head:refs/pull/{n}/head", n = pr.number),
            format!("+refs/heads/{base}:refs/heads/{base}", base = pr.base_ref),
        ]
    }
}
