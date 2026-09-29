//! Сквозные тесты: команда в комментарии PR → результат опубликован.
//!
//! Настоящих внешних зависимостей нет: GitHub подменён wiremock, LLM — FakeRunner,
//! remote — локальный bare-репозиторий.

use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use momulus_core::config::{GithubConfig, Limits};
use momulus_core::{
    AckState, Error, Job, JobStatus, LocalGitAccess, MemoryCursorStore, Result, StaticSkillCatalog,
};
use momulus_github::backoff::Backoff;
use momulus_github::trigger::ClientSource;
use momulus_github::{GithubPublisher, GithubTrigger, TriggerConfig};
use momulus_pipeline::fake::{FakeResponse, FakeRunner};
use momulus_pipeline::{Outcome, Pipeline, PipelineConfig, SharedRegistry};
use momulus_queue::{Db, JobHandler, Store, Worker, WorkerConfig};
use momulus_workspace::{Git, RepoCache};
use octocrab::Octocrab;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git запускается");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Bare-remote с ветками main и feature; feature правит статью.
/// Возвращает (путь к bare, head SHA ветки feature).
fn fixture_remote(tmp: &Path) -> (String, String) {
    let seed = tmp.join("seed");
    std::fs::create_dir_all(seed.join("content/posts/dns")).unwrap();
    git(&seed, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(
        seed.join("content/posts/dns/index.ru.md"),
        "# DNS\n\nстарый текст\nхвост\n",
    )
    .unwrap();
    std::fs::write(seed.join("astro.config.ts"), "export default {}\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "--quiet", "-m", "начало"]);

    git(&seed, &["checkout", "--quiet", "-b", "feature"]);
    std::fs::write(
        seed.join("content/posts/dns/index.ru.md"),
        "# DNS\n\nновый текст с ашибкой\nещё строка\nхвост\n",
    )
    .unwrap();
    std::fs::write(seed.join("astro.config.ts"), "export default { site: 1 }\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "--quiet", "-m", "правки"]);
    let head = git(&seed, &["rev-parse", "HEAD"]);
    git(&seed, &["checkout", "--quiet", "main"]);

    let bare = tmp.join("origin.git");
    git(
        &seed,
        &["clone", "--bare", "--quiet", ".", bare.to_str().unwrap()],
    );
    (bare.to_string_lossy().to_string(), head)
}

/// Каталог со скиллами proofread (review) и translate (patch).
fn fixture_skills(dir: &Path) {
    let proofread = dir.join("proofread");
    std::fs::create_dir_all(&proofread).unwrap();
    std::fs::write(
        proofread.join("skill.toml"),
        "mode = \"review\"\ntools = [\"read\", \"write\"]\nfiles = [\"**/*.md\", \"**/*.mdx\"]\nmax_comments = 10\n",
    )
    .unwrap();
    std::fs::write(
        proofread.join("SKILL.md"),
        "---\nname: proofread\ndescription: вычитка статей\n---\n\nинструкции\n",
    )
    .unwrap();

    let translate = dir.join("translate");
    std::fs::create_dir_all(&translate).unwrap();
    std::fs::write(
        translate.join("skill.toml"),
        "mode = \"patch\"\ntools = [\"read\", \"write\", \"edit\"]\nfiles = [\"**/*.md\"]\nargs = [\"lang\"]\nbranch = \"llm/{skill}-{lang}-{pr}\"\n",
    )
    .unwrap();
    std::fs::write(
        translate.join("SKILL.md"),
        "---\nname: translate\ndescription: перевод статей\n---\n\nинструкции\n",
    )
    .unwrap();
}

fn client(server: &MockServer) -> Octocrab {
    Octocrab::builder()
        .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
        .base_uri(server.uri())
        .unwrap()
        .user_access_token("test-token".to_string())
        .build()
        .unwrap()
}

fn fast_backoff() -> Backoff {
    Backoff {
        attempts: 2,
        base: Duration::from_millis(1),
        max: Duration::from_millis(5),
    }
}

/// Мокает всё, что нужно триггеру: комментарий с командой и данные PR.
async fn mock_trigger(server: &MockServer, body: &str, remote: &str, head_sha: &str) {
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": 1001,
            "body": body,
            "user": { "login": "zhurik", "type": "User" },
            "html_url": "https://github.com/acme/blog/pull/42#issuecomment-1001",
            "issue_url": "https://api.github.com/repos/acme/blog/issues/42",
            "created_at": "2026-09-29T10:00:00Z",
            "updated_at": "2026-09-29T10:00:00Z"
        }])))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 42,
            "state": "open",
            "head": {
                "ref": "feature",
                "sha": head_sha,
                "repo": { "full_name": "acme/blog", "clone_url": remote }
            },
            "base": {
                "ref": "main",
                "sha": "base",
                "repo": { "full_name": "acme/blog", "clone_url": remote }
            }
        })))
        .mount(server)
        .await;
    // Реакции на комментарий-команду.
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/comments/1001/reactions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
        .mount(server)
        .await;
}

