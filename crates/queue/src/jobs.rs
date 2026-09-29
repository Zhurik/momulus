//! Job lifecycle in SQLite.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use momulus_core::{
    Args, Command, CommentKind, CommentRef, Error, Job, JobId, JobStatus, PrRef, Result,
};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::Store;

/// A job together with the queue's bookkeeping fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJob {
    pub job: Job,
    pub status: JobStatus,
    /// How many times the job has been picked up.
    pub attempts: u32,
    pub error: Option<String>,
    pub log_path: Option<PathBuf>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Store {
    /// Enqueues a job. `false` means this comment is already in the database.
    pub async fn enqueue(&self, job: &Job) -> Result<bool> {
        let args = serde_json::to_string(&job.command.args)
            .map_err(|e| Error::Storage(format!("args: {e}")))?;
        let pr_json =
            serde_json::to_string(&job.pr).map_err(|e| Error::Storage(format!("pr: {e}")))?;

        let result = sqlx::query(
            "INSERT OR IGNORE INTO jobs (
                 id, platform, repo, pr, comment_id, comment_kind, comment_author,
                 skill, args, pr_json, status, attempts, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
        )
        .bind(job.id.to_string())
        .bind(&job.pr.platform)
        .bind(job.pr.full_name())
        .bind(job.pr.number as i64)
        .bind(job.comment.id as i64)
        .bind(job.comment.kind.as_str())
        .bind(&job.comment.author)
        .bind(&job.command.skill)
        .bind(args)
        .bind(pr_json)
        .bind(JobStatus::Queued.as_str())
        .bind(job.created_at.to_rfc3339())
        .execute(self.db().pool())
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;

        Ok(result.rows_affected() == 1)
    }

    /// Claims the oldest queued job and marks it as running.
    pub async fn claim_next(&self) -> Result<Option<StoredJob>> {
        let row = sqlx::query(
            "UPDATE jobs
                SET status = ?1, attempts = attempts + 1, started_at = ?2, error = NULL
              WHERE id = (
                  SELECT id FROM jobs WHERE status = ?3 ORDER BY created_at, rowid LIMIT 1
              )
              RETURNING *",
        )
        .bind(JobStatus::Running.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(JobStatus::Queued.as_str())
        .fetch_optional(self.db().pool())
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;

        row.map(stored_from_row).transpose()
    }

    /// Completes the job successfully.
    pub async fn mark_done(&self, id: JobId, log_path: Option<&Path>) -> Result<()> {
        self.finish(id, JobStatus::Done, None, log_path).await
    }

    /// Completes the job with an error.
    pub async fn mark_failed(&self, id: JobId, error: &str, log_path: Option<&Path>) -> Result<()> {
        self.finish(id, JobStatus::Failed, Some(error), log_path)
            .await
    }

    async fn finish(
        &self,
        id: JobId,
        status: JobStatus,
        error: Option<&str>,
        log_path: Option<&Path>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE jobs
                SET status = ?1, error = ?2, finished_at = ?3,
                    log_path = COALESCE(?4, log_path)
              WHERE id = ?5",
        )
        .bind(status.as_str())
        .bind(error)
        .bind(Utc::now().to_rfc3339())
        .bind(log_path.map(|p| p.to_string_lossy().to_string()))
        .bind(id.to_string())
        .execute(self.db().pool())
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    /// Puts the job back into the queue for another attempt.
    pub async fn requeue(&self, id: JobId, error: &str) -> Result<()> {
        sqlx::query("UPDATE jobs SET status = ?1, error = ?2, started_at = NULL WHERE id = ?3")
            .bind(JobStatus::Queued.as_str())
            .bind(error)
            .bind(id.to_string())
            .execute(self.db().pool())
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    /// On service start: everything left in `running` goes back to the queue.
    ///
    /// Returns the number of recovered jobs.
    pub async fn recover_running(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE jobs
                SET status = ?1, started_at = NULL,
                    error = 'the service restarted while this job was running'
              WHERE status = ?2",
        )
        .bind(JobStatus::Queued.as_str())
        .bind(JobStatus::Running.as_str())
        .execute(self.db().pool())
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(result.rows_affected())
    }

    /// A job by its identifier.
    pub async fn job(&self, id: JobId) -> Result<Option<StoredJob>> {
        let row = sqlx::query("SELECT * FROM jobs WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(self.db().pool())
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        row.map(stored_from_row).transpose()
    }

    /// How many jobs are in a given status.
    pub async fn count_by_status(&self, status: JobStatus) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM jobs WHERE status = ?1")
            .bind(status.as_str())
            .fetch_one(self.db().pool())
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.get("n"))
    }

    /// Records where the job's log was written.
    pub async fn set_log_path(&self, id: JobId, log_path: &Path) -> Result<()> {
        sqlx::query("UPDATE jobs SET log_path = ?1 WHERE id = ?2")
            .bind(log_path.to_string_lossy().to_string())
            .bind(id.to_string())
            .execute(self.db().pool())
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }
}

