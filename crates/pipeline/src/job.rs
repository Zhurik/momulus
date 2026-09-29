//! Orchestration of one job: working copy → skill → runner → validation → publication.

use std::path::PathBuf;
use std::sync::Arc;

use momulus_core::config::{Limits, ProviderApi};
use momulus_core::{
    AckState, Error, GitAccess, Job, JobId, Mount, Publisher, Result, RunSpec, Runner,
};
use momulus_workspace::RepoCache;
use url::Url;

use crate::limits::{check_input, measure_files};
use crate::prompt::{Origin, PromptContext};
use crate::registry::SharedRegistry;
use crate::render::{self, JobContext};
use crate::steps::{collect_patch, run_patch, run_review};
use crate::validate::prepare_review;
use momulus_skills::Mode;

/// Run settings shared by every job.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Directory where working copies are created.
    pub work_dir: PathBuf,
    /// Where the agent logs are written.
    pub logs_dir: PathBuf,
    /// Skills directory, mounted into the container.
    pub skills_dir: PathBuf,
    pub provider: String,
    pub model: String,
    /// Provider protocol, when it has to be described in models.json.
    pub api: Option<ProviderApi>,
    /// Name of the environment variable holding the provider key.
    pub api_key_env: String,
    /// The key itself; it never reaches the logs.
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub image: String,
    pub cpu: f64,
    pub memory_mb: u64,
    pub limits: Limits,
}

/// How the job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The review was published.
    Review { findings: usize },
    /// The patch was pushed and a PR was opened.
    Patch { url: Url },
    /// No file matched the skill's filter.
    NothingToDo,
    /// The skill changed nothing.
    NoChanges,
    /// The command was refused for a clear reason (a fork, an oversized input).
    Rejected { reason: String },
}

/// The job's outcome together with the path to its log.
#[derive(Debug, Clone)]
pub struct JobResult {
    pub outcome: Outcome,
    pub log_path: Option<PathBuf>,
}

/// The pipeline for a single job.
pub struct Pipeline {
    registry: Arc<SharedRegistry>,
    runner: Arc<dyn Runner>,
    publisher: Arc<dyn Publisher>,
    access: Arc<dyn GitAccess>,
    cache: RepoCache,
    config: PipelineConfig,
}

impl Pipeline {
    pub fn new(
        registry: Arc<SharedRegistry>,
        runner: Arc<dyn Runner>,
        publisher: Arc<dyn Publisher>,
        access: Arc<dyn GitAccess>,
        cache: RepoCache,
        config: PipelineConfig,
    ) -> Pipeline {
        Pipeline {
            registry,
            runner,
            publisher,
            access,
            cache,
            config,
        }
    }

    pub fn publisher(&self) -> &Arc<dyn Publisher> {
        &self.publisher
    }

    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    /// Runs the job from start to published result.
    pub async fn execute(&self, job: &Job) -> Result<JobResult> {
        let skill = self
            .registry
            .skill(&job.command.skill)
            .await
            .ok_or_else(|| Error::Skill(format!("skill \"{}\" not found", job.command.skill)))?;

        let args = skill
            .contract
            .resolve_args(&skill.name, &job.command.args)?;
        let command_line = job.command.to_command_line();
        let job_ctx = JobContext {
            job_id: job.id,
            skill: &skill.name,
            command: &command_line,
        };

        // We cannot push into a fork — say so right away, before calling the model.
        if skill.contract.mode == Mode::Patch && job.pr.is_fork() {
            let reason = format!("the PR comes from the fork {}", job.pr.head_repo);
            self.publisher
                .comment(
                    &job.pr,
                    &render::fork_unsupported(&job_ctx, &job.pr.head_repo),
                )
                .await?;
            return Ok(JobResult {
                outcome: Outcome::Rejected { reason },
                log_path: None,
            });
        }

        // A working copy at the PR head commit.
        let token = self.access.git_token(&job.pr).await?;
        let bare = self
            .cache
            .sync(
                &job.pr.owner,
                &job.pr.repo,
                &job.pr.clone_url,
                &self.access.refspecs(&job.pr),
                token.as_deref(),
            )
            .await?;
        let dest = self.config.work_dir.join(job.id.to_string());
        let worktree = self.cache.worktree(&bare, &dest, &job.pr.head_sha).await?;

        // The PR files that match the skill's filter.
        let diff_text = self
            .cache
            .pr_diff(&bare, &self.access.base_rev(&job.pr), &job.pr.head_sha)
            .await?;
        let diff = momulus_workspace::DiffIndex::parse(&diff_text)?;
        let files = skill.contract.select_files(diff.reviewable_files())?;

        if files.is_empty() {
            self.publisher
                .comment(
                    &job.pr,
                    &render::nothing_to_do(&job_ctx, &skill.contract.files),
                )
                .await?;
            return Ok(JobResult {
                outcome: Outcome::NothingToDo,
                log_path: None,
            });
        }

        // Oversized input — refuse without calling the model.
        if let Err(err) = check_input(
            diff_text.len() as u64,
            &measure_files(worktree.path(), &files),
            &self.config.limits,
        ) {
            self.publisher
                .comment(&job.pr, &render::too_large(&job_ctx, &err.to_string()))
                .await?;
            return Ok(JobResult {
                outcome: Outcome::Rejected {
                    reason: err.to_string(),
                },
                log_path: None,
            });
        }

        let out_dir = self.config.work_dir.join(format!("{}-out", job.id));
        std::fs::create_dir_all(&out_dir)?;
        let spec = self.run_spec(job.id, &skill, worktree.path().to_path_buf(), out_dir)?;

        let prompt_ctx = PromptContext {
            skill: &skill,
            files: &files,
            args: &args,
            origin: Origin::PullRequest {
                repo: job.pr.full_name(),
                number: job.pr.number,
            },
        };

        let log_path = self.config.logs_dir.join(format!("{}.log", job.id));
        std::fs::create_dir_all(&self.config.logs_dir)?;

        let result = match skill.contract.mode {
            Mode::Review => {
                let step = run_review(
                    self.runner.as_ref(),
                    spec,
                    &prompt_ctx,
                    Some(log_path.as_path()),
                )
                .await?;

                let review = prepare_review(
                    step.output,
                    &files,
                    Some(&diff),
                    skill.contract.max_comments(),
                );
                let summary = render::review_summary(&review, &job_ctx, files.len());
                self.publisher
                    .post_review(&job.pr, &review.inline, &summary)
                    .await?;
                Outcome::Review {
                    findings: review.kept(),
                }
            }
            Mode::Patch => {
                let step = run_patch(
                    self.runner.as_ref(),
                    spec,
                    &prompt_ctx,
                    Some(log_path.as_path()),
                )
                .await?;

                let branch = skill
                    .contract
                    .branch_name(&skill.name, job.pr.number, &args);
                let body = render::pr_body(&job_ctx, job.pr.number, step.summary.as_deref(), &[]);
                let patch = collect_patch(
                    &worktree,
                    branch,
                    render::pr_title(&skill.name, job.pr.number, &args),
                    body,
                    format!("momulus: {} for #{}", skill.name, job.pr.number),
                )
                .await?;

                match patch {
                    None => {
                        self.publisher
                            .comment(
                                &job.pr,
                                &render::no_changes(&job_ctx, step.summary.as_deref()),
                            )
                            .await?;
                        Outcome::NoChanges
                    }
                    Some(mut patch) => {
                        // The file list is only known once the patch is collected.
                        patch.body = render::pr_body(
                            &job_ctx,
                            job.pr.number,
                            step.summary.as_deref(),
                            &patch.files,
                        );
                        let url = self.publisher.push_and_open_pr(&job.pr, &patch).await?;
                        self.publisher
                            .comment(&job.pr, &format!("The changes are in a separate PR: {url}"))
                            .await?;
                        Outcome::Patch { url }
                    }
                }
            }
        };

        // The working copy is removed here, explicitly, so cleanup errors surface.
        worktree.cleanup().await?;
        let _ = std::fs::remove_dir_all(self.config.work_dir.join(format!("{}-out", job.id)));

        Ok(JobResult {
            outcome: result,
            log_path: Some(log_path),
        })
    }