fn trigger(server: &MockServer, skills: &[&str]) -> GithubTrigger {
    GithubTrigger::new(
        TriggerConfig {
            poll_interval: Duration::from_millis(20),
            allowed_users: vec!["zhurik".into()],
            repos: vec!["acme/blog".into()],
        },
        ClientSource::Fixed {
            client: client(server),
            repos: vec!["acme/blog".into()],
        },
        Arc::new(MemoryCursorStore::new()),
        Arc::new(StaticSkillCatalog::new(skills.to_vec())),
    )
    .with_backoff(fast_backoff())
}

fn pipeline(
    tmp: &Path,
    server: &MockServer,
    skills_dir: &Path,
    runner: Arc<FakeRunner>,
) -> Arc<Pipeline> {
    let publisher = Arc::new(
        GithubPublisher::new(
            Arc::new(momulus_github::FixedClient(client(server))),
            Git::default(),
            GithubConfig::default(),
            Arc::new(LocalGitAccess),
        )
        .with_backoff(fast_backoff()),
    );
    Arc::new(Pipeline::new(
        SharedRegistry::load(skills_dir).unwrap(),
        runner,
        publisher,
        Arc::new(LocalGitAccess),
        RepoCache::new(tmp.join("repos"), Git::default()),
        PipelineConfig {
            work_dir: tmp.join("work"),
            logs_dir: tmp.join("logs"),
            skills_dir: skills_dir.to_path_buf(),
            provider: "cloudru".into(),
            model: "zai-org/GLM-5.1".into(),
            api: None,
            api_key_env: "CLOUDRU_API_KEY".into(),
            api_key: Some("test-key".into()),
            base_url: None,
            image: "momulus-runner:latest".into(),
            cpu: 1.0,
            memory_mb: 512,
            limits: Limits::default(),
        },
    ))
}

/// Обработчик для воркера: тонкая обёртка над пайплайном.
struct PipelineHandler {
    pipeline: Arc<Pipeline>,
    outcomes: tokio::sync::Mutex<Vec<Outcome>>,
}

#[async_trait]
impl JobHandler for PipelineHandler {
    async fn handle(&self, job: &Job) -> Result<()> {
        let result = self.pipeline.execute(job).await?;
        self.outcomes.lock().await.push(result.outcome);
        Ok(())
    }

    async fn ack(&self, job: &Job, state: AckState) -> Result<()> {
        self.pipeline.ack(job, state).await
    }

    async fn report_error(&self, job: &Job, error: &Error) -> Result<()> {
        self.pipeline.report_error(job, error).await
    }
}

/// Прогоняет триггер, воркер и пайплайн до появления джобы в терминальном статусе.
async fn run_pipeline_once(
    store: Store,
    trigger: GithubTrigger,
    handler: Arc<PipelineHandler>,
) -> Vec<Outcome> {
    let (tx, rx) = mpsc::channel(8);
    let shutdown = CancellationToken::new();

    let worker = Worker::new(
        store.clone(),
        handler.clone(),
        WorkerConfig {
            concurrency: 1,
            max_attempts: 1,
            shutdown_timeout: Duration::from_secs(10),
            idle_tick: Duration::from_millis(20),
        },
    );
    let worker_task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { worker.run(rx, shutdown).await })
    };

    // Одного прохода опроса достаточно: команда уже лежит в моках.
    for job in trigger.poll_once().await.unwrap() {
        tx.send(job).await.unwrap();
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let done = store.count_by_status(JobStatus::Done).await.unwrap();
        let failed = store.count_by_status(JobStatus::Failed).await.unwrap();
        if done + failed > 0 || tokio::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    shutdown.cancel();
    drop(tx);
    worker_task.await.unwrap().unwrap();
    handler.outcomes.lock().await.clone()
}

