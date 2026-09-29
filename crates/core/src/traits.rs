//! The boundary between the core and the platforms. Implement these to add one.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::Result;
use crate::types::{AckState, Finding, Job, JobRef, Patch, PrRef, RunResult, RunSpec};

/// Source of commands: watches the platform and emits jobs.
#[async_trait]
pub trait Trigger: Send + Sync {
    /// Runs until cancelled; every recognised command goes into `tx`.
    async fn run(&self, tx: mpsc::Sender<Job>, shutdown: CancellationToken) -> Result<()>;
}

/// Sink for results: publishes reviews, patches and statuses.
#[async_trait]
pub trait Publisher: Send + Sync {
    /// Marks the job state on the originating comment (a reaction).
    async fn ack(&self, job: &JobRef, state: AckState) -> Result<()>;

    /// Publishes a review with inline comments.
    async fn post_review(&self, pr: &PrRef, findings: &[Finding], summary: &str) -> Result<()>;

    /// Pushes the branch with the changes and opens a PR; returns its link.
    async fn push_and_open_pr(&self, pr: &PrRef, patch: &Patch) -> Result<Url>;

    /// Posts a plain comment on the PR.
    async fn comment(&self, pr: &PrRef, body: &str) -> Result<()>;
}

/// The single LLM step of the pipeline.
#[async_trait]
pub trait Runner: Send + Sync {
    async fn run(&self, spec: RunSpec) -> Result<RunResult>;
}

/// Where the Trigger keeps its polling position and the memory of handled comments.
///
/// Behind a trait so that platform code does not depend on SQLite.
#[async_trait]
pub trait CursorStore: Send + Sync {
    /// Polling position for a "repository + comment stream" pair.
    async fn cursor(&self, repo_key: &str, stream: &str) -> Result<Option<DateTime<Utc>>>;

    /// Stores a new position.
    async fn set_cursor(&self, repo_key: &str, stream: &str, value: DateTime<Utc>) -> Result<()>;

    /// Marks a comment as handled. `true` means we are seeing it for the first time.
    ///
    /// Keeps us from answering the same comment twice, even if the cursor moved
    /// backwards or the comment was edited.
    async fn mark_seen(&self, platform: &str, comment_id: u64) -> Result<bool>;
}

/// The Trigger's view of the skill registry: check a name and show help.
///
/// Also a trait: the registry lives in another crate and can be reloaded on SIGHUP.
#[async_trait]
pub trait SkillCatalog: Send + Sync {
    async fn contains(&self, skill: &str) -> bool;
    async fn help_text(&self) -> String;
}

/// Access to the platform's git remote: the token and the refspecs to fetch.
///
/// Platforms name their PR refs differently, so this is a platform detail too.
#[async_trait]
pub trait GitAccess: Send + Sync {
    /// Token for fetch/push; `None` means the remote needs no authentication.
    async fn git_token(&self, pr: &PrRef) -> Result<Option<String>>;

    /// What to fetch in order to get the PR head and its base branch.
    fn refspecs(&self, pr: &PrRef) -> Vec<String>;

    /// The revision the PR diff is computed against (usually the base branch).
    fn base_rev(&self, pr: &PrRef) -> String {
        format!("refs/heads/{}", pr.base_ref)
    }
}
