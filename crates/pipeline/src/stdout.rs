//! Publisher, который ничего не публикует: печатает в stdout.
//!
//! Используется флагом `--dry-run`: джоба выполняется целиком, включая вызов
//! модели, но в PR ничего не уходит.

use async_trait::async_trait;
use llm_bot_core::{AckState, Finding, JobRef, Patch, PrRef, Publisher, Result};
use url::Url;

#[derive(Debug, Default)]
pub struct StdoutPublisher;

impl StdoutPublisher {
    pub fn new() -> StdoutPublisher {
        StdoutPublisher
    }
}

#[async_trait]
impl Publisher for StdoutPublisher {
    async fn ack(&self, job: &JobRef, state: AckState) -> Result<()> {
        let mark = match state {
            AckState::Received => "взято в работу 👀",
            AckState::Succeeded => "успех ✅",
            AckState::Failed => "провал ❌",
        };
        println!(
            "[dry-run] {} PR #{} comment {} → {mark}",
            job.pr.full_name(),
            job.pr.number,
            job.comment.id
        );
        Ok(())
    }

    async fn post_review(&self, pr: &PrRef, findings: &[Finding], summary: &str) -> Result<()> {
        println!(
            "\n[dry-run] ревью в {}#{} ({} inline-комментариев)\n--- summary ---\n{summary}",
            pr.full_name(),
            pr.number,
            findings.len()
        );
        for finding in findings {
            println!(
                "--- {}:{} [{}] ---\n{}",
                finding.path,
                finding.line,
                finding.severity.as_str(),
                finding.body
            );
            if let Some(suggestion) = &finding.suggestion {
                println!("```suggestion\n{suggestion}\n```");
            }
        }
        Ok(())
    }

    async fn push_and_open_pr(&self, pr: &PrRef, patch: &Patch) -> Result<Url> {
        println!(
            "\n[dry-run] ветка {} в {} ({} файлов), PR с base {}\n--- заголовок ---\n{}\n--- тело ---\n{}",
            patch.branch,
            pr.full_name(),
            patch.files.len(),
            pr.head_ref,
            patch.title,
            patch.body
        );
        for file in &patch.files {
            println!("  изменён: {file}");
        }
        println!(
            "  (рабочая копия оставлена как есть: {})",
            patch.worktree.display()
        );
        Ok(Url::parse("https://example.invalid/dry-run/pull/0").expect("валидный URL"))
    }

    async fn comment(&self, pr: &PrRef, body: &str) -> Result<()> {
        println!(
            "\n[dry-run] комментарий в {}#{}:\n{body}",
            pr.full_name(),
            pr.number
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_bot_core::{CommentKind, CommentRef, JobId, Severity};
    use std::path::PathBuf;

    fn pr() -> PrRef {
        PrRef {
            platform: "github".into(),
            owner: "acme".into(),
            repo: "blog".into(),
            number: 42,
            head_sha: "abc".into(),
            head_ref: "feature".into(),
            base_ref: "main".into(),
            head_repo: "acme/blog".into(),
            clone_url: "https://github.com/acme/blog.git".into(),
        }
    }

    #[tokio::test]
    async fn every_method_succeeds_and_publishes_nothing() {
        let publisher = StdoutPublisher::new();
        let job = JobRef {
            id: JobId::new(),
            pr: pr(),
            comment: CommentRef {
                id: 1,
                kind: CommentKind::Issue,
                author: "zhurik".into(),
                url: None,
            },
        };

        publisher.ack(&job, AckState::Received).await.unwrap();
        publisher
            .post_review(
                &pr(),
                &[Finding {
                    path: "a.md".into(),
                    line: 1,
                    severity: Severity::Typo,
                    body: "b".into(),
                    suggestion: Some("c".into()),
                }],
                "итог",
            )
            .await
            .unwrap();
        publisher.comment(&pr(), "текст").await.unwrap();

        let url = publisher
            .push_and_open_pr(
                &pr(),
                &Patch {
                    worktree: PathBuf::from("/tmp/work"),
                    branch: "llm/x".into(),
                    commit_message: "c".into(),
                    title: "t".into(),
                    body: "b".into(),
                    files: vec!["a.md".into()],
                },
            )
            .await
            .unwrap();
        assert_eq!(url.host_str(), Some("example.invalid"));
    }
}
