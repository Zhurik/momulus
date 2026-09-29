//! Конфигурация сервиса: TOML-файл плюс секреты из окружения.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Имена переменных окружения с секретами.
pub const ENV_GITHUB_APP_ID: &str = "GITHUB_APP_ID";
pub const ENV_GITHUB_APP_PRIVATE_KEY_PATH: &str = "GITHUB_APP_PRIVATE_KEY_PATH";
pub const ENV_LLM_API_KEY: &str = "LLM_API_KEY";
pub const ENV_LLM_BASE_URL: &str = "LLM_BASE_URL";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Как часто опрашивать платформу.
    #[serde(with = "humantime_serde", default = "default_poll_interval")]
    pub poll_interval: Duration,

    /// Сколько джоб выполняем одновременно.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,

    /// Логины, которым разрешено запускать команды.
    pub allowed_users: Vec<String>,

    /// Белый список репозиториев "owner/repo"; пусто — все установки App.
    #[serde(default)]
    pub repos: Vec<String>,

    /// Куда складываем БД, кэш репозиториев, worktree и логи.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// Каталог со скиллами.
    #[serde(default = "default_skills_dir")]
    pub skills_dir: PathBuf,

    /// Сколько ждать завершения текущих джоб при остановке.
    #[serde(with = "humantime_serde", default = "default_shutdown_timeout")]
    pub shutdown_timeout: Duration,

    #[serde(default)]
    pub llm: LlmConfig,

    #[serde(default)]
    pub docker: DockerConfig,

    #[serde(default)]
    pub github: GithubConfig,

    #[serde(default)]
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    /// Провайдер, который передаётся pi (`--provider`).
    #[serde(default = "default_provider")]
    pub default_provider: String,
    /// Модель по умолчанию; скилл может переопределить.
    #[serde(default = "default_model")]
    pub default_model: String,
    /// Имя переменной окружения, в которой pi ждёт ключ провайдера.
    /// Пусто — выводим из имени провайдера (cloudru -> CLOUDRU_API_KEY).
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Протокол провайдера для pi: нужен, когда провайдер не встроен в pi
    /// и описывается через models.json. Пусто — провайдер встроенный.
    #[serde(default)]
    pub api: Option<ProviderApi>,
}

/// Протоколы, которые умеет pi (значение поля `api` в models.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderApi {
    OpenaiCompletions,
    OpenaiResponses,
    AnthropicMessages,
    GoogleGenerativeAi,
    BedrockConverse,
    AzureOpenaiResponses,
}

impl ProviderApi {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderApi::OpenaiCompletions => "openai-completions",
            ProviderApi::OpenaiResponses => "openai-responses",
            ProviderApi::AnthropicMessages => "anthropic-messages",
            ProviderApi::GoogleGenerativeAi => "google-generative-ai",
            ProviderApi::BedrockConverse => "bedrock-converse",
            ProviderApi::AzureOpenaiResponses => "azure-openai-responses",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerConfig {
    /// Образ с pi.
    #[serde(default = "default_runner_image")]
    pub runner_image: String,
    /// Ограничение CPU в ядрах.
    #[serde(default = "default_cpu")]
    pub cpu: f64,
    /// Ограничение памяти в мегабайтах.
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u64,
    /// Адрес демона; пусто — как в окружении (DOCKER_HOST или сокет по умолчанию).
    #[serde(default)]
    pub host: Option<String>,
    /// Пользователь внутри контейнера ("1000:1000"); пусто — как в образе.
    /// Нужен, когда uid сервиса на хосте не совпадает с uid в образе:
    /// иначе pi не сможет писать в смонтированный /out.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubConfig {
    /// База REST API (переопределяется для GitHub Enterprise и тестов).
    #[serde(default = "default_github_api")]
    pub api_base: String,
    /// Имя автора коммитов бота.
    #[serde(default = "default_bot_name")]
    pub bot_name: String,
    /// Почта автора коммитов бота.
    #[serde(default = "default_bot_email")]
    pub bot_email: String,
}

/// Ограничения на размер входа — защита от гигантских PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_max_diff_bytes")]
    pub max_diff_bytes: u64,
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_max_changed_files")]
    pub max_changed_files: usize,
}

