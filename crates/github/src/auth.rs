//! GitHub App authentication: the app JWT → installation tokens.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use momulus_core::{Error, Result};
use octocrab::Octocrab;
use octocrab::models::{AppId, InstallationId};
use tokio::sync::Mutex;

/// How long before expiry we treat a token as stale.
const EXPIRY_SLACK: Duration = Duration::from_secs(120);

/// A client authenticated as the App, and a factory of installation clients.
pub struct AppAuth {
    /// Client holding the app JWT: it can read installations.
    app: Octocrab,
    /// Cache of installation tokens used for git operations.
    tokens: Mutex<HashMap<u64, CachedToken>>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    /// When we consider the token no longer good.
    good_until: DateTime<Utc>,
}

impl std::fmt::Debug for AppAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AppAuth")
    }
}

impl AppAuth {
    /// Builds the app client from an id and a PEM key.
    pub fn new(app_id: u64, private_key_pem: &[u8], api_base: &str) -> Result<Arc<AppAuth>> {
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem)
            .map_err(|e| Error::Config(format!("the App private key cannot be read: {e}")))?;

        // We do our own retries (crate::backoff), so the built-in ones are off:
        // otherwise the number of API calls multiplies.
        let mut builder = Octocrab::builder()
            .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
            .app(AppId(app_id), key);
        if !api_base.is_empty() {
            builder = builder
                .base_uri(api_base)
                .map_err(|e| Error::Config(format!("github.api_base: {e}")))?;
        }
        let app = builder
            .build()
            .map_err(|e| Error::Config(format!("could not build the GitHub client: {e}")))?;

        Ok(Arc::new(AppAuth {
            app,
            tokens: Mutex::new(HashMap::new()),
        }))
    }

    /// Reads the key from a file.
    pub fn from_key_path(app_id: u64, path: &Path, api_base: &str) -> Result<Arc<AppAuth>> {
        let pem = std::fs::read(path)
            .map_err(|e| Error::Config(format!("App key {}: {e}", path.display())))?;
        AppAuth::new(app_id, &pem, api_base)
    }

    /// The client holding the app JWT.
    pub fn app_client(&self) -> &Octocrab {
        &self.app
    }

    /// Installation client: every ordinary API call goes through it.
    pub fn installation_client(&self, installation_id: u64) -> Result<Octocrab> {
        self.app
            .installation(InstallationId(installation_id))
            .map_err(|e| Error::Internal(format!("installation client: {e}")))
    }

    /// Installation token for git: cached until it expires.
    ///
    /// Only `git fetch`/`push` need it, so it is handed out as a plain string,
    /// but it is never logged.
    pub async fn installation_token(&self, installation_id: u64) -> Result<String> {
        if let Some(cached) = self.tokens.lock().await.get(&installation_id)
            && cached.good_until > Utc::now()
        {
            return Ok(cached.token.clone());
        }

        let (_client, token) = self
            .app
            .installation_and_token(InstallationId(installation_id))
            .await
            .map_err(crate::error::from_octocrab)?;
        let token = secrecy::ExposeSecret::expose_secret(&token).to_string();

        // GitHub issues the token for an hour; we refresh a little earlier.
        let good_until = Utc::now()
            + chrono::Duration::from_std(Duration::from_secs(3600) - EXPIRY_SLACK)
                .expect("the interval is valid");
        self.tokens.lock().await.insert(
            installation_id,
            CachedToken {
                token: token.clone(),
                good_until,
            },
        );
        Ok(token)
    }

    /// Drops the cached token — after a 401, for instance.
    pub async fn forget_token(&self, installation_id: u64) {
        self.tokens.lock().await.remove(&installation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testkey::test_app_key_pem;

    // The octocrab client builds a tower service, which needs a tokio reactor.
    #[tokio::test]
    async fn builds_app_client_from_pem() {
        let auth = AppAuth::new(
            12345,
            test_app_key_pem().as_bytes(),
            "https://api.github.com",
        )
        .unwrap();
        assert!(auth.installation_client(42).is_ok());
    }

    #[tokio::test]
    async fn rejects_broken_pem() {
        let err = AppAuth::new(1, b"not a key", "").unwrap_err();
        assert!(err.to_string().contains("private key"), "{err}");
    }

    #[tokio::test]
    async fn reports_missing_key_file() {
        let err = AppAuth::from_key_path(1, Path::new("/definitely/not/here.pem"), "").unwrap_err();
        assert!(err.to_string().contains("App key"), "{err}");
    }
}
