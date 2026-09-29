//! Runner: the single LLM step runs as a container with pi inside.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, KillContainerOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, StartContainerOptions, WaitContainerOptionsBuilder,
};
use futures::StreamExt;
use momulus_core::{Error, Mount, ProviderApi, Redactor, Result, RunResult, RunSpec, Runner};

/// Where the working copy is mounted.
pub const WORK_MOUNT: &str = "/work";
/// Where the skills directory is mounted.
pub const SKILLS_MOUNT: &str = "/skills";
/// Where the container writes its artifacts.
pub const OUT_MOUNT: &str = "/out";
/// Subdirectory of /out where pi's models.json is placed.
const AGENT_DIR: &str = ".pi-agent";

/// Runs pi in Docker through bollard.
pub struct DockerRunner {
    docker: Docker,
    redactor: Redactor,
    /// Container user ("1000:1000"); None means the image default.
    user: Option<String>,
}

impl std::fmt::Debug for DockerRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerRunner")
            .field("user", &self.user)
            .finish()
    }
}

impl DockerRunner {
    /// Connects to the daemon: at the configured address or the default one.
    pub fn connect(host: Option<&str>, user: Option<String>, redactor: Redactor) -> Result<Self> {
        let docker = match host {
            Some(host) if !host.is_empty() => {
                Docker::connect_with_socket(host, 120, bollard::API_DEFAULT_VERSION)
                    .map_err(|e| Error::Runner(format!("docker {host}: {e}")))?
            }
            _ => Docker::connect_with_defaults()
                .map_err(|e| Error::Runner(format!("docker: {e}")))?,
        };
        Ok(DockerRunner {
            docker,
            redactor,
            user,
        })
    }

    /// Checks that the image with pi has been built.
    pub async fn ensure_image(&self, image: &str) -> Result<()> {
        self.docker.inspect_image(image).await.map_err(|e| {
            Error::Runner(format!(
                "image {image} is unavailable ({e}); build it with: just build-images"
            ))
        })?;
        Ok(())
    }

    /// The command handed to the container.
    fn command(spec: &RunSpec) -> Vec<String> {
        let mut cmd = vec![
            "pi".to_string(),
            "--print".to_string(),
            "--no-session".to_string(),
            "--no-extensions".to_string(),
            "--no-context-files".to_string(),
            "--provider".to_string(),
            spec.provider.clone(),
        ];
        if let Some(model) = &spec.model
            && !model.is_empty()
        {
            cmd.push("--model".to_string());
            cmd.push(model.clone());
        }
        if spec.tools.is_empty() {
            cmd.push("--no-tools".to_string());
        } else {
            cmd.push("--tools".to_string());
            cmd.push(spec.tools.join(","));
        }
        cmd.push("--skill".to_string());
        cmd.push(format!("{SKILLS_MOUNT}/{}", spec.skill));
        // End of options: the prompt may start with anything.
        cmd.push("--".to_string());
        cmd.push(spec.prompt.clone());
        cmd
    }

    fn env(spec: &RunSpec, agent_dir: Option<&str>) -> Vec<String> {
        let mut env: Vec<String> = vec![
            "PI_SKIP_VERSION_CHECK=1".to_string(),
            "PI_TELEMETRY=0".to_string(),
        ];
        if let Some(dir) = agent_dir {
            env.push(format!("PI_CODING_AGENT_DIR={dir}"));
        }
        for (key, value) in &spec.env {
            env.push(format!("{key}={value}"));
        }
        env
    }

    fn binds(spec: &RunSpec) -> Vec<String> {
        let work_mode = match spec.mount {
            Mount::ReadOnly => ":ro",
            Mount::ReadWrite => ":rw",
        };
        vec![
            format!("{}:{WORK_MOUNT}{work_mode}", spec.workdir.display()),
            format!("{}:{SKILLS_MOUNT}:ro", spec.skills_dir.display()),
            format!("{}:{OUT_MOUNT}:rw", spec.out_dir.display()),
        ]
    }

