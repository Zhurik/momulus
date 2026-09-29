//! Tests that need real Docker and the momulus-runner image with pi.
//!
//! Run with `just test-integration` (or `cargo test -- --ignored`).
//! Without `LLM_API_KEY` the test says so and passes.

use std::path::PathBuf;
use std::time::Duration;

use std::collections::BTreeMap;

use momulus_core::config::{ProviderApi, provider_key_env};
use momulus_core::{JobId, Mount, Redactor, RunSpec, Runner};
use momulus_pipeline::prompt::{Origin, PromptContext};
use momulus_runner_docker::{DockerRunner, models_json};
use momulus_skills::Registry;

fn skills_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills")
}

/// A tiny post with obvious mistakes — something for the model to find.
fn fixture_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("posts")).unwrap();
    std::fs::write(
        dir.join("posts/hello.mdx"),
        "---\ntitle: Hello\n---\n\n# Hello\n\nThis is a txt with a tpyo and an extra  comma , right here.\n",
    )
    .unwrap();
}

#[tokio::test]
#[ignore = "needs docker and an LLM provider key"]
async fn runs_proofread_with_real_pi() {
    let Ok(api_key) = std::env::var("LLM_API_KEY") else {
        eprintln!("skipped: LLM_API_KEY is not set");
        return;
    };
    let provider = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".to_string());
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-5.1".to_string());
    let key_env = provider_key_env(&provider);
    // With LLM_BASE_URL set we treat the provider as an OpenAI-compatible gateway
    // pi does not know about, and describe it through models.json.
    let agent_config = match std::env::var("LLM_BASE_URL") {
        Ok(base_url) if !base_url.is_empty() => Some(
            models_json(
                &provider,
                &base_url,
                &key_env,
                Some(ProviderApi::OpenaiCompletions),
                &[model.as_str()],
            )
            .expect("provider description"),
        ),
        _ => None,
    };

    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    let out = tmp.path().join("out");
    fixture_repo(&work);
    std::fs::create_dir_all(&out).unwrap();

    let runner = DockerRunner::connect(
        None,
        std::env::var("MOMULUS_CONTAINER_USER").ok(),
        Redactor::new().with_secret(api_key.clone()),
    )
    .expect("docker is available");

    // The very prompt the pipeline sends in production, including the JSON Schema:
    // this is what makes the model return schema-conformant severities.
    let registry = Registry::load(&skills_dir()).expect("the shipped skills are valid");
    let skill = registry
        .get("proofread")
        .expect("the proofread skill exists");
    let files = vec!["posts/hello.mdx".to_string()];
    let prompt = PromptContext {
        skill,
        files: &files,
        args: &BTreeMap::new(),
        origin: Origin::Local {
            path: "/work".to_string(),
        },
    }
    .render();

    let spec = RunSpec {
        job_id: JobId::new(),
        skill: "proofread".into(),
        workdir: work,
        skills_dir: skills_dir(),
        out_dir: out.clone(),
        mount: Mount::ReadOnly,
        prompt,
        tools: skill.contract.tools.clone(),
        provider: provider.clone(),
        model: Some(model),
        timeout: skill.contract.timeout,
        image: std::env::var("MOMULUS_RUNNER_IMAGE")
            .unwrap_or_else(|_| "momulus-runner:latest".to_string()),
        cpu_limit: 2.0,
        memory_limit_mb: 2048,
        env: vec![(key_env, api_key)],
        agent_config,
    };

    let result = runner.run(spec).await.expect("the container ran");
    eprintln!("stdout:\n{}", result.stdout);
    eprintln!("stderr:\n{}", result.stderr);
    assert!(!result.timed_out, "finished within the timeout");
    assert_eq!(result.exit_code, 0, "pi exited successfully");

    let findings = out.join("findings.json");
    assert!(findings.exists(), "pi wrote /out/findings.json");
    let parsed: momulus_core::ReviewOutput =
        serde_json::from_str(&std::fs::read_to_string(&findings).unwrap())
            .expect("valid JSON matching the schema");
    assert!(!parsed.summary.is_empty());
}

#[tokio::test]
#[ignore = "needs docker"]
async fn kills_container_on_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&out).unwrap();

    let runner = DockerRunner::connect(None, None, Redactor::new()).expect("docker is available");

    // The image is the same; we simply give a normal pi run a one-millisecond
    // timeout, which it cannot possibly meet.
    let spec = RunSpec {
        job_id: JobId::new(),
        skill: "proofread".into(),
        workdir: work,
        skills_dir: skills_dir(),
        out_dir: out,
        mount: Mount::ReadOnly,
        prompt: "wait".into(),
        tools: Vec::new(),
        provider: "anthropic".into(),
        model: None,
        timeout: Duration::from_millis(1),
        image: std::env::var("MOMULUS_RUNNER_IMAGE")
            .unwrap_or_else(|_| "momulus-runner:latest".to_string()),
        cpu_limit: 1.0,
        memory_limit_mb: 512,
        env: Vec::new(),
        agent_config: None,
    };

    let result = runner.run(spec).await.expect("the container was created");
    assert!(result.timed_out, "the timeout fired: {result:?}");
}
