//! Оркестрация одной джобы: рабочая копия → скилл → runner → валидация → публикация.

use std::path::PathBuf;
use std::sync::Arc;

use llm_bot_core::config::{Limits, ProviderApi};
use llm_bot_core::{
    AckState, Error, GitAccess, Job, JobId, Mount, Publisher, Result, RunSpec, Runner,
};
use llm_bot_workspace::RepoCache;
use url::Url;

use crate::limits::{check_input, measure_files};
use crate::prompt::{Origin, PromptContext};
use crate::registry::SharedRegistry;
use crate::render::{self, JobContext};
use crate::steps::{collect_patch, run_patch, run_review};
use crate::validate::prepare_review;
use llm_bot_skills::Mode;

/// Настройки запуска, одинаковые для всех джоб.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Каталог, в котором создаются рабочие копии.
    pub work_dir: PathBuf,
    /// Куда пишем логи агента.
    pub logs_dir: PathBuf,
    /// Каталог скиллов, монтируется в контейнер.
    pub skills_dir: PathBuf,
    pub provider: String,
    pub model: String,
    /// Протокол провайдера, если его нужно описать в models.json.
    pub api: Option<ProviderApi>,
    /// Имя переменной окружения с ключом провайдера.
    pub api_key_env: String,
    /// Сам ключ; в логи не попадает.
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub image: String,
    pub cpu: f64,
    pub memory_mb: u64,
    pub limits: Limits,
}

/// Чем закончилась джоба.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Ревью опубликовано.
    Review { findings: usize },
    /// Патч запушен, PR открыт.
    Patch { url: Url },
    /// Под фильтр скилла не попало ни одного файла.
    NothingToDo,
    /// Скилл ничего не изменил.
    NoChanges,
    /// Команда отклонена по понятной причине (форк, слишком большой вход).
    Rejected { reason: String },
}

/// Результат выполнения джобы вместе с путём к логу.
#[derive(Debug, Clone)]
pub struct JobResult {
    pub outcome: Outcome,
    pub log_path: Option<PathBuf>,
}

/// Пайплайн одной джобы.
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

    /// Выполняет джобу от начала до публикации результата.
    pub async fn execute(&self, job: &Job) -> Result<JobResult> {
        let skill = self
            .registry
            .skill(&job.command.skill)
            .await
            .ok_or_else(|| Error::Skill(format!("скилл \"{}\" не найден", job.command.skill)))?;

        let args = skill
            .contract
            .resolve_args(&skill.name, &job.command.args)?;
        let command_line = job.command.to_command_line();
        let job_ctx = JobContext {
            job_id: job.id,
            skill: &skill.name,
            command: &command_line,
        };

        // Патч в форк не запушить — говорим сразу, не тратя вызов модели.
        if skill.contract.mode == Mode::Patch && job.pr.is_fork() {
            let reason = format!("PR из форка {}", job.pr.head_repo);
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

        // Рабочая копия на head-коммите PR.
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

        // Файлы PR под фильтром скилла.
        let diff_text = self
            .cache
            .pr_diff(&bare, &self.access.base_rev(&job.pr), &job.pr.head_sha)
            .await?;
        let diff = llm_bot_workspace::DiffIndex::parse(&diff_text)?;
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

        // Слишком большой вход — отказ без вызова модели.
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
                    format!("llm-bot: {} для #{}", skill.name, job.pr.number),
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
                        // Список файлов известен только после сборки патча.
                        patch.body = render::pr_body(
                            &job_ctx,
                            job.pr.number,
                            step.summary.as_deref(),
                            &patch.files,
                        );
                        let url = self.publisher.push_and_open_pr(&job.pr, &patch).await?;
                        self.publisher
                            .comment(&job.pr, &format!("Изменения в отдельном PR: {url}"))
                            .await?;
                        Outcome::Patch { url }
                    }
                }
            }
        };

        // Рабочая копия удаляется здесь, явно, чтобы поймать ошибки очистки.
        worktree.cleanup().await?;
        let _ = std::fs::remove_dir_all(self.config.work_dir.join(format!("{}-out", job.id)));

        Ok(JobResult {
            outcome: result,
            log_path: Some(log_path),
        })
    }

    /// Отмечает джобу на платформе (реакция на комментарий-команду).
    pub async fn ack(&self, job: &Job, state: AckState) -> Result<()> {
        self.publisher.ack(&job.job_ref(), state).await
    }

    /// Сообщает об ошибке в PR понятным текстом, без стектрейсов и секретов.
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
        skill: &llm_bot_skills::Skill,
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
                // В review-режиме рабочая копия только для чтения: содержимое PR —
                // недоверенный ввод.
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

/// Описание провайдера для pi (то же, что делает runner-docker, но без
/// зависимости пайплайна от докера).
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
