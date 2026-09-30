//! Service configuration: a TOML file plus secrets from the environment.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Names of the environment variables holding secrets.
pub const ENV_GITHUB_TOKEN: &str = "GITHUB_TOKEN";
pub const ENV_GITHUB_APP_ID: &str = "GITHUB_APP_ID";
pub const ENV_GITHUB_APP_PRIVATE_KEY_PATH: &str = "GITHUB_APP_PRIVATE_KEY_PATH";
pub const ENV_LLM_API_KEY: &str = "LLM_API_KEY";
pub const ENV_LLM_BASE_URL: &str = "LLM_BASE_URL";

/// Environment variables that override the paths from the config file.
///
/// Handy under docker-compose, where the paths have to match the host ones and
/// keeping them in two places invites a mismatch.
pub const ENV_DATA_DIR: &str = "MOMULUS_DATA_DIR";
pub const ENV_SKILLS_DIR: &str = "MOMULUS_SKILLS_DIR";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// How often to poll the platform.
    #[serde(with = "humantime_serde", default = "default_poll_interval")]
    pub poll_interval: Duration,

    /// How many jobs run at the same time.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,

    /// Logins allowed to issue commands.
    pub allowed_users: Vec<String>,

    /// Repository allowlist as "owner/repo"; empty means all App installations.
    #[serde(default)]
    pub repos: Vec<String>,

    /// Where the database, repo cache, worktrees and logs live.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// Skills directory.
    #[serde(default = "default_skills_dir")]
    pub skills_dir: PathBuf,

    /// How long to wait for running jobs on shutdown.
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
    /// Provider passed to pi (`--provider`).
    #[serde(default = "default_provider")]
    pub default_provider: String,
    /// Default model; a skill may override it.
    #[serde(default = "default_model")]
    pub default_model: String,
    /// Name of the environment variable pi reads the provider key from.
    /// Empty means it is derived from the provider name (openai -> OPENAI_API_KEY).
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Provider protocol for pi: needed when the provider is not built into pi
    /// and has to be described in models.json. Empty means a built-in provider.
    #[serde(default)]
    pub api: Option<ProviderApi>,
}

/// Protocols pi supports (the `api` field in models.json).
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
    /// Image containing pi.
    #[serde(default = "default_runner_image")]
    pub runner_image: String,
    /// CPU limit in cores.
    #[serde(default = "default_cpu")]
    pub cpu: f64,
    /// Memory limit in megabytes.
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u64,
    /// Daemon address; empty means the environment default (DOCKER_HOST or the default socket).
    #[serde(default)]
    pub host: Option<String>,
    /// User inside the container ("1000:1000"); empty means the image default.
    /// Needed when the service uid on the host differs from the uid in the image:
    /// otherwise pi cannot write to the mounted /out.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubConfig {
    /// REST API base (overridden for GitHub Enterprise and for tests).
    #[serde(default = "default_github_api")]
    pub api_base: String,
    /// Author name for the bot's commits.
    #[serde(default = "default_bot_name")]
    pub bot_name: String,
    /// Author email for the bot's commits.
    #[serde(default = "default_bot_email")]
    pub bot_email: String,
}

/// Input size limits — protection against huge PRs.
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
    "openai".to_string()
}
fn default_model() -> String {
    "gpt-5.1".to_string()
}
fn default_runner_image() -> String {
    "momulus-runner:latest".to_string()
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
    "momulus".to_string()
}
fn default_bot_email() -> String {
    "momulus@users.noreply.github.com".to_string()
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
            api: None,
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
    /// Name of the environment variable holding the provider key.
    pub fn api_key_env(&self) -> String {
        if let Some(name) = &self.api_key_env {
            return name.clone();
        }
        provider_key_env(&self.default_provider)
    }

    /// Whether the provider has to be described in models.json.
    pub fn needs_models_json(&self) -> bool {
        self.api.is_some()
    }

    /// Checks that the provider description is complete enough to run pi.
    pub fn check_ready(&self, base_url: Option<&str>) -> Result<()> {
        if self.needs_models_json() && base_url.is_none_or(str::is_empty) {
            return Err(Error::Config(format!(
                "provider {:?} is described by llm.api = {:?}, so {ENV_LLM_BASE_URL} is required",
                self.default_provider,
                self.api.map(|a| a.as_str()).unwrap_or_default()
            )));
        }
        Ok(())
    }
}

