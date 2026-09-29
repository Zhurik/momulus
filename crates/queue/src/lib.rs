//! Queue storage: SQLite via sqlx.

pub mod jobs;
pub mod store;
pub mod worker;

pub use jobs::StoredJob;
pub use store::Store;
pub use worker::{JobHandler, Worker, WorkerConfig};

use std::path::Path;
use std::str::FromStr;

use momulus_core::{Error, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

/// Migrations live in the repository root and are embedded into the binary.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// A connection pool with the migrations already applied.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Opens (and creates if needed) a file-backed database.
    pub async fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Storage(format!("cannot create {}: {e}", parent.display())))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(10))
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        Db::connect(options).await
    }

    /// An in-memory database — for tests.
    pub async fn open_in_memory() -> Result<Db> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(|e| Error::Storage(e.to_string()))?
            .foreign_keys(true);
        Db::connect(options).await
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Db> {
        let pool = SqlitePoolOptions::new()
            // An in-memory SQLite database lives as long as the connection: keep exactly one.
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| Error::Storage(format!("cannot open database: {e}")))?;
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|e| Error::Storage(format!("migrations failed: {e}")))?;
        Ok(Db { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Closes the pool; used during graceful shutdown.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// The table list — used by tests and diagnostics.
    pub async fn tables(&self) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(rows
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_create_the_schema() {
        let db = Db::open_in_memory().await.unwrap();
        let tables = db.tables().await.unwrap();
        for expected in ["jobs", "repo_cursors", "seen_comments"] {
            assert!(tables.contains(&expected.to_string()), "{tables:?}");
        }
    }

    #[tokio::test]
    async fn file_database_is_created_with_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/momulus.sqlite");
        let db = Db::open(&path).await.unwrap();
        assert!(path.exists());
        db.close().await;
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("momulus.sqlite");
        Db::open(&path).await.unwrap().close().await;
        let db = Db::open(&path).await.unwrap();
        assert!(db.tables().await.unwrap().contains(&"jobs".to_string()));
    }
}
