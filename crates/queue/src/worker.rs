//! Worker: claims jobs from the queue, runs them and maintains their status.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use momulus_core::{AckState, Error, Job, JobId, Result};
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::Store;

/// What the worker does with a job. Implemented by the pipeline.
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// Runs the job. The worker classifies the error itself.
    async fn handle(&self, job: &Job) -> Result<()>;

    /// Marks the job state on the platform (a reaction on the comment).
    async fn ack(&self, job: &Job, state: AckState) -> Result<()> {
        let _ = (job, state);
        Ok(())
    }

    /// Posts the failure reason to the PR.
    async fn report_error(&self, job: &Job, error: &Error) -> Result<()> {
        let _ = (job, error);
        Ok(())
    }
}

/// Worker settings.
#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    /// How many jobs run at the same time.
    pub concurrency: usize,
    /// Maximum attempts per job: the first one plus two retries of transient errors.
    pub max_attempts: u32,
    /// How long we wait for running jobs on shutdown.
    pub shutdown_timeout: Duration,
    /// How often we peek into the queue when nothing woke us up.
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

/// The queue worker.
pub struct Worker {
    store: Store,
    handler: Arc<dyn JobHandler>,
    config: WorkerConfig,
    /// Wakes the dispatcher when a new job lands in the queue.
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

    /// Runs until cancelled: accepts jobs from the channel and executes them.
    ///
    /// On start, anything left in `running` after a crash goes back to the queue.
    pub async fn run(
        &self,
        mut rx: mpsc::Receiver<Job>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        let recovered = self.store.recover_running().await?;
        if recovered > 0 {
            tracing::info!(recovered, "jobs from the previous run were requeued");
        }

        let semaphore = Arc::new(Semaphore::new(self.config.concurrency));
        let mut running: JoinSet<()> = JoinSet::new();
        let mut inbox_open = true;

        loop {
            // Drain the queue while there are free slots and work to take.
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
                // A new job from the trigger.
                received = rx.recv(), if inbox_open => match received {
                    Some(job) => {
                        let id = job.id;
                        match self.store.enqueue(&job).await {
                            Ok(true) => {
                                tracing::info!(job = %id, skill = %job.command.skill, "job accepted");
                            }
                            Ok(false) => {
                                tracing::debug!(comment = job.comment.id, "a job for this comment already exists");
                            }
                            Err(err) => tracing::error!(error = %err, "the job was not stored"),
                        }
                    }
                    None => {
                        tracing::debug!("the job channel is closed");
                        inbox_open = false;
                    }
                },
                // A job finished — a slot is free again.
                _ = self.wake.notified() => {}
                // A job may have been queued outside the channel (a retry, say).
                _ = tokio::time::sleep(self.config.idle_tick) => {}
                _ = shutdown.cancelled() => break,
            }
        }

        // Shutdown: take no new jobs, wait for the running ones with a timeout.
        let pending = running.len();
        if pending > 0 {
            tracing::info!(pending, "waiting for the running jobs to finish");
        }
        let wait = tokio::time::timeout(self.config.shutdown_timeout, async {
            while running.join_next().await.is_some() {}
        });
        if wait.await.is_err() {
            tracing::warn!(
                timeout_s = self.config.shutdown_timeout.as_secs(),
                "jobs did not finish in time, aborting them"
            );
            running.shutdown().await;
        }
        Ok(())
    }

    /// A handle for tests and retries: wake the dispatcher.
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

/// Runs a single job and records its status.
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
        // The 👀 reaction is set once, on the first attempt.
        if let Err(err) = handler.ack(&job, AckState::Received).await {
            tracing::warn!(job = %job.id, error = %err, "could not acknowledge picking up the job");
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
                "transient error, requeueing the job"
            );
            if let Err(err) = store.requeue(job.id, &err.to_string()).await {
                tracing::error!(job = %job.id, error = %err, "could not requeue the job");
            }
        }
        Err(err) => {
            tracing::error!(job = %job.id, attempt, error = %err, "the job failed");
            if let Err(err) = store.mark_failed(job.id, &err.to_string(), None).await {
                tracing::error!(error = %err, "the job status was not recorded");
            }
            if let Err(report) = handler.report_error(&job, &err).await {
                tracing::warn!(error = %report, "could not report the error to the PR");
            }
            if let Err(err) = handler.ack(&job, AckState::Failed).await {
                tracing::warn!(error = %err, "could not set the failure reaction");
            }
        }
    }
    wake.notify_one();
}

async fn finish_ok(store: &Store, handler: &Arc<dyn JobHandler>, job: &Job) {
    if let Err(err) = store.mark_done(job.id, None).await {
        tracing::error!(job = %job.id, error = %err, "the job status was not recorded");
    }
    if let Err(err) = handler.ack(job, AckState::Succeeded).await {
        tracing::warn!(error = %err, "could not set the success reaction");
    }
}

/// Handy for tests: check that a job exists and what status it is in.
pub async fn status_of(store: &Store, id: JobId) -> Result<Option<momulus_core::JobStatus>> {
    Ok(store.job(id).await?.map(|stored| stored.status))
}
