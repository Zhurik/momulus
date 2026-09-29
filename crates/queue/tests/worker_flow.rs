//! Поведение воркера: статусы, ретраи, конкурентность, остановка.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use llm_bot_core::{
    AckState, Command, CommentKind, CommentRef, Error, Job, JobStatus, PrRef, Result,
};
use llm_bot_queue::{Db, JobHandler, Store, Worker, WorkerConfig};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

fn pr() -> PrRef {
    PrRef {
        platform: "github".into(),
        owner: "acme".into(),
        repo: "blog".into(),
        number: 42,
        head_sha: "abc123".into(),
        head_ref: "feature".into(),
        base_ref: "main".into(),
        head_repo: "acme/blog".into(),
        clone_url: "https://github.com/acme/blog.git".into(),
    }
}

fn job(comment_id: u64) -> Job {
    Job::new(
        pr(),
        Command::parse("/llm proofread").unwrap(),
        CommentRef {
            id: comment_id,
            kind: CommentKind::Issue,
            author: "zhurik".into(),
            url: None,
        },
    )
}

/// Обработчик, которым управляет тест.
struct ScriptedHandler {
    /// Что вернуть на каждый вызов по порядку; когда сценарий кончился — успех.
    script: Mutex<Vec<Result<()>>>,
    calls: AtomicU32,
    acks: Mutex<Vec<AckState>>,
    errors: AtomicU32,
    /// Задержка внутри обработки — для проверки остановки и конкурентности.
    delay: Duration,
}

impl ScriptedHandler {
    fn new(script: Vec<Result<()>>) -> Arc<ScriptedHandler> {
        Arc::new(ScriptedHandler {
            script: Mutex::new(script.into_iter().rev().collect()),
            calls: AtomicU32::new(0),
            acks: Mutex::new(Vec::new()),
            errors: AtomicU32::new(0),
            delay: Duration::ZERO,
        })
    }

    fn slow(delay: Duration) -> Arc<ScriptedHandler> {
        Arc::new(ScriptedHandler {
            script: Mutex::new(Vec::new()),
            calls: AtomicU32::new(0),
            acks: Mutex::new(Vec::new()),
            errors: AtomicU32::new(0),
            delay,
        })
    }

    fn call_count(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JobHandler for ScriptedHandler {
    async fn handle(&self, _job: &Job) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let mut script = self.script.lock().await;
        script.pop().unwrap_or(Ok(()))
    }

    async fn ack(&self, _job: &Job, state: AckState) -> Result<()> {
        self.acks.lock().await.push(state);
        Ok(())
    }

    async fn report_error(&self, _job: &Job, _error: &Error) -> Result<()> {
        self.errors.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Запускает воркер, ждёт условие и останавливает его.
async fn run_until<F, Fut>(
    store: Store,
    handler: Arc<ScriptedHandler>,
    config: WorkerConfig,
    jobs: Vec<Job>,
    condition: F,
) where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let (tx, rx) = mpsc::channel(16);
    let shutdown = CancellationToken::new();
    let worker = Worker::new(store, handler, config);

    let task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { worker.run(rx, shutdown).await })
    };

    for job in jobs {
        tx.send(job).await.unwrap();
    }

    // Ждём выполнения условия, но не дольше пяти секунд.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !condition().await {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    shutdown.cancel();
    drop(tx);
    task.await.unwrap().unwrap();
}

fn fast_config() -> WorkerConfig {
    WorkerConfig {
        concurrency: 1,
        max_attempts: 3,
        shutdown_timeout: Duration::from_secs(5),
        idle_tick: Duration::from_millis(20),
    }
}

#[tokio::test]
async fn successful_job_is_marked_done_and_acked() {
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = ScriptedHandler::new(vec![Ok(())]);
    let job = job(1001);
    let id = job.id;

    let probe = store.clone();
    run_until(
        store.clone(),
        handler.clone(),
        fast_config(),
        vec![job],
        move || {
            let probe = probe.clone();
            async move { probe.count_by_status(JobStatus::Done).await.unwrap_or(0) == 1 }
        },
    )
    .await;

    let stored = store.job(id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Done);
    assert_eq!(stored.attempts, 1);
    assert_eq!(handler.call_count(), 1);
    let acks = handler.acks.lock().await.clone();
    assert_eq!(acks, vec![AckState::Received, AckState::Succeeded]);
}

#[tokio::test]
async fn transient_error_is_retried_then_succeeds() {
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = ScriptedHandler::new(vec![Err(Error::Network("сеть отвалилась".into())), Ok(())]);
    let job = job(1002);
    let id = job.id;

    let probe = store.clone();
    run_until(
        store.clone(),
        handler.clone(),
        fast_config(),
        vec![job],
        move || {
            let probe = probe.clone();
            async move { probe.count_by_status(JobStatus::Done).await.unwrap_or(0) == 1 }
        },
    )
    .await;

    let stored = store.job(id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Done);
    assert_eq!(stored.attempts, 2, "одна повторная попытка");
    assert_eq!(handler.call_count(), 2);
    // Отметка о взятии ставится только на первой попытке.
    let acks = handler.acks.lock().await.clone();
    assert_eq!(acks.iter().filter(|s| **s == AckState::Received).count(), 1);
}

