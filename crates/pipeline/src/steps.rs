//! Шаги пайплайна вокруг единственного вызова модели.

use std::path::Path;

use momulus_core::{Error, Patch, Result, ReviewOutput, RunResult, RunSpec, Runner};
use momulus_workspace::Worktree;

use crate::prompt::{FINDINGS_FILE, PromptContext, SUMMARY_FILE};
use crate::validate::parse_review;

/// Сколько всего попыток делаем на невалидный JSON: первая плюс одна повторная.
pub const MAX_ATTEMPTS: usize = 2;

/// Результат review-шага.
#[derive(Debug, Clone)]
pub struct ReviewStep {
    pub output: ReviewOutput,
    /// Логи всех попыток, в порядке выполнения.
    pub runs: Vec<RunResult>,
}

/// Результат patch-шага.
#[derive(Debug, Clone)]
pub struct PatchStep {
    /// Содержимое `/out/summary.md`, если скилл его написал.
    pub summary: Option<String>,
    pub runs: Vec<RunResult>,
}

impl ReviewStep {
    pub fn attempts(&self) -> usize {
        self.runs.len()
    }
}

/// Запускает review-скилл и разбирает его вывод.
///
/// Если JSON невалиден, делаем ровно одну повторную попытку, передав модели
/// текст ошибки. Вторая неудача — ошибка джобы.
pub async fn run_review(
    runner: &dyn Runner,
    spec: RunSpec,
    ctx: &PromptContext<'_>,
    log_path: Option<&Path>,
) -> Result<ReviewStep> {
    let findings_path = spec.out_dir.join(FINDINGS_FILE);
    let mut runs = Vec::new();
    let mut prompt = ctx.render();

    for attempt in 1..=MAX_ATTEMPTS {
        // Чтобы не принять артефакт предыдущей попытки за новый.
        let _ = std::fs::remove_file(&findings_path);

        let mut attempt_spec = spec.clone();
        attempt_spec.prompt = prompt.clone();
        let result = runner.run(attempt_spec).await?;
        let failure = check_run(&result, &spec);
        runs.push(result);
        // Лог пишем сразу: он нужен и когда попытка провалилась.
        write_log(log_path, &runs);
        if let Some(err) = failure {
            return Err(err);
        }

        match read_findings(&findings_path) {
            Ok(output) => return Ok(ReviewStep { output, runs }),
            Err(err) if attempt < MAX_ATTEMPTS => {
                tracing::warn!(error = %err, "невалидный вывод модели, повторяем один раз");
                prompt = ctx.render_retry(&err.to_string());
            }
            Err(err) => return Err(err),
        }
    }

    Err(Error::InvalidOutput(
        "модель не вернула валидный findings.json".into(),
    ))
}

/// Запускает patch-скилл. Формат вывода здесь не фиксирован, ретраить нечего:
/// результат виден по изменениям в рабочей копии.
pub async fn run_patch(
    runner: &dyn Runner,
    spec: RunSpec,
    ctx: &PromptContext<'_>,
    log_path: Option<&Path>,
) -> Result<PatchStep> {
    let mut attempt_spec = spec.clone();
    attempt_spec.prompt = ctx.render();
    let result = runner.run(attempt_spec).await?;
    let failure = check_run(&result, &spec);
    let runs = vec![result];
    write_log(log_path, &runs);
    if let Some(err) = failure {
        return Err(err);
    }

    Ok(PatchStep {
        summary: read_summary(&spec.out_dir),
        runs,
    })
}

/// Собирает патч из рабочей копии: `None` — скилл ничего не изменил.
///
/// Коммит, ветку и pull request делает Publisher; здесь только факты о файлах.
pub async fn collect_patch(
    worktree: &Worktree,
    branch: String,
    title: String,
    body: String,
    commit_message: String,
) -> Result<Option<Patch>> {
    let files = worktree.dirty_files().await?;
    if files.is_empty() {
        return Ok(None);
    }
    Ok(Some(Patch {
        worktree: worktree.path().to_path_buf(),
        branch,
        commit_message,
        title,
        body,
        files,
    }))
}