fn default_poll_interval() -> Duration {
    Duration::from_secs(45)
}
fn default_concurrency() -> usize {
    1
}
fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}
fn default_skills_dir() -> PathBuf {
    PathBuf::from("skills")
}
fn default_shutdown_timeout() -> Duration {
    Duration::from_secs(300)
}
fn default_provider() -> String {
    "cloudru".to_string()
}
fn default_model() -> String {
    "zai-org/GLM-5.1".to_string()
}
fn default_api() -> Option<ProviderApi> {
    Some(ProviderApi::OpenaiCompletions)
}
fn default_runner_image() -> String {
    "llm-bot-runner:latest".to_string()
}
fn default_cpu() -> f64 {
    2.0
}
fn default_memory_mb() -> u64 {
    2048
}
fn default_github_api() -> String {
    "https://api.github.com".to_string()
}
fn default_bot_name() -> String {
    "llm-bot".to_string()
}
fn default_bot_email() -> String {
    "llm-bot@users.noreply.github.com".to_string()
}
fn default_max_diff_bytes() -> u64 {
    400_000
}
fn default_max_file_bytes() -> u64 {
    200_000
}
fn default_max_changed_files() -> usize {
    100
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            default_provider: default_provider(),
            default_model: default_model(),
            api_key_env: None,
            api: default_api(),
        }
    }
}

impl Default for DockerConfig {
    fn default() -> Self {
        DockerConfig {
            runner_image: default_runner_image(),
            cpu: default_cpu(),
            memory_mb: default_memory_mb(),
            host: None,
            user: None,
        }
    }
}

impl Default for GithubConfig {
    fn default() -> Self {
        GithubConfig {
            api_base: default_github_api(),
            bot_name: default_bot_name(),
            bot_email: default_bot_email(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_diff_bytes: default_max_diff_bytes(),
            max_file_bytes: default_max_file_bytes(),
            max_changed_files: default_max_changed_files(),
        }
    }
}

impl LlmConfig {
    /// Имя переменной окружения с ключом провайдера.
    pub fn api_key_env(&self) -> String {
        if let Some(name) = &self.api_key_env {
            return name.clone();
        }
        provider_key_env(&self.default_provider)
    }

    /// Нужно ли описывать провайдера через models.json.
    pub fn needs_models_json(&self) -> bool {
        self.api.is_some()
    }

    /// Проверяет, что провайдера хватит для запуска pi.
    pub fn check_ready(&self, base_url: Option<&str>) -> Result<()> {
        if self.needs_models_json() && base_url.is_none_or(str::is_empty) {
            return Err(Error::Config(format!(
                "провайдер {:?} описан через llm.api = {:?}, значит нужен {ENV_LLM_BASE_URL}",
                self.default_provider,
                self.api.map(|a| a.as_str()).unwrap_or_default()
            )));
        }
        Ok(())
    }
}

/// Переменная окружения, в которой pi ищет ключ конкретного провайдера.
pub fn provider_key_env(provider: &str) -> String {
    match provider {
        "google" | "gemini" => "GEMINI_API_KEY".to_string(),
        "azure-openai" => "AZURE_OPENAI_API_KEY".to_string(),
        other => format!(
            "{}_API_KEY",
            other.to_ascii_uppercase().replace(['-', '.', ' '], "_")
        ),
    }
}

impl Config {
    /// Читает и валидирует конфиг.
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?;
        let config = Config::from_toml(&text)?;
        Ok(config)
    }

