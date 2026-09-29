//! `momulus serve` — the main mode: poll the platform and run jobs.

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use momulus_core::{AckState, Config, Error, GitAccess, Job, Publisher, Runner, Secrets, Trigger};
use momulus_github::trigger::ClientSource;
use momulus_github::{AppAuth, GithubApp, GithubPublisher, GithubTrigger, TriggerConfig};
use momulus_pipeline::{Outcome, Pipeline, PipelineConfig, SharedRegistry, StdoutPublisher};
use momulus_queue::{Db, JobHandler, Store, Worker, WorkerConfig};
use momulus_runner_docker::DockerRunner;
use momulus_workspace::{Git, RepoCache};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::cli::Cli;
use crate::commands::skills::resolve_skills_dir;

/// How many jobs may wait in the channel between trigger and queue.
const CHANNEL_CAPACITY: usize = 64;

pub async fn run(cli: &Cli, dry_run: bool) -> Result<()> {
    let config =
        Config::load(&cli.config).with_context(|| format!("config {}", cli.config.display()))?;
    let secrets = Secrets::from_env()?;
    config.llm.check_ready(secrets.llm_base_url.as_deref())?;

    let skills_dir = resolve_skills_dir(cli)?.canonicalize().with_context(|| {
        format!(
            "skills directory {}",
            resolve_skills_dir(cli).unwrap().display()
        )
    })?;
    let registry = SharedRegistry::load(&skills_dir)?;
    tracing::info!(
        skills = ?registry.names().await,
        dry_run,
        "the skill registry is loaded"
    );

    std::fs::create_dir_all(&config.data_dir)?;
    let store = Store::new(Db::open(&config.db_path()).await?);

    let redactor = secrets.redactor();
    let git = Git::new(redactor.clone());
    let cache = RepoCache::new(config.repos_dir(), git.clone());

    // App authentication: both API clients and git tokens grow out of it.
    let (app_id, key_path) = secrets.require_github()?;
    let auth = AppAuth::from_key_path(app_id, &key_path, &config.github.api_base)?;
    let app = GithubApp::new(auth.clone());

    let runner: Arc<dyn Runner> = Arc::new(DockerRunner::connect(
        config.docker.host.as_deref(),
        config.docker.user.clone(),
        redactor,
    )?);
    runner_check(runner.as_ref(), &config).await;

    // The git token always comes from the App, even in dry-run mode: fetching
    // the working copy is needed there too.
    let access: Arc<dyn GitAccess> = app.clone();

    let publisher: Arc<dyn Publisher> = if dry_run {
        tracing::warn!("--dry-run mode: nothing will be published");
        Arc::new(StdoutPublisher::new())
    } else {
        Arc::new(GithubPublisher::new(
            app.clone(),
            git.clone(),
            config.github.clone(),
            access.clone(),
        ))
    };

    let pipeline = Arc::new(Pipeline::new(
        registry.clone(),
        runner,
        publisher,
        access,
        cache,
        PipelineConfig {
            work_dir: config.work_dir(),
            logs_dir: config.logs_dir(),
            skills_dir,
            provider: config.llm.default_provider.clone(),
            model: config.llm.default_model.clone(),
            api: config.llm.api,
            api_key_env: config.llm.api_key_env(),
            api_key: secrets.llm_api_key.clone(),
            base_url: secrets.llm_base_url.clone(),
            image: config.docker.runner_image.clone(),
            cpu: config.docker.cpu,
            memory_mb: config.docker.memory_mb,
            limits: config.limits.clone(),
        },
    ));

    let trigger = GithubTrigger::new(
        TriggerConfig {
            poll_interval: config.poll_interval,
            allowed_users: config.allowed_users.clone(),
            repos: config.repos.clone(),
        },
        ClientSource::App(auth),
        Arc::new(store.clone()),
        registry.clone(),
    );

    let worker = Worker::new(
        store.clone(),
        Arc::new(PipelineHandler {
            pipeline: pipeline.clone(),
        }),
        WorkerConfig {
            concurrency: config.concurrency,
            max_attempts: 3,
            shutdown_timeout: config.shutdown_timeout,
            idle_tick: std::time::Duration::from_secs(5),
        },
    );

    // A shared shutdown signal for the trigger and the worker.
    let shutdown = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<Job>(CHANNEL_CAPACITY);

    let signals = tokio::spawn(watch_signals(shutdown.clone(), registry.clone()));
    let trigger_task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { trigger.run(tx, shutdown).await })
    };
    let worker_task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { worker.run(rx, shutdown).await })
    };

    tracing::info!(
        poll_interval_s = config.poll_interval.as_secs(),
        concurrency = config.concurrency,
        "the service is running"
    );

    // A failure in either task stops the service as well.
    let trigger_result = trigger_task.await.context("the trigger task")?;
    shutdown.cancel();
    let worker_result = worker_task.await.context("the worker task")?;
    signals.abort();

    trigger_result?;
    worker_result?;
    tracing::info!("the service has stopped");
    Ok(())
}

/// Reports the runner configuration at startup.
async fn runner_check(runner: &dyn Runner, config: &Config) {
    let _ = runner;
    tracing::debug!(image = %config.docker.runner_image, "the runner is configured");
}

/// SIGTERM/SIGINT shut the service down, SIGHUP reloads the skills.
async fn watch_signals(shutdown: CancellationToken, registry: Arc<SharedRegistry>) {
    let mut term = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(err) => {
            tracing::error!(error = %err, "could not subscribe to SIGTERM");
            return;
        }
    };
    let mut hup = match signal(SignalKind::hangup()) {
        Ok(signal) => signal,
        Err(err) => {
            tracing::error!(error = %err, "could not subscribe to SIGHUP");
            return;
        }
    };

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down");
                shutdown.cancel();
                return;
            }
            _ = term.recv() => {
                tracing::info!("received SIGTERM, shutting down");
                shutdown.cancel();
                return;
            }
            _ = hup.recv() => match registry.reload().await {
                Ok(count) => tracing::info!(skills = count, "the skill registry was reloaded"),
                Err(err) => tracing::error!(error = %err, "reloading the skills failed"),
            },
        }
    }
}

/// Glue between the worker and the pipeline.
struct PipelineHandler {
    pipeline: Arc<Pipeline>,
}

#[async_trait]
impl JobHandler for PipelineHandler {
    async fn handle(&self, job: &Job) -> momulus_core::Result<()> {
        let result = self.pipeline.execute(job).await?;
        match &result.outcome {
            Outcome::Review { findings } => {
                tracing::info!(job = %job.id, findings, "the review was published");
            }
            Outcome::Patch { url } => {
                tracing::info!(job = %job.id, url = %url, "the pull request is open");
            }
            Outcome::NothingToDo => tracing::info!(job = %job.id, "nothing to do"),
            Outcome::NoChanges => tracing::info!(job = %job.id, "the skill changed nothing"),
            Outcome::Rejected { reason } => {
                tracing::info!(job = %job.id, reason, "the command was refused");
            }
        }
        Ok(())
    }

    async fn ack(&self, job: &Job, state: AckState) -> momulus_core::Result<()> {
        self.pipeline.ack(job, state).await
    }

    async fn report_error(&self, job: &Job, error: &Error) -> momulus_core::Result<()> {
        self.pipeline.report_error(job, error).await
    }
}