    /// Writes models.json next to the artifacts when the provider needs its own base URL.
    fn write_agent_config(spec: &RunSpec) -> Result<Option<String>> {
        let Some(config) = &spec.agent_config else {
            return Ok(None);
        };
        let dir = spec.out_dir.join(AGENT_DIR);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("models.json"), config)?;
        Ok(Some(format!("{OUT_MOUNT}/{AGENT_DIR}")))
    }

    async fn remove_container(&self, id: &str) {
        let options = RemoveContainerOptionsBuilder::default()
            .force(true)
            .v(true)
            .build();
        if let Err(err) = self.docker.remove_container(id, Some(options)).await {
            tracing::warn!(container = id, error = %err, "the container was not removed");
        }
    }
}

#[async_trait]
impl Runner for DockerRunner {
    async fn run(&self, spec: RunSpec) -> Result<RunResult> {
        self.ensure_image(&spec.image).await?;
        std::fs::create_dir_all(&spec.out_dir)?;
        let agent_dir = Self::write_agent_config(&spec)?;

        let host_config = HostConfig {
            binds: Some(Self::binds(&spec)),
            memory: Some((spec.memory_limit_mb as i64) * 1024 * 1024),
            nano_cpus: Some((spec.cpu_limit * 1e9) as i64),
            pids_limit: Some(512),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges".to_string()]),
            // Networking is required: pi talks to the provider API.
            network_mode: Some("bridge".to_string()),
            auto_remove: Some(false),
            init: Some(true),
            ..Default::default()
        };

        let labels = HashMap::from([
            ("momulus.job".to_string(), spec.job_id.to_string()),
            ("momulus.skill".to_string(), spec.skill.clone()),
        ]);

        let body = ContainerCreateBody {
            image: Some(spec.image.clone()),
            cmd: Some(Self::command(&spec)),
            env: Some(Self::env(&spec, agent_dir.as_deref())),
            working_dir: Some(WORK_MOUNT.to_string()),
            user: self.user.clone(),
            labels: Some(labels),
            host_config: Some(host_config),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(false),
            ..Default::default()
        };

        let name = format!("momulus-{}", spec.job_id.short());
        let options = CreateContainerOptionsBuilder::default().name(&name).build();
        let created = self
            .docker
            .create_container(Some(options), body)
            .await
            .map_err(|e| Error::Runner(self.redactor.redact(&format!("create container: {e}"))))?;
        let id = created.id;

        let result = self.run_container(&id, &spec).await;
        self.remove_container(&id).await;
        result
    }
}

impl DockerRunner {
    /// Starts the container, collects logs and waits for it with a timeout.
    async fn run_container(&self, id: &str, spec: &RunSpec) -> Result<RunResult> {
        self.docker
            .start_container(id, None::<StartContainerOptions>)
            .await
            .map_err(|e| Error::Runner(self.redactor.redact(&format!("start container: {e}"))))?;

        let logs_options = LogsOptionsBuilder::default()
            .follow(true)
            .stdout(true)
            .stderr(true)
            .build();
        let mut logs = self.docker.logs(id, Some(logs_options));

        let redactor = self.redactor.clone();
        let logs_task = tokio::spawn(async move {
            let mut stdout = String::new();
            let mut stderr = String::new();
            while let Some(item) = logs.next().await {
                match item {
                    Ok(bollard::container::LogOutput::StdOut { message })
                    | Ok(bollard::container::LogOutput::Console { message }) => {
                        stdout.push_str(&String::from_utf8_lossy(&message));
                    }
                    Ok(bollard::container::LogOutput::StdErr { message }) => {
                        stderr.push_str(&String::from_utf8_lossy(&message));
                    }
                    Ok(bollard::container::LogOutput::StdIn { .. }) => {}
                    Err(err) => {
                        stderr.push_str(&format!("\n[logs] {err}\n"));
                        break;
                    }
                }
            }
            (redactor.redact(&stdout), redactor.redact(&stderr))
        });

        let wait_options = WaitContainerOptionsBuilder::default()
            .condition("not-running")
            .build();
        let mut wait = self.docker.wait_container(id, Some(wait_options));

        let waited = tokio::time::timeout(spec.timeout, wait.next()).await;

        let timed_out = match waited {
            Ok(_) => false,
            Err(_) => {
                tracing::warn!(container = id, "timed out, killing the container");
                let options = KillContainerOptionsBuilder::default()
                    .signal("SIGKILL")
                    .build();
                let _ = self.docker.kill_container(id, Some(options)).await;
                true
            }
        };

        let exit_code = match waited {
            // bollard reports a non-zero code as an error — dig it out of there.
            Ok(Some(Ok(response))) => response.status_code,
            Ok(Some(Err(bollard::errors::Error::DockerContainerWaitError { code, .. }))) => code,
            Ok(Some(Err(err))) => {
                return Err(Error::Runner(
                    self.redactor.redact(&format!("wait container: {err}")),
                ));
            }
            Ok(None) => 0,
            Err(_) => -1,
        };

        let (stdout, stderr) = logs_task
            .await
            .map_err(|e| Error::Runner(format!("log collection: {e}")))?;

        Ok(RunResult {
            exit_code,
            stdout,
            stderr,
            timed_out,
        })
    }
}