/// Превращает неуспешный прогон в ошибку с понятной причиной.
fn check_run(result: &RunResult, spec: &RunSpec) -> Option<Error> {
    if result.timed_out {
        return Some(Error::Timeout(spec.timeout));
    }
    if !result.is_success() {
        let tail = result
            .stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("подробности в логе джобы");
        return Some(Error::Runner(format!(
            "агент завершился с кодом {}: {}",
            result.exit_code,
            tail.trim()
        )));
    }
    None
}

/// Читает и разбирает `findings.json`.
fn read_findings(path: &Path) -> Result<ReviewOutput> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::InvalidOutput(format!(
                "файл {FINDINGS_FILE} не создан"
            )));
        }
        Err(err) => return Err(Error::InvalidOutput(format!("{FINDINGS_FILE}: {err}"))),
    };
    parse_review(&raw)
}

/// Читает `summary.md`, если он есть.
pub fn read_summary(out_dir: &Path) -> Option<String> {
    std::fs::read_to_string(out_dir.join(SUMMARY_FILE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Пишет лог попыток в файл; ошибку записи только логируем.
fn write_log(log_path: Option<&Path>, runs: &[RunResult]) {
    let Some(path) = log_path else { return };
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(error = %err, path = %parent.display(), "каталог логов не создан");
        return;
    }
    if let Err(err) = std::fs::write(path, combined_log(runs)) {
        tracing::warn!(error = %err, path = %path.display(), "лог не записан");
    }
}

/// Склеивает логи всех попыток для файла джобы.
pub fn combined_log(runs: &[RunResult]) -> String {
    let mut out = String::new();
    for (i, run) in runs.iter().enumerate() {
        out.push_str(&format!("=== попытка {} ===\n", i + 1));
        out.push_str(&run.combined_log());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeResponse, FakeRunner};
    use crate::prompt::Origin;
    use momulus_core::{JobId, Mount};
    use momulus_skills::{Skill, SkillContract, SkillDoc};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    fn skill(mode: &str) -> Skill {
        let contract = match mode {
            "review" => "mode = \"review\"\ntools = [\"read\", \"write\"]",
            _ => "mode = \"patch\"\ntools = [\"read\", \"write\", \"edit\"]",
        };
        Skill {
            name: "proofread".into(),
            dir: PathBuf::from("/skills/proofread"),
            contract: SkillContract::parse("proofread", contract).unwrap(),
            doc: SkillDoc {
                name: "proofread".into(),
                description: "описание".into(),
                body: String::new(),
            },
        }
    }

    fn spec(out_dir: PathBuf, work: PathBuf) -> RunSpec {
        RunSpec {
            job_id: JobId::new(),
            skill: "proofread".into(),
            workdir: work,
            skills_dir: PathBuf::from("/skills"),
            out_dir,
            mount: Mount::ReadOnly,
            prompt: String::new(),
            tools: vec!["read".into(), "write".into()],
            provider: "cloudru".into(),
            model: Some("zai-org/GLM-5.1".into()),
            timeout: Duration::from_secs(60),
            image: "momulus-runner:latest".into(),
            cpu_limit: 1.0,
            memory_limit_mb: 512,
            env: Vec::new(),
            agent_config: None,
        }
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        out: PathBuf,
        work: PathBuf,
        files: Vec<String>,
        args: BTreeMap<String, String>,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        Fixture {
            _tmp: tmp,
            out,
            work,
            files: vec!["posts/a.mdx".to_string()],
            args: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn review_succeeds_on_the_first_attempt() {
        let f = fixture();
        let skill = skill("review");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::with_findings(r#"{"summary":"ок","findings":[]}"#);
        let step = run_review(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap();

        assert_eq!(step.attempts(), 1);
        assert_eq!(step.output.summary, "ок");
    }

    #[tokio::test]
    async fn review_retries_once_on_invalid_json() {
        let f = fixture();
        let skill = skill("review");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![
            FakeResponse::findings("извини, не смог"),
            FakeResponse::findings(r#"{"summary":"со второй попытки","findings":[]}"#),
        ]);
        let step = run_review(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap();

        assert_eq!(step.attempts(), 2);
        assert_eq!(step.output.summary, "со второй попытки");

        // Во втором промпте модели сообщили, что именно сломалось.
        let calls = runner.calls();
        assert!(
            calls[1].prompt.contains("не прошла попытка")
                || calls[1].prompt.contains("не прошла валидацию"),
            "{}",
            calls[1].prompt
        );
        assert!(
            calls[1].prompt.contains("expected value"),
            "{}",
            calls[1].prompt
        );
        assert!(calls[0].prompt.len() < calls[1].prompt.len());
    }

    #[tokio::test]
    async fn review_fails_after_the_second_invalid_json() {
        let f = fixture();
        let skill = skill("review");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![
            FakeResponse::findings("не json"),
            FakeResponse::findings(
                r#"{"summary":"s","findings":[{"path":"a","line":1,"severity":"ой","body":"b"}]}"#,
            ),
        ]);
        let err = run_review(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap_err();

        assert!(matches!(err, Error::InvalidOutput(_)), "{err:?}");
        assert_eq!(runner.call_count(), 2, "больше двух попыток не делаем");
    }

    #[tokio::test]
    async fn missing_findings_file_is_retried_then_fails() {
        let f = fixture();
        let skill = skill("review");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![FakeResponse::empty(), FakeResponse::empty()]);
        let err = run_review(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("findings.json не создан"), "{err}");
        assert_eq!(runner.call_count(), 2);
    }

    #[tokio::test]
    async fn stale_artifact_is_not_reused() {
        let f = fixture();
        // Артефакт от прошлой джобы в том же каталоге.
        std::fs::write(
            f.out.join(FINDINGS_FILE),
            r#"{"summary":"старое","findings":[]}"#,
        )
        .unwrap();

        let skill = skill("review");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![FakeResponse::empty(), FakeResponse::empty()]);
        let err = run_review(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("не создан"), "{err}");
    }

    #[tokio::test]
    async fn timeout_is_reported_without_retry() {
        let f = fixture();
        let skill = skill("review");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![FakeResponse::empty().with_result(RunResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        })]);
        let err = run_review(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap_err();

        assert!(matches!(err, Error::Timeout(_)), "{err:?}");
        assert_eq!(runner.call_count(), 1);
    }

    #[tokio::test]
    async fn nonzero_exit_code_mentions_stderr_tail() {
        let f = fixture();
        let skill = skill("review");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![FakeResponse::empty().with_result(RunResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "401 invalid api key\n".into(),
            timed_out: false,
        })]);
        let err = run_review(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("401 invalid api key"), "{err}");
        assert_eq!(runner.call_count(), 1);
    }

    #[tokio::test]
    async fn patch_step_reads_summary() {
        let f = fixture();
        let skill = skill("patch");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![FakeResponse::patch(
            vec![("posts/a.en.mdx".into(), "Hello\n".into())],
            "  перевёл одну статью\n",
        )]);
        let mut spec = spec(f.out.clone(), f.work.clone());
        spec.mount = Mount::ReadWrite;
        let step = run_patch(&runner, spec, &ctx, None).await.unwrap();

        assert_eq!(step.summary.as_deref(), Some("перевёл одну статью"));
        assert!(f.work.join("posts/a.en.mdx").exists());
    }

    #[tokio::test]
    async fn patch_step_without_summary() {
        let f = fixture();
        let skill = skill("patch");
        let ctx = PromptContext {
            skill: &skill,
            files: &f.files,
            args: &f.args,
            origin: Origin::Local {
                path: "/tmp/repo".into(),
            },
        };
        let runner = FakeRunner::new(vec![FakeResponse::empty()]);
        let step = run_patch(&runner, spec(f.out.clone(), f.work.clone()), &ctx, None)
            .await
            .unwrap();
        assert!(step.summary.is_none());
    }

    #[test]
    fn combined_log_numbers_attempts() {
        let runs = vec![
            RunResult {
                exit_code: 0,
                stdout: "раз".into(),
                stderr: String::new(),
                timed_out: false,
            },
            RunResult {
                exit_code: 0,
                stdout: "два".into(),
                stderr: String::new(),
                timed_out: false,
            },
        ];
        let log = combined_log(&runs);
        assert!(log.contains("=== попытка 1 ==="), "{log}");
        assert!(log.contains("=== попытка 2 ==="), "{log}");
        assert!(log.contains("раз") && log.contains("два"));
    }
}
