//! Аутентификация GitHub App: JWT приложения → installation tokens.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use momulus_core::{Error, Result};
use octocrab::Octocrab;
use octocrab::models::{AppId, InstallationId};
use tokio::sync::Mutex;

/// За сколько до истечения считаем токен просроченным.
const EXPIRY_SLACK: Duration = Duration::from_secs(120);

/// Клиент, аутентифицированный как приложение, и фабрика клиентов установок.
pub struct AppAuth {
    /// Клиент с JWT приложения: умеет читать установки.
    app: Octocrab,
    /// Кэш installation-токенов для git-операций.
    tokens: Mutex<HashMap<u64, CachedToken>>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    /// Когда токен перестанет быть годным по нашей оценке.
    good_until: DateTime<Utc>,
}

impl std::fmt::Debug for AppAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AppAuth")
    }
}

impl AppAuth {
    /// Собирает клиент приложения из id и PEM-ключа.
    pub fn new(app_id: u64, private_key_pem: &[u8], api_base: &str) -> Result<Arc<AppAuth>> {
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem)
            .map_err(|e| Error::Config(format!("приватный ключ App не читается: {e}")))?;

        // Повторы делаем сами (crate::backoff), поэтому встроенные отключаем:
        // иначе число запросов к API перемножается.
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
            .map_err(|e| Error::Config(format!("не удалось собрать клиент GitHub: {e}")))?;

        Ok(Arc::new(AppAuth {
            app,
            tokens: Mutex::new(HashMap::new()),
        }))
    }

    /// Читает ключ из файла.
    pub fn from_key_path(app_id: u64, path: &Path, api_base: &str) -> Result<Arc<AppAuth>> {
        let pem = std::fs::read(path)
            .map_err(|e| Error::Config(format!("ключ App {}: {e}", path.display())))?;
        AppAuth::new(app_id, &pem, api_base)
    }

    /// Клиент с JWT приложения.
    pub fn app_client(&self) -> &Octocrab {
        &self.app
    }

    /// Клиент установки: им делаем все обычные вызовы API.
    pub fn installation_client(&self, installation_id: u64) -> Result<Octocrab> {
        self.app
            .installation(InstallationId(installation_id))
            .map_err(|e| Error::Internal(format!("клиент установки: {e}")))
    }

    /// Installation-токен для git: кэшируется до истечения.
    ///
    /// Токен нужен только `git fetch`/`push`, поэтому наружу отдаётся строкой,
    /// но нигде не логируется.
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

        // GitHub выдаёт токен на час; обновляем чуть раньше.
        let good_until = Utc::now()
            + chrono::Duration::from_std(Duration::from_secs(3600) - EXPIRY_SLACK)
                .expect("интервал валиден");
        self.tokens.lock().await.insert(
            installation_id,
            CachedToken {
                token: token.clone(),
                good_until,
            },
        );
        Ok(token)
    }

    /// Сбрасывает кэш токена — например, после 401.
    pub async fn forget_token(&self, installation_id: u64) {
        self.tokens.lock().await.remove(&installation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тестовый RSA-ключ (только для юнит-тестов, ничего не защищает).
    const TEST_KEY: &str = include_str!("../tests/data/test-app-key.pem");

    // Клиент octocrab строит tower-сервис, которому нужен реактор tokio.
    #[tokio::test]
    async fn builds_app_client_from_pem() {
        let auth = AppAuth::new(12345, TEST_KEY.as_bytes(), "https://api.github.com").unwrap();
        assert!(auth.installation_client(42).is_ok());
    }

    #[tokio::test]
    async fn rejects_broken_pem() {
        let err = AppAuth::new(1, b"not a key", "").unwrap_err();
        assert!(err.to_string().contains("приватный ключ"), "{err}");
    }

    #[tokio::test]
    async fn reports_missing_key_file() {
        let err = AppAuth::from_key_path(1, Path::new("/definitely/not/here.pem"), "").unwrap_err();
        assert!(err.to_string().contains("ключ App"), "{err}");
    }
}