#[tokio::test]
async fn review_command_publishes_a_review() {
    let tmp = tempfile::tempdir().unwrap();
    let (remote, head) = fixture_remote(tmp.path());
    let skills = tmp.path().join("skills");
    fixture_skills(&skills);

    let server = MockServer::start().await;
    mock_trigger(&server, "/llm proofread", &remote, &head).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls/42/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 7 })))
        .expect(1)
        .mount(&server)
        .await;

    // Модель нашла две проблемы: одну в diff, одну за его пределами.
    let runner = Arc::new(FakeRunner::with_findings(
        r#"{
            "summary": "Одна опечатка и одно замечание вне diff.",
            "findings": [
                { "path": "/work/content/posts/dns/index.ru.md", "line": 3,
                  "severity": "typo", "body": "ашибкой → ошибкой",
                  "suggestion": "новый текст с ошибкой" },
                { "path": "content/posts/dns/index.ru.md", "line": 999,
                  "severity": "style", "body": "вне diff" },
                { "path": "astro.config.ts", "line": 1,
                  "severity": "other", "body": "файл вне области скилла" }
            ]
        }"#,
    ));

    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = Arc::new(PipelineHandler {
        pipeline: pipeline(tmp.path(), &server, &skills, runner.clone()),
        outcomes: tokio::sync::Mutex::new(Vec::new()),
    });

    let outcomes =
        run_pipeline_once(store.clone(), trigger(&server, &["proofread"]), handler).await;

    assert_eq!(store.count_by_status(JobStatus::Done).await.unwrap(), 1);
    assert_eq!(outcomes, vec![Outcome::Review { findings: 3 }]);

    // Проверяем то, что реально ушло в GitHub.
    let requests = server.received_requests().await.unwrap();
    let review = requests
        .iter()
        .find(|r| r.url.path() == "/repos/acme/blog/pulls/42/reviews")
        .expect("ревью опубликовано");
    let body: serde_json::Value = serde_json::from_slice(&review.body).unwrap();
    assert_eq!(body["event"], "COMMENT");
    assert_eq!(body["commit_id"], head);

    let comments = body["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "inline только то, что попало в diff");
    assert_eq!(comments[0]["path"], "content/posts/dns/index.ru.md");
    assert_eq!(comments[0]["line"], 3);
    assert!(
        comments[0]["body"]
            .as_str()
            .unwrap()
            .contains("```suggestion"),
        "{}",
        comments[0]["body"]
    );

    let summary = body["body"].as_str().unwrap();
    assert!(summary.contains("вне diff"), "{summary}");
    assert!(summary.contains("astro.config.ts"), "{summary}");

    // Реакции: 👀 при взятии и ✅ в конце.
    let reactions: Vec<String> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/reactions"))
        .map(|r| {
            serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["content"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(reactions, vec!["eyes", "+1"]);

    // Лог агента сохранён, рабочая копия убрана.
    assert!(tmp.path().join("logs").read_dir().unwrap().next().is_some());
    assert!(
        !tmp.path().join("work").exists()
            || tmp.path().join("work").read_dir().unwrap().next().is_none(),
        "worktree удалён"
    );
    assert_eq!(runner.call_count(), 1);
}

#[tokio::test]
async fn patch_command_pushes_a_branch_and_opens_a_pull_request() {
    let tmp = tempfile::tempdir().unwrap();
    let (remote, head) = fixture_remote(tmp.path());
    let skills = tmp.path().join("skills");
    fixture_skills(&skills);

    let server = MockServer::start().await;
    mock_trigger(&server, "/llm translate en", &remote, &head).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 43,
            "html_url": "https://github.com/acme/blog/pull/43"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 2 })))
        .mount(&server)
        .await;

    // Модель создала перевод и оставила резюме.
    let runner = Arc::new(FakeRunner::new(vec![FakeResponse::patch(
        vec![(
            "content/posts/dns/index.en.md".into(),
            "# DNS\n\nnew text with a typo\n".into(),
        )],
        "Перевёл статью про DNS, код и ссылки не трогал.",
    )]));

    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = Arc::new(PipelineHandler {
        pipeline: pipeline(tmp.path(), &server, &skills, runner),
        outcomes: tokio::sync::Mutex::new(Vec::new()),
    });

    let outcomes =
        run_pipeline_once(store.clone(), trigger(&server, &["translate"]), handler).await;

    assert_eq!(store.count_by_status(JobStatus::Done).await.unwrap(), 1);
    assert!(
        matches!(&outcomes[0], Outcome::Patch { url } if url.as_str() == "https://github.com/acme/blog/pull/43"),
        "{outcomes:?}"
    );

    // Ветка ушла в bare-remote вместе с переводом.
    let branches = git(Path::new(&remote), &["branch", "--list"]);
    assert!(branches.contains("llm/translate-en-42"), "{branches}");
    let files = git(
        Path::new(&remote),
        &["ls-tree", "-r", "--name-only", "llm/translate-en-42"],
    );
    assert!(files.contains("content/posts/dns/index.en.md"), "{files}");

    // Тело PR содержит команду, резюме скилла и список файлов.
    let requests = server.received_requests().await.unwrap();
    let created = requests
        .iter()
        .find(|r| r.url.path() == "/repos/acme/blog/pulls")
        .expect("PR создан");
    let body: serde_json::Value = serde_json::from_slice(&created.body).unwrap();
    assert_eq!(body["base"], "feature", "база — head-ветка исходного PR");
    assert_eq!(body["head"], "llm/translate-en-42");
    let text = body["body"].as_str().unwrap();
    assert!(text.contains("/llm translate en"), "{text}");
    assert!(text.contains("Перевёл статью про DNS"), "{text}");
    assert!(text.contains("content/posts/dns/index.en.md"), "{text}");
}