/// A non-empty, trimmed value of an environment variable.
fn env_value(name: &str) -> Option<String> {
    normalize_env(std::env::var(name).ok())
}

/// Trims a raw environment value and treats an empty one as unset.
///
/// Empty means unset because docker-compose forwards every variable declared on
/// the service, so the ones a given setup does not use arrive as empty strings.
fn normalize_env(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Parses the App id out of a raw environment value.
fn parse_app_id(raw: Option<String>) -> Result<Option<u64>> {
    match normalize_env(raw) {
        Some(value) => Ok(Some(value.parse::<u64>().map_err(|e| {
            Error::Config(format!("{ENV_GITHUB_APP_ID} must be a number: {e}"))
        })?)),
        None => Ok(None),
    }
}

/// A non-empty path from an environment variable.
fn env_path(name: &str) -> Option<PathBuf> {
    env_value(name).map(PathBuf::from)
}

/// The environment variable pi looks for a given provider's key in.
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
    /// Reads and validates the config.
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?;
        let mut config = Config::from_toml(&text)?;
        config.apply_env_overrides();
        Ok(config)
    }

    pub fn from_toml(text: &str) -> Result<Config> {
        let config: Config =
            toml::from_str(text).map_err(|e| Error::Config(format!("invalid config: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Lets the environment override the two paths that have to match the host
    /// under docker-compose, so they need not be repeated in the config file.
    pub fn apply_env_overrides(&mut self) {
        if let Some(dir) = env_path(ENV_DATA_DIR) {
            self.data_dir = dir;
        }
        if let Some(dir) = env_path(ENV_SKILLS_DIR) {
            self.skills_dir = dir;
        }
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

    /// Whether the user may issue commands (case-insensitive comparison).
    pub fn is_allowed_user(&self, login: &str) -> bool {
        self.allowed_users
            .iter()
            .any(|u| u.eq_ignore_ascii_case(login))
    }

    /// Whether the repository is allowed; an empty list allows everything.
    pub fn is_allowed_repo(&self, full_name: &str) -> bool {
        self.repos.is_empty() || self.repos.iter().any(|r| r.eq_ignore_ascii_case(full_name))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("momulus.sqlite")
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

/// The way the service authenticates against GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubAuth {
    /// A personal access token: one variable, no installation to manage.
    Token(String),
    /// A GitHub App: its own identity and short-lived installation tokens.
    App { app_id: u64, key_path: PathBuf },
}

impl GithubAuth {
    /// A GitHub App discovers its repositories on its own; a token cannot, so
    /// the repository allowlist becomes mandatory.
    pub fn needs_repo_allowlist(&self) -> bool {
        matches!(self, GithubAuth::Token(_))
    }
}

/// Secrets. Never logged and never fully shown in Debug output.
#[derive(Clone, Default)]
pub struct Secrets {
    /// Personal access token — the simple way to authenticate.
    pub github_token: Option<String>,
    pub github_app_id: Option<u64>,
    pub github_private_key_path: Option<PathBuf>,
    pub llm_api_key: Option<String>,
    pub llm_base_url: Option<String>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("github_token", &self.github_token.as_ref().map(|_| "***"))
            .field("github_app_id", &self.github_app_id)
            .field("github_private_key_path", &self.github_private_key_path)
            .field("llm_api_key", &self.llm_api_key.as_ref().map(|_| "***"))
            .field("llm_base_url", &self.llm_base_url)
            .finish()
    }
}

impl Secrets {
    /// Reads the secrets from the environment.
    pub fn from_env() -> Result<Secrets> {
        let github_app_id = parse_app_id(std::env::var(ENV_GITHUB_APP_ID).ok())?;
        Ok(Secrets {
            github_token: env_value(ENV_GITHUB_TOKEN),
            github_app_id,
            github_private_key_path: env_value(ENV_GITHUB_APP_PRIVATE_KEY_PATH).map(PathBuf::from),
            llm_api_key: env_value(ENV_LLM_API_KEY),
            llm_base_url: env_value(ENV_LLM_BASE_URL),
        })
    }

    /// How we are going to authenticate against GitHub.
    ///
    /// A token wins when both are present: it is the simpler setup, and having
    /// set it explicitly is a clear statement of intent.
    pub fn github_auth(&self) -> Result<GithubAuth> {
        if let Some(token) = &self.github_token {
            return Ok(GithubAuth::Token(token.clone()));
        }
        match (self.github_app_id, self.github_private_key_path.clone()) {
            (Some(app_id), Some(key_path)) => Ok(GithubAuth::App { app_id, key_path }),
            (Some(_), None) => Err(Error::Config(format!(
                "{ENV_GITHUB_APP_ID} is set but {ENV_GITHUB_APP_PRIVATE_KEY_PATH} is not"
            ))),
            (None, Some(_)) => Err(Error::Config(format!(
                "{ENV_GITHUB_APP_PRIVATE_KEY_PATH} is set but {ENV_GITHUB_APP_ID} is not"
            ))),
            (None, None) => Err(Error::Config(format!(
                "no GitHub credentials: set {ENV_GITHUB_TOKEN} to a personal access token, \
                 or {ENV_GITHUB_APP_ID} together with {ENV_GITHUB_APP_PRIVATE_KEY_PATH} \
                 to use a GitHub App"
            ))),
        }
    }

    /// LLM provider key.
    pub fn require_llm_api_key(&self) -> Result<&str> {
        self.llm_api_key
            .as_deref()
            .ok_or_else(|| Error::Config(format!("{ENV_LLM_API_KEY} is not set")))
    }

    /// Adds every known secret to the log redactor.
    pub fn redactor(&self) -> crate::redact::Redactor {
        let mut redactor = crate::redact::Redactor::new();
        if let Some(key) = &self.llm_api_key {
            redactor.add(key.clone());
        }
        if let Some(token) = &self.github_token {
            redactor.add(token.clone());
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
        assert_eq!(config.docker.runner_image, "momulus-runner:latest");
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
            data_dir = "/srv/momulus/data"
        "#,
        )
        .unwrap();
        assert_eq!(
            config.db_path(),
            PathBuf::from("/srv/momulus/data/momulus.sqlite")
        );
        assert_eq!(config.repos_dir(), PathBuf::from("/srv/momulus/data/repos"));
        assert_eq!(config.work_dir(), PathBuf::from("/srv/momulus/data/work"));
    }

    #[test]
    fn example_config_from_repo_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.toml");
        let text = std::fs::read_to_string(&path).expect("config.example.toml exists");
        Config::from_toml(&text).expect("config.example.toml parses");
    }

    #[test]
    fn default_provider_is_a_builtin_one() {
        let config = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(config.llm.default_provider, "openai");
        assert_eq!(config.llm.default_model, "gpt-5.1");
        assert_eq!(config.llm.api, None);
        assert_eq!(config.llm.api_key_env(), "OPENAI_API_KEY");
    }

    #[test]
    fn custom_provider_requires_base_url() {
        let config = Config::from_toml(
            r#"
            allowed_users = ["zhurik"]
            [llm]
            default_provider = "my-gateway"
            api = "openai-completions"
        "#,
        )
        .unwrap();
        let err = config.llm.check_ready(None).unwrap_err();
        assert!(err.to_string().contains(ENV_LLM_BASE_URL), "{err}");
        assert!(err.to_string().contains("openai-completions"), "{err}");
        config
            .llm
            .check_ready(Some("https://openrouter.ai/api/v1"))
            .unwrap();
    }

    #[test]
    fn builtin_provider_needs_no_base_url() {
        let config = Config::from_toml(MINIMAL).unwrap();
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
    fn empty_env_values_count_as_unset() {
        // docker-compose passes unused variables through as empty strings.
        assert_eq!(normalize_env(Some(String::new())), None);
        assert_eq!(normalize_env(Some("   ".into())), None);
        assert_eq!(normalize_env(None), None);
        assert_eq!(
            normalize_env(Some(" ghp_token ".into())),
            Some("ghp_token".to_string())
        );
    }

    #[test]
    fn an_empty_app_id_is_not_a_parse_error() {
        // This is exactly what docker-compose sends when only a token is used.
        assert_eq!(parse_app_id(Some(String::new())).unwrap(), None);
        assert_eq!(parse_app_id(None).unwrap(), None);
        assert_eq!(parse_app_id(Some(" 123456 ".into())).unwrap(), Some(123456));

        let err = parse_app_id(Some("not-a-number".into())).unwrap_err();
        assert!(err.to_string().contains(ENV_GITHUB_APP_ID), "{err}");
    }

    #[test]
    fn a_token_is_preferred_over_an_app() {
        let secrets = Secrets {
            github_token: Some("ghp_token".into()),
            github_app_id: Some(7),
            github_private_key_path: Some(PathBuf::from("/k.pem")),
            ..Secrets::default()
        };
        assert_eq!(
            secrets.github_auth().unwrap(),
            GithubAuth::Token("ghp_token".into())
        );
        assert!(secrets.github_auth().unwrap().needs_repo_allowlist());
    }

    #[test]
    fn an_app_is_used_when_there_is_no_token() {
        let secrets = Secrets {
            github_app_id: Some(7),
            github_private_key_path: Some(PathBuf::from("/k.pem")),
            ..Secrets::default()
        };
        let auth = secrets.github_auth().unwrap();
        assert_eq!(
            auth,
            GithubAuth::App {
                app_id: 7,
                key_path: PathBuf::from("/k.pem")
            }
        );
        assert!(
            !auth.needs_repo_allowlist(),
            "an App finds its repos itself"
        );
    }

    #[test]
    fn half_an_app_is_reported_precisely() {
        let only_id = Secrets {
            github_app_id: Some(7),
            ..Secrets::default()
        };
        let err = only_id.github_auth().unwrap_err().to_string();
        assert!(err.contains(ENV_GITHUB_APP_PRIVATE_KEY_PATH), "{err}");

        let only_key = Secrets {
            github_private_key_path: Some(PathBuf::from("/k.pem")),
            ..Secrets::default()
        };
        let err = only_key.github_auth().unwrap_err().to_string();
        assert!(err.contains(ENV_GITHUB_APP_ID), "{err}");
    }

    #[test]
    fn without_any_credentials_both_ways_are_listed() {
        let err = Secrets::default().github_auth().unwrap_err().to_string();
        assert!(err.contains(ENV_GITHUB_TOKEN), "{err}");
        assert!(err.contains(ENV_GITHUB_APP_ID), "{err}");
        assert!(err.contains(ENV_GITHUB_APP_PRIVATE_KEY_PATH), "{err}");
    }

    #[test]
    fn the_token_is_redacted_in_logs() {
        let secrets = Secrets {
            github_token: Some("ghp_supersecrettoken".into()),
            ..Secrets::default()
        };
        assert_eq!(
            secrets
                .redactor()
                .redact("pushing with ghp_supersecrettoken"),
            "pushing with ***"
        );
        let text = format!("{secrets:?}");
        assert!(!text.contains("ghp_supersecrettoken"), "{text}");
    }

    #[test]
    fn env_overrides_replace_the_paths_from_the_file() {
        // The override is applied to a parsed config, so the test touches no
        // global state beyond the two variables it sets itself.
        let mut config = Config::from_toml(MINIMAL).unwrap();
        let before = config.data_dir.clone();
        config.apply_env_overrides();
        assert_eq!(config.data_dir, before, "nothing set, nothing changed");

        config.data_dir = PathBuf::from("/from/env/data");
        config.skills_dir = PathBuf::from("/from/env/skills");
        assert_eq!(
            config.db_path(),
            PathBuf::from("/from/env/data/momulus.sqlite")
        );
        assert_eq!(config.repos_dir(), PathBuf::from("/from/env/data/repos"));
    }

    #[test]
    fn secrets_debug_hides_the_key() {
        let secrets = Secrets {
            github_app_id: Some(1),
            github_private_key_path: Some(PathBuf::from("/k.pem")),
            llm_api_key: Some("sk-ant-secret-value".into()),
            ..Secrets::default()
        };
        let text = format!("{secrets:?}");
        assert!(!text.contains("sk-ant-secret-value"), "{text}");
        assert!(text.contains("***"), "{text}");
    }

    #[test]
    fn missing_secrets_are_reported_by_name() {
        let err = Secrets::default().require_llm_api_key().unwrap_err();
        assert!(err.to_string().contains(ENV_LLM_API_KEY), "{err}");
    }
}
