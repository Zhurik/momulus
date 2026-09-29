//! Platform-independent types the pipeline works with.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::command::Command;

/// Job identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub Uuid);

impl JobId {
    pub fn new() -> Self {
        JobId(Uuid::new_v4())
    }

    /// Short form used in PR messages.
    pub fn short(&self) -> String {
        self.0.simple().to_string()[..8].to_string()
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for JobId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(JobId(Uuid::parse_str(s)?))
    }
}

/// Reference to a pull request on some platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRef {
    /// Platform identifier: "github", "gitlab", ...
    pub platform: String,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    /// SHA of the PR head commit.
    pub head_sha: String,
    /// Head branch name (without the remote).
    pub head_ref: String,
    /// Base branch name.
    pub base_ref: String,
    /// Full name of the head repository ("owner/repo"); differs from base for forks.
    pub head_repo: String,
    /// Clone URL of the base repository.
    pub clone_url: String,
}

impl PrRef {
    /// Full name of the base repository.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// The PR comes from a fork — we cannot push to its head branch.
    pub fn is_fork(&self) -> bool {
        self.head_repo != self.full_name()
    }

    /// Stable repository key for the cache and cursors.
    pub fn repo_key(&self) -> String {
        format!("{}:{}/{}", self.platform, self.owner, self.repo)
    }
}

/// Where exactly the command comment lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    /// Plain PR comment (issue comment).
    Issue,
    /// Comment inside a review thread (review comment).
    Review,
}

impl CommentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentKind::Issue => "issue",
            CommentKind::Review => "review",
        }
    }
}

/// The comment the command arrived in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRef {
    pub id: u64,
    pub kind: CommentKind,
    pub author: String,
    pub url: Option<String>,
}

/// Unit of work: one `/llm` command from one comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub pr: PrRef,
    pub command: Command,
    pub comment: CommentRef,
    pub created_at: DateTime<Utc>,
}

impl Job {
    pub fn new(pr: PrRef, command: Command, comment: CommentRef) -> Self {
        Job {
            id: JobId::new(),
            pr,
            command,
            comment,
            created_at: Utc::now(),
        }
    }

    pub fn job_ref(&self) -> JobRef {
        JobRef {
            id: self.id,
            pr: self.pr.clone(),
            comment: self.comment.clone(),
        }
    }
}

/// The minimum needed to acknowledge a job's status on the platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRef {
    pub id: JobId,
    pub pr: PrRef,
    pub comment: CommentRef,
}

/// Job status in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for JobStatus {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(JobStatus::Queued),
            "running" => Ok(JobStatus::Running),
            "done" => Ok(JobStatus::Done),
            "failed" => Ok(JobStatus::Failed),
            other => Err(crate::Error::Storage(format!(
                "unknown job status: {other}"
            ))),
        }
    }
}

/// Job state as shown in the PR (a reaction on the comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckState {
    /// Picked up — 👀
    Received,
    /// Success — ✅
    Succeeded,
    /// Failure — ❌
    Failed,
}

/// Finding category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Typo,
    Grammar,
    Punctuation,
    Style,
    Terminology,
    Bug,
    Security,
    Other,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Typo => "typo",
            Severity::Grammar => "grammar",
            Severity::Punctuation => "punctuation",
            Severity::Style => "style",
            Severity::Terminology => "terminology",
            Severity::Bug => "bug",
            Severity::Security => "security",
            Severity::Other => "other",
        }
    }
}

/// A single model finding, anchored to a line of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// File path relative to the repository root.
    pub path: String,
    /// Line number in the new version of the file (1-based).
    pub line: u32,
    pub severity: Severity,
    /// Finding text.
    pub body: String,
    /// Suggested replacement for the whole line, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Output of a review-mode skill — exactly what the model writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutput {
    /// Short review summary.
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// A patch ready to publish: the changes already live in the worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// Working copy holding the changes.
    pub worktree: PathBuf,
    /// Branch name we commit to.
    pub branch: String,
    pub commit_message: String,
    /// Title of the PR to be opened.
    pub title: String,
    /// Body of the PR to be opened.
    pub body: String,
    /// Changed files (relative paths).
    pub files: Vec<String>,
}

/// How the working copy is mounted inside the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mount {
    ReadOnly,
    ReadWrite,
}

/// Description of the single LLM step.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    pub job_id: JobId,
    /// Skill name: the container gets its directory via `--skill /skills/<name>`.
    pub skill: String,
    /// Host path of the working copy, mounted at /work.
    pub workdir: PathBuf,
    /// Host path of the skills directory, mounted at /skills (read-only).
    pub skills_dir: PathBuf,
    /// Host path for artifacts, mounted at /out (read-write).
    pub out_dir: PathBuf,
    pub mount: Mount,
    pub prompt: String,
    pub tools: Vec<String>,
    pub provider: String,
    pub model: Option<String>,
    pub timeout: Duration,
    pub image: String,
    pub cpu_limit: f64,
    pub memory_limit_mb: u64,
    /// Container environment variables (the LLM provider key only).
    pub env: Vec<(String, String)>,
    /// Contents of pi's `models.json`, when the provider needs its own base URL.
    pub agent_config: Option<String>,
}

/// What the LLM step returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl RunResult {
    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    /// Combined log to store in the job's log file.
    pub fn combined_log(&self) -> String {
        format!(
            "=== stdout ===\n{}\n=== stderr ===\n{}\n",
            self.stdout, self.stderr
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(head_repo: &str) -> PrRef {
        PrRef {
            platform: "github".into(),
            owner: "acme".into(),
            repo: "blog".into(),
            number: 42,
            head_sha: "deadbeef".into(),
            head_ref: "feature".into(),
            base_ref: "main".into(),
            head_repo: head_repo.into(),
            clone_url: "https://github.com/acme/blog.git".into(),
        }
    }

    #[test]
    fn fork_is_detected_by_head_repo() {
        assert!(!pr("acme/blog").is_fork());
        assert!(pr("contributor/blog").is_fork());
    }

    #[test]
    fn repo_key_is_stable() {
        assert_eq!(pr("acme/blog").repo_key(), "github:acme/blog");
    }

    #[test]
    fn job_status_roundtrip() {
        for status in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Done,
            JobStatus::Failed,
        ] {
            let parsed: JobStatus = status.as_str().parse().unwrap();
            assert_eq!(parsed, status);
        }
        assert!("nope".parse::<JobStatus>().is_err());
    }

    #[test]
    fn findings_reject_unknown_fields() {
        let json = r#"{"path":"a.md","line":1,"severity":"typo","body":"x","oops":true}"#;
        let err = serde_json::from_str::<Finding>(json).unwrap_err();
        assert!(err.to_string().contains("oops"), "{err}");
    }

    #[test]
    fn review_output_parses_minimal_json() {
        let out: ReviewOutput = serde_json::from_str(r#"{"summary":"ok"}"#).unwrap();
        assert!(out.findings.is_empty());
    }

    #[test]
    fn job_id_short_is_eight_chars() {
        assert_eq!(JobId::new().short().len(), 8);
    }
}