/// Builds a [`StoredJob`] from a database row.
fn stored_from_row(row: SqliteRow) -> Result<StoredJob> {
    let bad = |what: &str, e: String| Error::Storage(format!("{what}: {e}"));

    let id: String = row.get("id");
    let id: JobId = id
        .parse()
        .map_err(|e: uuid::Error| bad("id", e.to_string()))?;

    let pr_json: String = row.get("pr_json");
    let pr: PrRef = serde_json::from_str(&pr_json).map_err(|e| bad("pr_json", e.to_string()))?;

    let args_json: String = row.get("args");
    let args: Args = serde_json::from_str(&args_json).map_err(|e| bad("args", e.to_string()))?;

    let kind: String = row.get("comment_kind");
    let kind = match kind.as_str() {
        "issue" => CommentKind::Issue,
        "review" => CommentKind::Review,
        other => {
            return Err(Error::Storage(format!("unknown comment kind {other}")));
        }
    };

    let status: String = row.get("status");
    let status: JobStatus = status.parse()?;

    let created_at: String = row.get("created_at");
    let created_at = parse_time(&created_at)?;

    Ok(StoredJob {
        job: Job {
            id,
            pr,
            command: Command {
                skill: row.get("skill"),
                args,
            },
            comment: CommentRef {
                id: row.get::<i64, _>("comment_id") as u64,
                kind,
                author: row.get("comment_author"),
                url: None,
            },
            created_at,
        },
        status,
        attempts: row.get::<i64, _>("attempts") as u32,
        error: row.get("error"),
        log_path: row.get::<Option<String>, _>("log_path").map(PathBuf::from),
        started_at: row
            .get::<Option<String>, _>("started_at")
            .map(|t| parse_time(&t))
            .transpose()?,
        finished_at: row
            .get::<Option<String>, _>("finished_at")
            .map(|t| parse_time(&t))
            .transpose()?,
    })
}

