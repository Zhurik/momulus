//! A Publisher that publishes nothing: it prints to stdout.
//!
//! Used by the `--dry-run` flag: the job runs end to end, model call included,
//! but nothing reaches the PR.

use async_trait::async_trait;
use momulus_core::{AckState, Finding, JobRef, Patch, PrRef, Publisher, Result};
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
            AckState::Received => "picked up 👀",
            AckState::Succeeded => "success ✅",
            AckState::Failed => "failure ❌",
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
            "\n[dry-run] review on {}#{} ({} inline comments)\n--- summary ---\n{summary}",
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
            "\n[dry-run] branch {} in {} ({} files), PR with base {}\n--- title ---\n{}\n--- body ---\n{}",
            patch.branch,
            pr.full_name(),
            patch.files.len(),
            pr.head_ref,
            patch.title,
            patch.body
        );
        for file in &patch.files {
            println!("  changed: {file}");
        }
        println!(
            "  (the working copy was left as is: {})",
            patch.worktree.display()
        );
        Ok(Url::parse("https://example.invalid/dry-run/pull/0").expect("a valid URL"))
    }

    async fn comment(&self, pr: &PrRef, body: &str) -> Result<()> {
        println!(
            "\n[dry-run] comment on {}#{}:\n{body}",
            pr.full_name(),
            pr.number
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use momulus_core::{CommentKind, CommentRef, JobId, Severity};
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
                "wrap-up",
            )
            .await
            .unwrap();
        publisher.comment(&pr(), "some text").await.unwrap();

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