/// Generates pi's `models.json`: a description of a provider that is not built
/// into pi (an OpenAI-compatible gateway, say) plus its model list.
///
/// The key never reaches the file: pi substitutes it from the environment.
pub fn models_json(
    provider: &str,
    base_url: &str,
    api_key_env: &str,
    api: Option<ProviderApi>,
    models: &[&str],
) -> Result<String> {
    if base_url.is_empty() {
        return Err(Error::Config("base URL is empty".into()));
    }
    let mut spec = serde_json::Map::new();
    spec.insert("baseUrl".into(), serde_json::json!(base_url));
    spec.insert(
        "apiKey".into(),
        serde_json::json!(format!("${api_key_env}")),
    );
    if let Some(api) = api {
        spec.insert("api".into(), serde_json::json!(api.as_str()));
    }
    if !models.is_empty() {
        spec.insert(
            "models".into(),
            serde_json::json!(
                models
                    .iter()
                    .map(|id| serde_json::json!({ "id": id }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    let value = serde_json::json!({ "providers": { provider: spec } });
    serde_json::to_string_pretty(&value).map_err(|e| Error::Internal(e.to_string()))
}

/// Checks that the path is absolute: Docker treats relative paths as volume names.
pub fn require_absolute(path: &Path, what: &str) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::Runner(format!(
            "{what} must be an absolute path, got {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use momulus_core::{JobId, Mount};
    use std::path::PathBuf;
    use std::time::Duration;

    fn spec() -> RunSpec {
        RunSpec {
            job_id: JobId::new(),
            skill: "proofread".into(),
            workdir: PathBuf::from("/data/work/job"),
            skills_dir: PathBuf::from("/srv/skills"),
            out_dir: PathBuf::from("/data/work/job-out"),
            mount: Mount::ReadOnly,
            prompt: "/skill:proofread ...".into(),
            tools: vec!["read".into(), "grep".into()],
            provider: "anthropic".into(),
            model: Some("claude-sonnet-5".into()),
            timeout: Duration::from_secs(600),
            image: "momulus-runner:latest".into(),
            cpu_limit: 2.0,
            memory_limit_mb: 2048,
            env: vec![("ANTHROPIC_API_KEY".into(), "sk-ant-secret".into())],
            agent_config: None,
        }
    }

    #[test]
    fn command_matches_pi_flags() {
        let cmd = DockerRunner::command(&spec());
        assert_eq!(
            cmd,
            vec![
                "pi",
                "--print",
                "--no-session",
                "--no-extensions",
                "--no-context-files",
                "--provider",
                "anthropic",
                "--model",
                "claude-sonnet-5",
                "--tools",
                "read,grep",
                "--skill",
                "/skills/proofread",
                "--",
                "/skill:proofread ...",
            ]
        );
    }

    #[test]
    fn command_without_model_uses_provider_default() {
        let mut spec = spec();
        spec.model = None;
        let cmd = DockerRunner::command(&spec);
        assert!(!cmd.contains(&"--model".to_string()), "{cmd:?}");
    }

    #[test]
    fn empty_tools_disable_tools() {
        let mut spec = spec();
        spec.tools.clear();
        let cmd = DockerRunner::command(&spec);
        assert!(cmd.contains(&"--no-tools".to_string()), "{cmd:?}");
    }

    #[test]
    fn review_mounts_workdir_read_only() {
        let binds = DockerRunner::binds(&spec());
        assert_eq!(binds[0], "/data/work/job:/work:ro");
        assert_eq!(binds[1], "/srv/skills:/skills:ro");
        assert_eq!(binds[2], "/data/work/job-out:/out:rw");
    }

    #[test]
    fn patch_mounts_workdir_read_write() {
        let mut spec = spec();
        spec.mount = Mount::ReadWrite;
        assert_eq!(DockerRunner::binds(&spec)[0], "/data/work/job:/work:rw");
    }

    #[test]
    fn env_contains_only_provider_key_and_pi_settings() {
        let env = DockerRunner::env(&spec(), None);
        assert!(env.contains(&"ANTHROPIC_API_KEY=sk-ant-secret".to_string()));
        assert!(env.contains(&"PI_TELEMETRY=0".to_string()));
        // No platform tokens may end up inside the container.
        assert!(
            !env.iter().any(|e| e.contains("GITHUB")),
            "the GitHub token must not reach the container: {env:?}"
        );
    }

    #[test]
    fn agent_dir_is_exported_when_needed() {
        let env = DockerRunner::env(&spec(), Some("/out/.pi-agent"));
        assert!(env.contains(&"PI_CODING_AGENT_DIR=/out/.pi-agent".to_string()));
    }

    #[test]
    fn models_json_describes_an_openai_compatible_provider() {
        let json = models_json(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "OPENROUTER_API_KEY",
            Some(ProviderApi::OpenaiCompletions),
            &["some-vendor/some-model"],
        )
        .unwrap();
        assert!(
            json.contains("\"baseUrl\": \"https://openrouter.ai/api/v1\""),
            "{json}"
        );
        assert!(json.contains("\"api\": \"openai-completions\""), "{json}");
        assert!(
            json.contains("\"id\": \"some-vendor/some-model\""),
            "{json}"
        );
        // pi substitutes the key from the environment: the file only names the variable.
        assert!(json.contains("$OPENROUTER_API_KEY"), "{json}");
    }

    #[test]
    fn models_json_for_builtin_provider_only_overrides_endpoint() {
        let json = models_json(
            "anthropic",
            "https://proxy.local/v1",
            "ANTHROPIC_API_KEY",
            None,
            &[],
        )
        .unwrap();
        assert!(json.contains("baseUrl"), "{json}");
        assert!(json.contains("$ANTHROPIC_API_KEY"), "{json}");
        assert!(!json.contains("\"api\""), "{json}");
        assert!(!json.contains("models"), "{json}");
    }

    #[test]
    fn models_json_requires_base_url() {
        assert!(models_json("openrouter", "", "K", None, &[]).is_err());
    }

    #[test]
    fn agent_config_is_written_next_to_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let mut spec = spec();
        spec.out_dir = tmp.path().to_path_buf();
        spec.agent_config = Some("{\"providers\":{}}".to_string());
        let dir = DockerRunner::write_agent_config(&spec).unwrap();
        assert_eq!(dir.as_deref(), Some("/out/.pi-agent"));
        assert!(tmp.path().join(".pi-agent/models.json").exists());
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert!(require_absolute(Path::new("relative/path"), "workdir").is_err());
        assert!(require_absolute(Path::new("/abs"), "workdir").is_ok());
    }
}