    pub fn from_toml(text: &str) -> Result<Config> {
        let config: Config =
            toml::from_str(text).map_err(|e| Error::Config(format!("invalid config: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.concurrency == 0 {
            return Err(Error::Config("concurrency must be at least 1".into()));
        }
        if self.allowed_users.is_empty() {
            return Err(Error::Config(
                "allowed_users must list at least one login".into(),
            ));
        }
        if self.poll_interval < Duration::from_secs(5) {
            return Err(Error::Config("poll_interval must be at least 5s".into()));
        }
        if self.docker.cpu <= 0.0 {
            return Err(Error::Config("docker.cpu must be positive".into()));
        }
        if self.docker.memory_mb < 128 {
            return Err(Error::Config(
                "docker.memory_mb must be at least 128".into(),
            ));
        }
        for repo in &self.repos {
            if repo.split('/').filter(|p| !p.is_empty()).count() != 2 {
                return Err(Error::Config(format!(
                    "repos entry must look like owner/repo, got {repo:?}"
                )));
            }
        }
        Ok(())
    }

    /// Разрешён ли пользователь запускать команды (сравнение регистронезависимое).
    pub fn is_allowed_user(&self, login: &str) -> bool {
        self.allowed_users
            .iter()
            .any(|u| u.eq_ignore_ascii_case(login))
    }

    /// Разрешён ли репозиторий; пустой список — разрешены все.
    pub fn is_allowed_repo(&self, full_name: &str) -> bool {
        self.repos.is_empty() || self.repos.iter().any(|r| r.eq_ignore_ascii_case(full_name))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("llm-bot.sqlite")
    }

    pub fn repos_dir(&self) -> PathBuf {
        self.data_dir.join("repos")
    }

    pub fn work_dir(&self) -> PathBuf {
        self.data_dir.join("work")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
}

/// Секреты. Никогда не логируются и не попадают в Debug целиком.
#[derive(Clone, Default)]
pub struct Secrets {
    pub github_app_id: Option<u64>,
    pub github_private_key_path: Option<PathBuf>,
    pub llm_api_key: Option<String>,
    pub llm_base_url: Option<String>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("github_app_id", &self.github_app_id)
            .field("github_private_key_path", &self.github_private_key_path)
            .field("llm_api_key", &self.llm_api_key.as_ref().map(|_| "***"))
            .field("llm_base_url", &self.llm_base_url)
            .finish()
    }
}

impl Secrets {
    /// Читает секреты из окружения.
    pub fn from_env() -> Result<Secrets> {
        let github_app_id = match std::env::var(ENV_GITHUB_APP_ID) {
            Ok(value) => Some(value.trim().parse::<u64>().map_err(|e| {
                Error::Config(format!("{ENV_GITHUB_APP_ID} must be a number: {e}"))
            })?),
            Err(_) => None,
        };
        Ok(Secrets {
            github_app_id,
            github_private_key_path: std::env::var(ENV_GITHUB_APP_PRIVATE_KEY_PATH)
                .ok()
                .map(PathBuf::from),
            llm_api_key: std::env::var(ENV_LLM_API_KEY)
                .ok()
                .filter(|v| !v.is_empty()),
            llm_base_url: std::env::var(ENV_LLM_BASE_URL)
                .ok()
                .filter(|v| !v.is_empty()),
        })
    }

    /// Секреты, нужные для работы с GitHub.
    pub fn require_github(&self) -> Result<(u64, PathBuf)> {
        let app_id = self
            .github_app_id
            .ok_or_else(|| Error::Config(format!("{ENV_GITHUB_APP_ID} is not set")))?;
        let key = self.github_private_key_path.clone().ok_or_else(|| {
            Error::Config(format!("{ENV_GITHUB_APP_PRIVATE_KEY_PATH} is not set"))
        })?;
        Ok((app_id, key))
    }

    /// Ключ LLM-провайдера.
    pub fn require_llm_api_key(&self) -> Result<&str> {
        self.llm_api_key
            .as_deref()
            .ok_or_else(|| Error::Config(format!("{ENV_LLM_API_KEY} is not set")))
    }

    /// Добавляет все известные секреты в редактор логов.
    pub fn redactor(&self) -> crate::redact::Redactor {
        let mut redactor = crate::redact::Redactor::new();
        if let Some(key) = &self.llm_api_key {
            redactor.add(key.clone());
        }
        redactor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"allowed_users = ["zhurik"]"#;

    #[test]
    fn minimal_config_gets_defaults() {
        let config = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(config.poll_interval, Duration::from_secs(45));
        assert_eq!(config.concurrency, 1);
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.docker.runner_image, "llm-bot-runner:latest");
        assert_eq!(config.limits.max_changed_files, 100);
        assert_eq!(config.github.api_base, "https://api.github.com");
    }

    #[test]
    fn parses_durations() {
        let config = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            poll_interval = "30s"
            shutdown_timeout = "2m"
        "#,
        )
        .unwrap();
        assert_eq!(config.poll_interval, Duration::from_secs(30));
        assert_eq!(config.shutdown_timeout, Duration::from_secs(120));
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            pol_interval = "30s"
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("pol_interval"), "{err}");
    }