fn parse_time(raw: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(raw)
        .map_err(|e| Error::Storage(format!("timestamp {raw:?}: {e}")))?
        .with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    async fn store() -> Store {
        Store::new(Db::open_in_memory().await.unwrap())
    }

    fn pr() -> PrRef {
        PrRef {
            platform: "github".into(),
            owner: "acme".into(),
            repo: "blog".into(),
            number: 42,
            head_sha: "abc123".into(),
            head_ref: "feature".into(),
            base_ref: "main".into(),
            head_repo: "acme/blog".into(),
            clone_url: "https://github.com/acme/blog.git".into(),
        }
    }

    fn job(comment_id: u64, command: &str) -> Job {
        Job::new(
            pr(),
            Command::parse(command).unwrap(),
            CommentRef {
                id: comment_id,
                kind: CommentKind::Issue,
                author: "zhurik".into(),
                url: None,
            },
        )
    }

    #[tokio::test]
    async fn enqueue_then_claim_then_done() {
        let store = store().await;
        let job = job(1001, "/llm translate en");
        assert!(store.enqueue(&job).await.unwrap());
        assert_eq!(store.count_by_status(JobStatus::Queued).await.unwrap(), 1);

        let claimed = store
            .claim_next()
            .await
            .unwrap()
            .expect("a job was claimed");
        assert_eq!(claimed.job.id, job.id);
        assert_eq!(claimed.status, JobStatus::Running);
        assert_eq!(claimed.attempts, 1);
        assert!(claimed.started_at.is_some());
        // Arguments and the PR reference were restored in full.
        assert_eq!(claimed.job.command.skill, "translate");
        assert_eq!(claimed.job.command.args.positional, vec!["en".to_string()]);
        assert_eq!(claimed.job.pr.head_sha, "abc123");
        assert_eq!(claimed.job.comment.kind, CommentKind::Issue);

        assert!(
            store.claim_next().await.unwrap().is_none(),
            "the queue is empty"
        );

        store
            .mark_done(job.id, Some(Path::new("/data/logs/x.log")))
            .await
            .unwrap();
        let stored = store.job(job.id).await.unwrap().unwrap();
        assert_eq!(stored.status, JobStatus::Done);
        assert_eq!(stored.log_path, Some(PathBuf::from("/data/logs/x.log")));
        assert!(stored.finished_at.is_some());
    }

    #[tokio::test]
    async fn duplicate_comment_is_rejected() {
        let store = store().await;
        assert!(store.enqueue(&job(1001, "/llm proofread")).await.unwrap());
        // A different job id, the same comment.
        assert!(!store.enqueue(&job(1001, "/llm proofread")).await.unwrap());
        assert_eq!(store.count_by_status(JobStatus::Queued).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn jobs_are_claimed_in_arrival_order() {
        let store = store().await;
        let first = job(1, "/llm proofread");
        let second = job(2, "/llm review");
        store.enqueue(&first).await.unwrap();
        store.enqueue(&second).await.unwrap();

        assert_eq!(store.claim_next().await.unwrap().unwrap().job.id, first.id);
        assert_eq!(store.claim_next().await.unwrap().unwrap().job.id, second.id);
    }

    #[tokio::test]
    async fn failed_job_keeps_the_reason() {
        let store = store().await;
        let job = job(1001, "/llm proofread");
        store.enqueue(&job).await.unwrap();
        store.claim_next().await.unwrap();
        store
            .mark_failed(job.id, "the model returned invalid JSON", None)
            .await
            .unwrap();

        let stored = store.job(job.id).await.unwrap().unwrap();
        assert_eq!(stored.status, JobStatus::Failed);
        assert_eq!(
            stored.error.as_deref(),
            Some("the model returned invalid JSON")
        );
    }

    #[tokio::test]
    async fn requeue_increments_attempts_on_next_claim() {
        let store = store().await;
        let job = job(1001, "/llm proofread");
        store.enqueue(&job).await.unwrap();

        store.claim_next().await.unwrap();
        store.requeue(job.id, "the network dropped").await.unwrap();
        let stored = store.job(job.id).await.unwrap().unwrap();
        assert_eq!(stored.status, JobStatus::Queued);
        assert_eq!(stored.attempts, 1);
        assert_eq!(stored.error.as_deref(), Some("the network dropped"));

        let claimed = store.claim_next().await.unwrap().unwrap();
        assert_eq!(claimed.attempts, 2);
        assert!(
            claimed.error.is_none(),
            "the previous attempt's error is cleared"
        );
    }

    #[tokio::test]
    async fn running_jobs_are_recovered_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("momulus.sqlite");
        let job = job(1001, "/llm proofread");
        {
            let store = Store::new(Db::open(&path).await.unwrap());
            store.enqueue(&job).await.unwrap();
            store.claim_next().await.unwrap();
            store.db().close().await;
        }

        let store = Store::new(Db::open(&path).await.unwrap());
        assert_eq!(store.recover_running().await.unwrap(), 1);
        let stored = store.job(job.id).await.unwrap().unwrap();
        assert_eq!(stored.status, JobStatus::Queued);
        assert!(stored.error.unwrap().contains("restarted"));
        // The attempt counter survived: a restart does not reset retries.
        assert_eq!(store.claim_next().await.unwrap().unwrap().attempts, 2);
    }

    #[tokio::test]
    async fn recover_does_not_touch_finished_jobs() {
        let store = store().await;
        let done = job(1, "/llm proofread");
        let failed = job(2, "/llm review");
        store.enqueue(&done).await.unwrap();
        store.enqueue(&failed).await.unwrap();
        store.claim_next().await.unwrap();
        store.mark_done(done.id, None).await.unwrap();
        store.claim_next().await.unwrap();
        store
            .mark_failed(failed.id, "an error", None)
            .await
            .unwrap();

        assert_eq!(store.recover_running().await.unwrap(), 0);
        assert_eq!(store.count_by_status(JobStatus::Done).await.unwrap(), 1);
        assert_eq!(store.count_by_status(JobStatus::Failed).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn log_path_can_be_set_separately() {
        let store = store().await;
        let job = job(1001, "/llm proofread");
        store.enqueue(&job).await.unwrap();
        store
            .set_log_path(job.id, Path::new("/data/logs/job.log"))
            .await
            .unwrap();
        // mark_done without a path does not erase the one already stored.
        store.claim_next().await.unwrap();
        store.mark_done(job.id, None).await.unwrap();
        assert_eq!(
            store.job(job.id).await.unwrap().unwrap().log_path,
            Some(PathBuf::from("/data/logs/job.log"))
        );
    }

    #[tokio::test]
    async fn unknown_job_is_none() {
        let store = store().await;
        assert!(store.job(JobId::new()).await.unwrap().is_none());
    }
}