#[tokio::test]
async fn patch_without_changes_is_reported_as_a_comment() {
    let tmp = tempfile::tempdir().unwrap();
    let (remote, head) = fixture_remote(tmp.path());
    let skills = tmp.path().join("skills");
    fixture_skills(&skills);

    let server = MockServer::start().await;
    mock_trigger(&server, "/llm translate en", &remote, &head).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 3 })))
        .expect(1)
        .mount(&server)
        .await;

    // Скилл ничего не изменил, только написал summary.
    let runner = Arc::new(FakeRunner::new(vec![FakeResponse::patch(
        Vec::new(),
        "Всё уже переведено.",
    )]));

    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = Arc::new(PipelineHandler {
        pipeline: pipeline(tmp.path(), &server, &skills, runner),
        outcomes: tokio::sync::Mutex::new(Vec::new()),
    });

    let outcomes =
        run_pipeline_once(store.clone(), trigger(&server, &["translate"]), handler).await;
    assert_eq!(outcomes, vec![Outcome::NoChanges]);
    assert_eq!(store.count_by_status(JobStatus::Done).await.unwrap(), 1);

    let requests = server.received_requests().await.unwrap();
    let comment = requests
        .iter()
        .find(|r| r.url.path() == "/repos/acme/blog/issues/42/comments")
        .expect("комментарий отправлен");
    let body: serde_json::Value = serde_json::from_slice(&comment.body).unwrap();
    let text = body["body"].as_str().unwrap();
    assert!(text.contains("изменений в файлах нет"), "{text}");
    assert!(text.contains("Всё уже переведено"), "{text}");

    // Ветка не создавалась.
    let branches = git(Path::new(&remote), &["branch", "--list"]);
    assert!(!branches.contains("llm/translate"), "{branches}");
}

