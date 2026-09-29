//! Воркер: берёт джобы из очереди, выполняет их и ведёт статусы.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use llm_bot_core::{AckState, Error, Job, JobId, Result};
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::Store;

/// Что воркер делает с джобой. Реализуется пайплайном.
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// Выполняет джобу. Ошибку воркер классифицирует сам.
    async fn handle(&self, job: &Job) -> Result<()>;

    /// Отмечает состояние джобы на платформе (реакция на комментарий).
    async fn ack(&self, job: &Job, state: AckState) -> Result<()> {
        let _ = (job, state);
        Ok(())
    }

    /// Пишет в PR причину провала.
    async fn report_error(&self, job: &Job, error: &Error) -> Result<()> {
        let _ = (job, error);
        Ok(())
    }
}

/// Настройки воркера.
#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    /// Сколько джоб выполняем одновременно.
    pub concurrency: usize,
    /// Максимум попыток на джобу: первая плюс два повтора транзиентных ошибок.
    pub max_attempts: u32,
    /// Сколько ждём завершения текущих джоб при остановке.
    pub shutdown_timeout: Duration,
    /// Как часто заглядываем в очередь, если никто не разбудил.
    pub idle_tick: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            concurrency: 1,
            max_attempts: 3,
            shutdown_timeout: Duration::from_secs(300),
            idle_tick: Duration::from_secs(5),
        }
    }
}

/// Воркер очереди.
pub struct Worker {
    store: Store,
    handler: Arc<dyn JobHandler>,
    config: WorkerConfig,
    /// Будит диспетчер, когда в очередь попала новая джоба.
    wake: Arc<Notify>,
}

impl Worker {
    pub fn new(store: Store, handler: Arc<dyn JobHandler>, config: WorkerConfig) -> Worker {
        Worker {
            store,
            handler,
            config,
            wake: Arc::new(Notify::new()),
        }
    }

    /// Работает до отмены: принимает джобы из канала и выполняет их.
    ///
    /// При старте всё, что осталось в `running` после падения сервиса,
    /// возвращается в очередь.
    pub async fn run(
        &self,
        mut rx: mpsc::Receiver<Job>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        let recovered = self.store.recover_running().await?;
        if recovered > 0 {
            tracing::info!(recovered, "джобы из прошлого запуска возвращены в очередь");
        }

        let semaphore = Arc::new(Semaphore::new(self.config.concurrency));
        let mut running: JoinSet<()> = JoinSet::new();
        let mut inbox_open = true;

        loop {
            // Разбираем очередь, пока есть свободные слоты и есть что брать.
            while !shutdown.is_cancelled() {
                let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                    break;
                };
                match self.store.claim_next().await? {
                    Some(stored) => {
                        let store = self.store.clone();
                        let handler = self.handler.clone();
                        let wake = self.wake.clone();
                        let max_attempts = self.config.max_attempts;
                        running.spawn(async move {
                            let _permit = permit;
                            run_one(store, handler, wake, stored, max_attempts).await;
                        });
                    }
                    None => {
                        drop(permit);
                        break;
                    }
                }
            }

            if shutdown.is_cancelled() {
                break;
            }

            tokio::select! {
                // Новая джоба от триггера.
                received = rx.recv(), if inbox_open => match received {
                    Some(job) => {
                        let id = job.id;
                        match self.store.enqueue(&job).await {
                            Ok(true) => {
                                tracing::info!(job = %id, skill = %job.command.skill, "джоба принята");
                            }
                            Ok(false) => {
                                tracing::debug!(comment = job.comment.id, "джоба на этот комментарий уже есть");
                            }
                            Err(err) => tracing::error!(error = %err, "джоба не сохранена"),
                        }
                    }
                    None => {
                        tracing::debug!("канал джоб закрыт");
                        inbox_open = false;
                    }
                },
                // Джоба завершилась — освободился слот.
                _ = self.wake.notified() => {}
                // Кто-то мог положить джобу мимо канала (например, ретрай).
                _ = tokio::time::sleep(self.config.idle_tick) => {}
                _ = shutdown.cancelled() => break,
            }
        }

        // Останов: новых джоб не берём, текущие дожидаем с таймаутом.
        let pending = running.len();
        if pending > 0 {
            tracing::info!(pending, "ждём завершения текущих джоб");
        }
        let wait = tokio::time::timeout(self.config.shutdown_timeout, async {
            while running.join_next().await.is_some() {}
        });
        if wait.await.is_err() {
            tracing::warn!(
                timeout_s = self.config.shutdown_timeout.as_secs(),
                "джобы не успели завершиться, прерываем"
            );
            running.shutdown().await;
        }
        Ok(())
    }

    /// Ручка для тестов и для ретраев: разбудить диспетчер.
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

/// Выполняет одну джобу и записывает её статус.
async fn run_one(
    store: Store,
    handler: Arc<dyn JobHandler>,
    wake: Arc<Notify>,
    stored: crate::StoredJob,
    max_attempts: u32,
) {
    let job = stored.job;
    let attempt = stored.attempts;

    if attempt == 1 {
        // Реакция 👀 ставится один раз, на первой попытке.
        if let Err(err) = handler.ack(&job, AckState::Received).await {
            tracing::warn!(job = %job.id, error = %err, "не удалось отметить взятие джобы");
        }
    }

    let result = handler.handle(&job).await;
    match result {
        Ok(()) => {
            finish_ok(&store, &handler, &job).await;
        }
        Err(err) if err.is_transient() && attempt < max_attempts => {
            tracing::warn!(
                job = %job.id,
                attempt,
                error = %err,
                "транзиентная ошибка, вернём джобу в очередь"
            );
            if let Err(err) = store.requeue(job.id, &err.to_string()).await {
                tracing::error!(job = %job.id, error = %err, "не удалось вернуть джобу в очередь");
            }
        }
        Err(err) => {
            tracing::error!(job = %job.id, attempt, error = %err, "джоба провалилась");
            if let Err(err) = store.mark_failed(job.id, &err.to_string(), None).await {
                tracing::error!(error = %err, "статус джобы не записан");
            }
            if let Err(report) = handler.report_error(&job, &err).await {
                tracing::warn!(error = %report, "не удалось сообщить об ошибке в PR");
            }
            if let Err(err) = handler.ack(&job, AckState::Failed).await {
                tracing::warn!(error = %err, "не удалось поставить отметку о провале");
            }
        }
    }
    wake.notify_one();
}

async fn finish_ok(store: &Store, handler: &Arc<dyn JobHandler>, job: &Job) {
    if let Err(err) = store.mark_done(job.id, None).await {
        tracing::error!(job = %job.id, error = %err, "статус джобы не записан");
    }
    if let Err(err) = handler.ack(job, AckState::Succeeded).await {
        tracing::warn!(error = %err, "не удалось поставить отметку об успехе");
    }
}

/// Полезно тестам: проверить, что джоба существует и в каком она статусе.
pub async fn status_of(store: &Store, id: JobId) -> Result<Option<llm_bot_core::JobStatus>> {
    Ok(store.job(id).await?.map(|stored| stored.status))
}