    /// Marks the job on the platform (a reaction on the command comment).
    pub async fn ack(&self, job: &Job, state: AckState) -> Result<()> {
        self.publisher.ack(&job.job_ref(), state).await
    }

    /// Reports an error to the PR in plain words, with no stack traces or secrets.
    pub async fn report_error(&self, job: &Job, error: &Error) -> Result<()> {
        let command_line = job.command.to_command_line();
        let ctx = JobContext {
            job_id: job.id,
            skill: &job.command.skill,
            command: &command_line,
        };
        self.publisher
            .comment(&job.pr, &render::error_comment(&ctx, &error.to_string()))
            .await
    }

    fn run_spec(
        &self,
        job_id: JobId,
        skill: &momulus_skills::Skill,
        workdir: PathBuf,
        out_dir: PathBuf,
    ) -> Result<RunSpec> {
        let model = if skill.contract.model.is_empty() {
            self.config.model.clone()
        } else {
            skill.contract.model.clone()
        };

        let mut env = Vec::new();
        if let Some(key) = &self.config.api_key {
            env.push((self.config.api_key_env.clone(), key.clone()));
        }

        let agent_config = match (&self.config.base_url, self.config.api) {
            (Some(base_url), api) if !base_url.is_empty() => Some(models_json(
                &self.config.provider,
                base_url,
                &self.config.api_key_env,
                api,
                &model,
            )?),
            _ => None,
        };

        Ok(RunSpec {
            job_id,
            skill: skill.name.clone(),
            workdir,
            skills_dir: self.config.skills_dir.clone(),
            out_dir,
            mount: match skill.contract.mode {
                // In review mode the working copy is read-only: PR content is
                // untrusted input.
                Mode::Review => Mount::ReadOnly,
                Mode::Patch => Mount::ReadWrite,
            },
            prompt: String::new(),
            tools: skill.contract.tools.clone(),
            provider: self.config.provider.clone(),
            model: Some(model),
            timeout: skill.contract.timeout,
            image: self.config.image.clone(),
            cpu_limit: self.config.cpu,
            memory_limit_mb: self.config.memory_mb,
            env,
            agent_config,
        })
    }
}

/// Provider description for pi (the same thing runner-docker does, but without
/// making the pipeline depend on Docker).
fn models_json(
    provider: &str,
    base_url: &str,
    api_key_env: &str,
    api: Option<ProviderApi>,
    model: &str,
) -> Result<String> {
    let mut spec = serde_json::Map::new();
    spec.insert("baseUrl".into(), serde_json::json!(base_url));
    spec.insert(
        "apiKey".into(),
        serde_json::json!(format!("${api_key_env}")),
    );
    if let Some(api) = api {
        spec.insert("api".into(), serde_json::json!(api.as_str()));
        spec.insert("models".into(), serde_json::json!([{ "id": model }]));
    }
    serde_json::to_string_pretty(&serde_json::json!({ "providers": { provider: spec } }))
        .map_err(|e| Error::Internal(e.to_string()))
}