#[tokio::test]
async fn invalid_model_output_twice_fails_the_job_with_a_comment() {
    let tmp = tempfile::tempdir().unwrap();
    let (remote, head) = fixture_remote(tmp.path());
    let skills = tmp.path().join("skills");
    fixture_skills(&skills);

    let server = MockServer::start().await;
    mock_trigger(&server, "/llm proofread", &remote, &head).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 4 })))
        .expect(1)
        .mount(&server)
        .await;

    let runner = Arc::new(FakeRunner::new(vec![
        FakeResponse::findings("не json"),
        FakeResponse::findings("тоже не json"),
    ]));

    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = Arc::new(PipelineHandler {
        pipeline: pipeline(tmp.path(), &server, &skills, runner.clone()),
        outcomes: tokio::sync::Mutex::new(Vec::new()),
    });

    run_pipeline_once(store.clone(), trigger(&server, &["proofread"]), handler).await;

    assert_eq!(store.count_by_status(JobStatus::Failed).await.unwrap(), 1);
    assert_eq!(runner.call_count(), 2, "одна повторная попытка");

    let requests = server.received_requests().await.unwrap();
    let comment = requests
        .iter()
        .find(|r| r.url.path() == "/repos/acme/blog/issues/42/comments")
        .expect("сообщение об ошибке");
    let body: serde_json::Value = serde_json::from_slice(&comment.body).unwrap();
    let text = body["body"].as_str().unwrap();
    assert!(text.contains("не выполнена"), "{text}");
    assert!(!text.contains("panicked"), "{text}");
    // Отметка о провале.
    let reactions: Vec<String> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/reactions"))
        .map(|r| {
            serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["content"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(reactions, vec!["eyes", "-1"]);
}

#[tokio::test]
async fn skill_without_matching_files_says_nothing_to_do() {
    let tmp = tempfile::tempdir().unwrap();
    let (remote, head) = fixture_remote(tmp.path());
    let skills = tmp.path().join("skills");
    fixture_skills(&skills);
    // Сужаем фильтр так, чтобы ни один файл PR не подошёл.
    std::fs::write(
        skills.join("proofread/skill.toml"),
        "mode = \"review\"\ntools = [\"read\", \"write\"]\nfiles = [\"**/*.rst\"]\n",
    )
    .unwrap();

    let server = MockServer::start().await;
    mock_trigger(&server, "/llm proofread", &remote, &head).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 5 })))
        .expect(1)
        .mount(&server)
        .await;

    // Модель не должна вызываться вовсе.
    let runner = Arc::new(FakeRunner::new(Vec::new()));
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = Arc::new(PipelineHandler {
        pipeline: pipeline(tmp.path(), &server, &skills, runner.clone()),
        outcomes: tokio::sync::Mutex::new(Vec::new()),
    });

    let outcomes =
        run_pipeline_once(store.clone(), trigger(&server, &["proofread"]), handler).await;
    assert_eq!(outcomes, vec![Outcome::NothingToDo]);
    assert_eq!(runner.call_count(), 0, "LLM не вызывалась");

    let requests = server.received_requests().await.unwrap();
    let comment = requests
        .iter()
        .find(|r| r.url.path() == "/repos/acme/blog/issues/42/comments")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&comment.body).unwrap();
    assert!(
        body["body"].as_str().unwrap().contains("Нечего делать"),
        "{body}"
    );
}

#[tokio::test]
async fn patch_from_a_fork_is_refused_before_calling_the_model() {
    let tmp = tempfile::tempdir().unwrap();
    let (remote, head) = fixture_remote(tmp.path());
    let skills = tmp.path().join("skills");
    fixture_skills(&skills);

    let server = MockServer::start().await;
    // head PR — в форке.
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": 1001,
            "body": "/llm translate en",
            "user": { "login": "zhurik", "type": "User" },
            "html_url": "https://github.com/acme/blog/pull/42#issuecomment-1001",
            "issue_url": "https://api.github.com/repos/acme/blog/issues/42",
            "created_at": "2026-09-29T10:00:00Z",
            "updated_at": "2026-09-29T10:00:00Z"
        }])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 42,
            "state": "open",
            "head": {
                "ref": "feature",
                "sha": head,
                "repo": { "full_name": "contributor/blog", "clone_url": remote }
            },
            "base": {
                "ref": "main",
                "sha": "base",
                "repo": { "full_name": "acme/blog", "clone_url": remote }
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/comments/1001/reactions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 6 })))
        .expect(1)
        .mount(&server)
        .await;

    let runner = Arc::new(FakeRunner::new(Vec::new()));
    let store = Store::new(Db::open_in_memory().await.unwrap());
    let handler = Arc::new(PipelineHandler {
        pipeline: pipeline(tmp.path(), &server, &skills, runner.clone()),
        outcomes: tokio::sync::Mutex::new(Vec::new()),
    });

    let outcomes =
        run_pipeline_once(store.clone(), trigger(&server, &["translate"]), handler).await;
    assert!(
        matches!(&outcomes[0], Outcome::Rejected { reason } if reason.contains("форка")),
        "{outcomes:?}"
    );
    assert_eq!(runner.call_count(), 0);

    let requests = server.received_requests().await.unwrap();
    let comment = requests
        .iter()
        .find(|r| r.url.path() == "/repos/acme/blog/issues/42/comments")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&comment.body).unwrap();
    assert!(body["body"].as_str().unwrap().contains("форка"), "{body}");
}
