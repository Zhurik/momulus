//! Тесты, которым нужен настоящий Docker и образ llm-bot-runner с pi.
//!
//! Запуск: `just test-integration` (или `cargo test -- --ignored`).
//! Без `LLM_API_KEY` тест сообщает об этом и завершается успешно.

use std::path::PathBuf;
use std::time::Duration;

use llm_bot_core::config::{ProviderApi, provider_key_env};
use llm_bot_core::{JobId, Mount, Redactor, RunSpec, Runner};
use llm_bot_runner_docker::{DockerRunner, models_json};

fn skills_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills")
}

/// Маленькая статья с явными ошибками — модели есть что найти.
fn fixture_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("posts")).unwrap();
    std::fs::write(
        dir.join("posts/hello.mdx"),
        "---\ntitle: Привет\n---\n\n# Привет\n\nЭто тэкст с ашибкой и лишней  запятой , вот.\n",
    )
    .unwrap();
}

#[tokio::test]
#[ignore = "нужен docker и ключ LLM-провайдера"]
async fn runs_proofread_with_real_pi() {
    let Ok(api_key) = std::env::var("LLM_API_KEY") else {
        eprintln!("пропуск: LLM_API_KEY не задан");
        return;
    };
    let provider = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "cloudru".to_string());
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "zai-org/GLM-5.1".to_string());
    let base_url = std::env::var("LLM_BASE_URL")
        .unwrap_or_else(|_| "https://foundation-models.api.cloud.ru/v1".to_string());
    let key_env = provider_key_env(&provider);
    // Провайдера, которого нет среди встроенных в pi, описываем через models.json.
    let agent_config = models_json(
        &provider,
        &base_url,
        &key_env,
        Some(ProviderApi::OpenaiCompletions),
        &[model.as_str()],
    )
    .expect("описание провайдера");

    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    let out = tmp.path().join("out");
    fixture_repo(&work);
    std::fs::create_dir_all(&out).unwrap();

    let runner = DockerRunner::connect(
        None,
        std::env::var("LLM_BOT_CONTAINER_USER").ok(),
        Redactor::new().with_secret(api_key.clone()),
    )
    .expect("docker доступен");

    let prompt = String::from(
        "/skill:proofread\n\nРабочая копия в /work. Проверь файл posts/hello.mdx.\n\
         Запиши результат инструментом write строго в /out/findings.json: объект с полями \
         summary (строка) и findings (массив объектов path, line, severity, body).\n",
    );

    let spec = RunSpec {
        job_id: JobId::new(),
        skill: "proofread".into(),
        workdir: work,
        skills_dir: skills_dir(),
        out_dir: out.clone(),
        mount: Mount::ReadOnly,
        prompt,
        tools: vec!["read".into(), "grep".into(), "ls".into(), "write".into()],
        provider: provider.clone(),
        model: Some(model),
        timeout: Duration::from_secs(600),
        image: std::env::var("LLM_BOT_RUNNER_IMAGE")
            .unwrap_or_else(|_| "llm-bot-runner:latest".to_string()),
        cpu_limit: 2.0,
        memory_limit_mb: 2048,
        env: vec![(key_env, api_key)],
        agent_config: Some(agent_config),
    };

    let result = runner.run(spec).await.expect("контейнер отработал");
    eprintln!("stdout:\n{}", result.stdout);
    eprintln!("stderr:\n{}", result.stderr);
    assert!(!result.timed_out, "уложились в таймаут");
    assert_eq!(result.exit_code, 0, "pi завершился успешно");

    let findings = out.join("findings.json");
    assert!(findings.exists(), "pi записал /out/findings.json");
    let parsed: llm_bot_core::ReviewOutput =
        serde_json::from_str(&std::fs::read_to_string(&findings).unwrap())
            .expect("валидный JSON по схеме");
    assert!(!parsed.summary.is_empty());
}

#[tokio::test]
#[ignore = "нужен docker"]
async fn kills_container_on_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&out).unwrap();

    let runner = DockerRunner::connect(None, None, Redactor::new()).expect("docker доступен");

    // Образ тот же, но команду подменяем на долгий sleep через переменную окружения pi нет —
    // поэтому используем таймаут в одну секунду на обычном запуске pi: он не успеет.
    let spec = RunSpec {
        job_id: JobId::new(),
        skill: "proofread".into(),
        workdir: work,
        skills_dir: skills_dir(),
        out_dir: out,
        mount: Mount::ReadOnly,
        prompt: "подожди".into(),
        tools: Vec::new(),
        provider: "anthropic".into(),
        model: None,
        timeout: Duration::from_millis(1),
        image: std::env::var("LLM_BOT_RUNNER_IMAGE")
            .unwrap_or_else(|_| "llm-bot-runner:latest".to_string()),
        cpu_limit: 1.0,
        memory_limit_mb: 512,
        env: Vec::new(),
        agent_config: None,
    };

    let result = runner.run(spec).await.expect("контейнер создан");
    assert!(result.timed_out, "таймаут отработал: {result:?}");
}
