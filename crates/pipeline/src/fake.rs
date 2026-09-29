//! Runner-заглушка: раскладывает заранее заданные артефакты в `/out`.
//!
//! Используется в тестах пайплайна и в `llm-bot run --fake-runner`.

use std::sync::Mutex;

use async_trait::async_trait;
use llm_bot_core::{Error, Result, RunResult, RunSpec, Runner};

/// Что «сделает» модель на очередном вызове.
#[derive(Debug, Clone)]
pub struct FakeResponse {
    /// Файлы, которые появятся в out_dir: (относительный путь, содержимое).
    pub out_files: Vec<(String, String)>,
    /// Файлы, которые появятся в рабочей копии (для patch-скиллов).
    pub work_files: Vec<(String, String)>,
    pub result: RunResult,
}

impl FakeResponse {
    /// Успешный прогон, который записал `findings.json`.
    pub fn findings(json: impl Into<String>) -> FakeResponse {
        FakeResponse {
            out_files: vec![("findings.json".to_string(), json.into())],
            work_files: Vec::new(),
            result: ok_result(),
        }
    }

    /// Успешный прогон patch-скилла: правки в рабочей копии плюс summary.
    pub fn patch(work_files: Vec<(String, String)>, summary: impl Into<String>) -> FakeResponse {
        FakeResponse {
            out_files: vec![("summary.md".to_string(), summary.into())],
            work_files,
            result: ok_result(),
        }
    }

    /// Прогон, который ничего не изменил.
    pub fn empty() -> FakeResponse {
        FakeResponse {
            out_files: Vec::new(),
            work_files: Vec::new(),
            result: ok_result(),
        }
    }

    pub fn with_result(mut self, result: RunResult) -> FakeResponse {
        self.result = result;
        self
    }
}

fn ok_result() -> RunResult {
    RunResult {
        exit_code: 0,
        stdout: "fake runner: готово\n".to_string(),
        stderr: String::new(),
        timed_out: false,
    }
}

/// Runner для тестов: отдаёт заготовленные ответы по очереди.
#[derive(Debug)]
pub struct FakeRunner {
    responses: Mutex<Vec<FakeResponse>>,
    calls: Mutex<Vec<RunSpec>>,
}

impl FakeRunner {
    /// Ответы отдаются в порядке, в котором переданы.
    pub fn new(responses: Vec<FakeResponse>) -> FakeRunner {
        FakeRunner {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Один успешный ответ с находками.
    pub fn with_findings(json: impl Into<String>) -> FakeRunner {
        FakeRunner::new(vec![FakeResponse::findings(json)])
    }

    /// Спеки всех состоявшихся вызовов — по ним тесты проверяют промпт и монтирование.
    pub fn calls(&self) -> Vec<RunSpec> {
        self.calls.lock().expect("mutex").clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("mutex").len()
    }

    /// Копирует все файлы каталога в ответ — для `llm-bot run --fake-runner <dir>`.
    pub fn from_dir(dir: &std::path::Path) -> Result<FakeRunner> {
        let mut out_files = Vec::new();
        collect(dir, dir, &mut out_files)?;
        Ok(FakeRunner::new(vec![FakeResponse {
            out_files,
            work_files: Vec::new(),
            result: ok_result(),
        }]))
    }
}

fn collect(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, String)>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| Error::Internal(e.to_string()))?
                .to_string_lossy()
                .to_string();
            out.push((rel, std::fs::read_to_string(&path)?));
        }
    }
    Ok(())
}

#[async_trait]
impl Runner for FakeRunner {
    async fn run(&self, spec: RunSpec) -> Result<RunResult> {
        let response = {
            let mut responses = self.responses.lock().expect("mutex");
            responses.pop().ok_or_else(|| {
                Error::Runner("FakeRunner: ответы закончились, а его снова вызвали".into())
            })?
        };

        write_files(&spec.out_dir, &response.out_files)?;
        write_files(&spec.workdir, &response.work_files)?;

        self.calls.lock().expect("mutex").push(spec);
        Ok(response.result)
    }
}

fn write_files(root: &std::path::Path, files: &[(String, String)]) -> Result<()> {
    for (rel, content) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_bot_core::{JobId, Mount};
    use std::time::Duration;

    fn spec(out: std::path::PathBuf, work: std::path::PathBuf) -> RunSpec {
        RunSpec {
            job_id: JobId::new(),
            skill: "proofread".into(),
            workdir: work,
            skills_dir: std::path::PathBuf::from("/skills"),
            out_dir: out,
            mount: Mount::ReadOnly,
            prompt: "prompt".into(),
            tools: vec!["read".into()],
            provider: "anthropic".into(),
            model: None,
            timeout: Duration::from_secs(60),
            image: "img".into(),
            cpu_limit: 1.0,
            memory_limit_mb: 512,
            env: Vec::new(),
            agent_config: None,
        }
    }

    #[tokio::test]
    async fn writes_artifacts_and_records_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();

        let runner = FakeRunner::with_findings(r#"{"summary":"ok","findings":[]}"#);
        let result = runner.run(spec(out.clone(), work)).await.unwrap();

        assert!(result.is_success());
        assert_eq!(
            std::fs::read_to_string(out.join("findings.json")).unwrap(),
            r#"{"summary":"ok","findings":[]}"#
        );
        assert_eq!(runner.call_count(), 1);
        assert_eq!(runner.calls()[0].skill, "proofread");
    }

    #[tokio::test]
    async fn responses_are_returned_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();

        let runner = FakeRunner::new(vec![
            FakeResponse::findings("не json"),
            FakeResponse::findings(r#"{"summary":"со второй попытки","findings":[]}"#),
        ]);
        runner.run(spec(out.clone(), work.clone())).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("findings.json")).unwrap(),
            "не json"
        );
        runner.run(spec(out.clone(), work.clone())).await.unwrap();
        assert!(
            std::fs::read_to_string(out.join("findings.json"))
                .unwrap()
                .contains("со второй попытки")
        );

        let err = runner.run(spec(out, work)).await.unwrap_err();
        assert!(err.to_string().contains("ответы закончились"), "{err}");
    }

    #[tokio::test]
    async fn patch_response_touches_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();

        let runner = FakeRunner::new(vec![FakeResponse::patch(
            vec![("posts/hello.en.mdx".into(), "Hello\n".into())],
            "перевёл одну статью",
        )]);
        runner.run(spec(out.clone(), work.clone())).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(work.join("posts/hello.en.mdx")).unwrap(),
            "Hello\n"
        );
        assert!(out.join("summary.md").exists());
    }

    #[tokio::test]
    async fn from_dir_picks_up_prepared_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let prepared = tmp.path().join("prepared");
        std::fs::create_dir_all(prepared.join("nested")).unwrap();
        std::fs::write(prepared.join("findings.json"), "{}").unwrap();
        std::fs::write(prepared.join("nested/extra.txt"), "x").unwrap();

        let runner = FakeRunner::from_dir(&prepared).unwrap();
        let out = tmp.path().join("out");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        runner.run(spec(out.clone(), work)).await.unwrap();

        assert!(out.join("findings.json").exists());
        assert!(out.join("nested/extra.txt").exists());
    }
}
