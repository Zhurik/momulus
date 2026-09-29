//! Границы между ядром и платформами. Реализуй их, чтобы добавить новую платформу.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::Result;
use crate::types::{AckState, Finding, Job, JobRef, Patch, PrRef, RunResult, RunSpec};

/// Источник команд: следит за платформой и отдаёт джобы в очередь.
#[async_trait]
pub trait Trigger: Send + Sync {
    /// Работает до отмены; каждая распознанная команда уходит в `tx`.
    async fn run(&self, tx: mpsc::Sender<Job>, shutdown: CancellationToken) -> Result<()>;
}

/// Приёмник результатов: публикует ревью, патчи и статусы.
#[async_trait]
pub trait Publisher: Send + Sync {
    /// Отметить состояние джобы на исходном комментарии (реакция).
    async fn ack(&self, job: &JobRef, state: AckState) -> Result<()>;

    /// Опубликовать ревью с inline-комментариями.
    async fn post_review(&self, pr: &PrRef, findings: &[Finding], summary: &str) -> Result<()>;

    /// Запушить ветку с изменениями и открыть PR; возвращает ссылку на него.
    async fn push_and_open_pr(&self, pr: &PrRef, patch: &Patch) -> Result<Url>;

    /// Обычный комментарий в PR.
    async fn comment(&self, pr: &PrRef, body: &str) -> Result<()>;
}

/// Единственный LLM-шаг пайплайна.
#[async_trait]
pub trait Runner: Send + Sync {
    async fn run(&self, spec: RunSpec) -> Result<RunResult>;
}

/// Где Trigger держит позицию опроса и память о разобранных комментариях.
///
/// Вынесено в трейт, чтобы платформенный код не зависел от SQLite.
#[async_trait]
pub trait CursorStore: Send + Sync {
    /// Позиция опроса для пары «репозиторий + поток комментариев».
    async fn cursor(&self, repo_key: &str, stream: &str) -> Result<Option<DateTime<Utc>>>;

    /// Запоминает новую позицию.
    async fn set_cursor(&self, repo_key: &str, stream: &str, value: DateTime<Utc>) -> Result<()>;

    /// Помечает комментарий разобранным. `true` — видим его впервые.
    ///
    /// Нужно, чтобы не отвечать дважды на один и тот же комментарий, даже если
    /// курсор вернулся назад или комментарий отредактировали.
    async fn mark_seen(&self, platform: &str, comment_id: u64) -> Result<bool>;
}

/// Взгляд Trigger'а на реестр скиллов: проверить имя и показать help.
///
/// Тоже трейт: реестр живёт в другом крейте и может перезагружаться на SIGHUP.
#[async_trait]
pub trait SkillCatalog: Send + Sync {
    async fn contains(&self, skill: &str) -> bool;
    async fn help_text(&self) -> String;
}
