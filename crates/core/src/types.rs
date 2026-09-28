//! Платформенно-независимые типы, которыми оперирует пайплайн.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::command::Command;

/// Идентификатор джобы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub Uuid);

impl JobId {
    pub fn new() -> Self {
        JobId(Uuid::new_v4())
    }

    /// Короткая форма для сообщений в PR.
    pub fn short(&self) -> String {
        self.0.simple().to_string()[..8].to_string()
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for JobId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(JobId(Uuid::parse_str(s)?))
    }
}

/// Ссылка на pull request на какой-то платформе.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRef {
    /// Идентификатор платформы: "github", "gitlab", ...
    pub platform: String,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    /// SHA головного коммита PR.
    pub head_sha: String,
    /// Имя head-ветки (без remote).
    pub head_ref: String,
    /// Имя base-ветки.
    pub base_ref: String,
    /// Полное имя репозитория head-а ("owner/repo"); у форка отличается от base.
    pub head_repo: String,
    /// URL для клонирования base-репозитория.
    pub clone_url: String,
}

impl PrRef {
    /// Полное имя base-репозитория.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// PR открыт из форка — push в head-ветку нам недоступен.
    pub fn is_fork(&self) -> bool {
        self.head_repo != self.full_name()
    }

    /// Стабильный ключ репозитория для кэша и курсоров.
    pub fn repo_key(&self) -> String {
        format!("{}:{}/{}", self.platform, self.owner, self.repo)
    }
}

/// Где именно оставлен комментарий с командой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    /// Обычный комментарий к PR (issue comment).
    Issue,
    /// Комментарий в треде ревью (review comment).
    Review,
}

impl CommentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentKind::Issue => "issue",
            CommentKind::Review => "review",
        }
    }
}

/// Комментарий, в котором пришла команда.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRef {
    pub id: u64,
    pub kind: CommentKind,
    pub author: String,
    pub url: Option<String>,
}

/// Единица работы: одна команда `/llm` из одного комментария.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub pr: PrRef,
    pub command: Command,
    pub comment: CommentRef,
    pub created_at: DateTime<Utc>,
}

impl Job {
    pub fn new(pr: PrRef, command: Command, comment: CommentRef) -> Self {
        Job {
            id: JobId::new(),
            pr,
            command,
            comment,
            created_at: Utc::now(),
        }
    }

    pub fn job_ref(&self) -> JobRef {
        JobRef {
            id: self.id,
            pr: self.pr.clone(),
            comment: self.comment.clone(),
        }
    }
}

/// Минимум, нужный для подтверждения статуса джобы на платформе.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRef {
    pub id: JobId,
    pub pr: PrRef,
    pub comment: CommentRef,
}

/// Статус джобы в очереди.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for JobStatus {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(JobStatus::Queued),
            "running" => Ok(JobStatus::Running),
            "done" => Ok(JobStatus::Done),
            "failed" => Ok(JobStatus::Failed),
            other => Err(crate::Error::Storage(format!(
                "unknown job status: {other}"
            ))),
        }
    }
}

/// Отметка о состоянии джобы, которую видно в PR (реакции на комментарий).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckState {
    /// Взяли в работу — 👀
    Received,
    /// Успех — ✅
    Succeeded,
    /// Провал — ❌
    Failed,
}

/// Категория замечания.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Typo,
    Grammar,
    Punctuation,
    Style,
    Terminology,
    Bug,
    Security,
    Other,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Typo => "typo",
            Severity::Grammar => "grammar",
            Severity::Punctuation => "punctuation",
            Severity::Style => "style",
            Severity::Terminology => "terminology",
            Severity::Bug => "bug",
            Severity::Security => "security",
            Severity::Other => "other",
        }
    }
}

/// Одно замечание модели, привязанное к строке файла.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// Путь к файлу относительно корня репозитория.
    pub path: String,
    /// Номер строки в новой версии файла (1-based).
    pub line: u32,
    pub severity: Severity,
    /// Текст замечания.
    pub body: String,
    /// Предлагаемая замена строки целиком, если применимо.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Результат работы скилла в режиме review — ровно то, что пишет модель.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutput {
    /// Краткое резюме ревью.
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// Готовый к публикации патч: изменения уже лежат в worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// Рабочая копия с изменениями.
    pub worktree: PathBuf,
    /// Имя ветки, в которую коммитим.
    pub branch: String,
    pub commit_message: String,
    /// Заголовок будущего PR.
    pub title: String,
    /// Тело будущего PR.
    pub body: String,
    /// Изменённые файлы (относительные пути).
    pub files: Vec<String>,
}

/// Куда монтируется рабочая копия внутри контейнера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mount {
    ReadOnly,
    ReadWrite,
}

/// Описание единственного LLM-шага.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    pub job_id: JobId,
    /// Хостовый путь рабочей копии, монтируется в /work.
    pub workdir: PathBuf,
    /// Хостовый путь каталога скиллов, монтируется в /skills (ro).
    pub skills_dir: PathBuf,
    /// Хостовый путь для артефактов, монтируется в /out (rw).
    pub out_dir: PathBuf,
    pub mount: Mount,
    pub prompt: String,
    pub tools: Vec<String>,
    pub provider: String,
    pub model: Option<String>,
    pub timeout: Duration,
    pub image: String,
    pub cpu_limit: f64,
    pub memory_limit_mb: u64,
    /// Переменные окружения контейнера (только ключ LLM-провайдера и base URL).
    pub env: Vec<(String, String)>,
}

/// Что вернул LLM-шаг.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl RunResult {
    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    /// Совмещённый лог для сохранения в файл джобы.
    pub fn combined_log(&self) -> String {
        format!(
            "=== stdout ===\n{}\n=== stderr ===\n{}\n",
            self.stdout, self.stderr
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(head_repo: &str) -> PrRef {
        PrRef {
            platform: "github".into(),
            owner: "acme".into(),
            repo: "blog".into(),
            number: 42,
            head_sha: "deadbeef".into(),
            head_ref: "feature".into(),
            base_ref: "main".into(),
            head_repo: head_repo.into(),
            clone_url: "https://github.com/acme/blog.git".into(),
        }
    }

    #[test]
    fn fork_is_detected_by_head_repo() {
        assert!(!pr("acme/blog").is_fork());
        assert!(pr("contributor/blog").is_fork());
    }

    #[test]
    fn repo_key_is_stable() {
        assert_eq!(pr("acme/blog").repo_key(), "github:acme/blog");
    }

    #[test]
    fn job_status_roundtrip() {
        for status in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Done,
            JobStatus::Failed,
        ] {
            let parsed: JobStatus = status.as_str().parse().unwrap();
            assert_eq!(parsed, status);
        }
        assert!("nope".parse::<JobStatus>().is_err());
    }

    #[test]
    fn findings_reject_unknown_fields() {
        let json = r#"{"path":"a.md","line":1,"severity":"typo","body":"x","oops":true}"#;
        let err = serde_json::from_str::<Finding>(json).unwrap_err();
        assert!(err.to_string().contains("oops"), "{err}");
    }

    #[test]
    fn review_output_parses_minimal_json() {
        let out: ReviewOutput = serde_json::from_str(r#"{"summary":"ok"}"#).unwrap();
        assert!(out.findings.is_empty());
    }

    #[test]
    fn job_id_short_is_eight_chars() {
        assert_eq!(JobId::new().short().len(), 8);
    }
}