#[tokio::test]
async fn transient_errors_stop_after_max_attempts() {
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = ScriptedHandler::new(vec![
        Err(Error::Network("раз".into())),
        Err(Error::Network("два".into())),
        Err(Error::Network("три".into())),
        Err(Error::Network("четыре".into())),
    ]);
    let job = job(1003);
    let id = job.id;

    let probe = store.clone();
    run_until(
        store.clone(),
        handler.clone(),
        fast_config(),
        vec![job],
        move || {
            let probe = probe.clone();
            async move { probe.count_by_status(JobStatus::Failed).await.unwrap_or(0) == 1 }
        },
    )
    .await;

    let stored = store.job(id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(stored.attempts, 3, "первая попытка плюс два повтора");
    assert_eq!(handler.call_count(), 3);
    assert!(stored.error.unwrap().contains("три"));
    assert_eq!(handler.errors.load(Ordering::SeqCst), 1, "сообщили в PR");
    assert_eq!(handler.acks.lock().await.last(), Some(&AckState::Failed));
}

#[tokio::test]
async fn permanent_error_is_not_retried() {
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = ScriptedHandler::new(vec![Err(Error::InvalidOutput("не JSON".into()))]);
    let job = job(1004);
    let id = job.id;

    let probe = store.clone();
    run_until(
        store.clone(),
        handler.clone(),
        fast_config(),
        vec![job],
        move || {
            let probe = probe.clone();
            async move { probe.count_by_status(JobStatus::Failed).await.unwrap_or(0) == 1 }
        },
    )
    .await;

    let stored = store.job(id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(stored.attempts, 1);
    assert_eq!(handler.call_count(), 1);
}

#[tokio::test]
async fn duplicate_jobs_from_the_trigger_are_ignored() {
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = ScriptedHandler::new(vec![Ok(()), Ok(())]);
    // Две джобы на один и тот же комментарий.
    let first = job(1005);
    let mut second = job(1005);
    second.id = llm_bot_core::JobId::new();

    let probe = store.clone();
    run_until(
        store.clone(),
        handler.clone(),
        fast_config(),
        vec![first, second],
        move || {
            let probe = probe.clone();
            async move { probe.count_by_status(JobStatus::Done).await.unwrap_or(0) == 1 }
        },
    )
    .await;

    assert_eq!(handler.call_count(), 1, "обработали только одну");
}

#[tokio::test]
async fn jobs_run_concurrently_up_to_the_limit() {
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = ScriptedHandler::slow(Duration::from_millis(150));
    let mut config = fast_config();
    config.concurrency = 3;

    let probe = store.clone();
    run_until(
        store.clone(),
        handler.clone(),
        config,
        (1..=3).map(job).collect(),
        move || {
            let probe = probe.clone();
            async move { probe.count_by_status(JobStatus::Done).await.unwrap_or(0) == 3 }
        },
    )
    .await;

    assert_eq!(store.count_by_status(JobStatus::Done).await.unwrap(), 3);
    assert_eq!(handler.call_count(), 3);
}

#[tokio::test]
async fn shutdown_waits_for_the_running_job() {
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = ScriptedHandler::slow(Duration::from_millis(300));
    let job = job(1006);
    let id = job.id;

    let (tx, rx) = mpsc::channel(4);
    let shutdown = CancellationToken::new();
    let worker = Worker::new(store.clone(), handler.clone(), fast_config());

    let task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { worker.run(rx, shutdown).await })
    };
    tx.send(job).await.unwrap();

    // Дожидаемся, пока джоба реально начнёт выполняться, и просим остановиться.
    while handler.call_count() == 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    shutdown.cancel();
    task.await.unwrap().unwrap();

    let stored = store.job(id).await.unwrap().unwrap();
    assert_eq!(
        stored.status,
        JobStatus::Done,
        "текущая джоба досчитана до конца"
    );
}

#[tokio::test]
async fn jobs_left_running_are_recovered_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("llm-bot.sqlite");
    let job = job(1007);
    let id = job.id;

    // Предыдущий запуск умер посреди джобы.
    {
        let store = Store::new(Db::open(&path).await.unwrap());
        store.enqueue(&job).await.unwrap();
        store.claim_next().await.unwrap();
        assert_eq!(store.count_by_status(JobStatus::Running).await.unwrap(), 1);
        store.db().close().await;
    }

    let store = Store::new(Db::open(&path).await.unwrap());
    let handler = ScriptedHandler::new(vec![Ok(())]);
    let probe = store.clone();
    run_until(
        store.clone(),
        handler.clone(),
        fast_config(),
        Vec::new(),
        move || {
            let probe = probe.clone();
            async move { probe.count_by_status(JobStatus::Done).await.unwrap_or(0) == 1 }
        },
    )
    .await;

    let stored = store.job(id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Done);
    assert_eq!(stored.attempts, 2, "попытка после восстановления — вторая");
    assert_eq!(handler.call_count(), 1);
}
