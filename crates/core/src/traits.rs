//! Границы между ядром и платформами. Реализуй их, чтобы добавить новую платформу.

use async_trait::async_trait;
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