    #[test]
    fn rejects_unknown_nested_fields() {
        let err = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            [docker]
            memory = 512
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("memory"), "{err}");
    }

    #[test]
    fn rejects_empty_allowed_users() {
        let err = Config::from_toml("allowed_users = []").unwrap_err();
        assert!(err.to_string().contains("allowed_users"), "{err}");
    }

    #[test]
    fn rejects_zero_concurrency() {
        let err = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            concurrency = 0
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("concurrency"), "{err}");
    }

    #[test]
    fn rejects_bad_repo_entry() {
        let err = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            repos = ["acme"]
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("owner/repo"), "{err}");
    }

    #[test]
    fn allowlists_are_case_insensitive() {
        let config = Config::from_toml(
            r#"
            allowed_users = ["Zhurik"]
            repos = ["Acme/Blog"]
        "#,
        )
        .unwrap();
        assert!(config.is_allowed_user("zhurik"));
        assert!(!config.is_allowed_user("someone"));
        assert!(config.is_allowed_repo("acme/blog"));
        assert!(!config.is_allowed_repo("other/blog"));
    }

    #[test]
    fn empty_repo_list_allows_everything() {
        let config = Config::from_toml(MINIMAL).unwrap();
        assert!(config.is_allowed_repo("anyone/anything"));
    }

    #[test]
    fn data_paths_are_derived() {
        let config = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            data_dir = "/srv/llm-bot/data"
        "#,
        )
        .unwrap();
        assert_eq!(
            config.db_path(),
            PathBuf::from("/srv/llm-bot/data/llm-bot.sqlite")
        );
        assert_eq!(config.repos_dir(), PathBuf::from("/srv/llm-bot/data/repos"));
        assert_eq!(config.work_dir(), PathBuf::from("/srv/llm-bot/data/work"));
    }

    #[test]
    fn example_config_from_repo_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.toml");
        let text = std::fs::read_to_string(&path).expect("config.example.toml exists");
        Config::from_toml(&text).expect("config.example.toml parses");
    }

    #[test]
    fn default_provider_is_cloudru_glm() {
        let config = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(config.llm.default_provider, "cloudru");
        assert_eq!(config.llm.default_model, "zai-org/GLM-5.1");
        assert_eq!(config.llm.api, Some(ProviderApi::OpenaiCompletions));
        assert_eq!(config.llm.api_key_env(), "CLOUDRU_API_KEY");
    }

    #[test]
    fn custom_provider_requires_base_url() {
        let config = Config::from_toml(MINIMAL).unwrap();
        let err = config.llm.check_ready(None).unwrap_err();
        assert!(err.to_string().contains(ENV_LLM_BASE_URL), "{err}");
        assert!(err.to_string().contains("openai-completions"), "{err}");
        config
            .llm
            .check_ready(Some("https://foundation-models.api.cloud.ru/v1"))
            .unwrap();
    }

    #[test]
    fn builtin_provider_needs_no_base_url() {
        let config = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            [llm]
            default_provider = "anthropic"
            default_model = "claude-sonnet-5"
        "#,
        )
        .unwrap();
        assert_eq!(config.llm.api, None);
        assert!(!config.llm.needs_models_json());
        config.llm.check_ready(None).unwrap();
    }

    #[test]
    fn unknown_provider_api_is_rejected() {
        let err = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            [llm]
            api = "grpc-magic"
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("openai-completions"), "{err}");
    }

    #[test]
    fn provider_key_env_names() {
        assert_eq!(provider_key_env("anthropic"), "ANTHROPIC_API_KEY");
        assert_eq!(provider_key_env("openai"), "OPENAI_API_KEY");
        assert_eq!(provider_key_env("google"), "GEMINI_API_KEY");
        assert_eq!(provider_key_env("openrouter"), "OPENROUTER_API_KEY");
        assert_eq!(provider_key_env("ant-ling"), "ANT_LING_API_KEY");
    }

    #[test]
    fn api_key_env_can_be_overridden() {
        let config = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            [llm]
            default_provider = "my-proxy"
            api_key_env = "PROXY_TOKEN"
        "#,
        )
        .unwrap();
        assert_eq!(config.llm.api_key_env(), "PROXY_TOKEN");
    }

    #[test]
    fn secrets_debug_hides_the_key() {
        let secrets = Secrets {
            github_app_id: Some(1),
            github_private_key_path: Some(PathBuf::from("/k.pem")),
            llm_api_key: Some("sk-ant-secret-value".into()),
            llm_base_url: None,
        };
        let text = format!("{secrets:?}");
        assert!(!text.contains("sk-ant-secret-value"), "{text}");
        assert!(text.contains("***"), "{text}");
    }

    #[test]
    fn missing_secrets_are_reported_by_name() {
        let secrets = Secrets::default();
        let err = secrets.require_github().unwrap_err();
        assert!(err.to_string().contains(ENV_GITHUB_APP_ID), "{err}");
        let err = secrets.require_llm_api_key().unwrap_err();
        assert!(err.to_string().contains(ENV_LLM_API_KEY), "{err}");
    }
}
