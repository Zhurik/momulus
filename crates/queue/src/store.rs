//! Хранилище курсоров опроса и разобранных комментариев.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use momulus_core::{CursorStore, Error, Result};
use sqlx::Row;

use crate::Db;

/// Реализация [`CursorStore`] на SQLite.
#[derive(Debug, Clone)]
pub struct Store {
    db: Db,
}

impl Store {
    pub fn new(db: Db) -> Store {
        Store { db }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }
}

#[async_trait]
impl CursorStore for Store {
    async fn cursor(&self, repo_key: &str, stream: &str) -> Result<Option<DateTime<Utc>>> {
        let row =
            sqlx::query("SELECT cursor FROM repo_cursors WHERE repo_key = ?1 AND stream = ?2")
                .bind(repo_key)
                .bind(stream)
                .fetch_optional(self.db.pool())
                .await
                .map_err(|e| Error::Storage(e.to_string()))?;

        let Some(row) = row else { return Ok(None) };
        let raw: String = row.get("cursor");
        let parsed = DateTime::parse_from_rfc3339(&raw)
            .map_err(|e| Error::Storage(format!("курсор {raw:?} не разбирается: {e}")))?;
        Ok(Some(parsed.with_timezone(&Utc)))
    }

    async fn set_cursor(&self, repo_key: &str, stream: &str, value: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "INSERT INTO repo_cursors (repo_key, stream, cursor, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (repo_key, stream) DO UPDATE SET cursor = ?3, updated_at = ?4",
        )
        .bind(repo_key)
        .bind(stream)
        .bind(value.to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn mark_seen(&self, platform: &str, comment_id: u64) -> Result<bool> {
        // Вставка с игнорированием конфликта: число затронутых строк и есть ответ.
        let result = sqlx::query(
            "INSERT OR IGNORE INTO seen_comments (platform, comment_id, seen_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(platform)
        .bind(comment_id as i64)
        .bind(Utc::now().to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(result.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> Store {
        Store::new(Db::open_in_memory().await.unwrap())
    }

    #[tokio::test]
    async fn cursor_is_absent_then_saved() {
        let store = store().await;
        assert!(
            store
                .cursor("github:acme/blog", "issue_comments")
                .await
                .unwrap()
                .is_none()
        );

        let value = "2026-09-29T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        store
            .set_cursor("github:acme/blog", "issue_comments", value)
            .await
            .unwrap();
        assert_eq!(
            store
                .cursor("github:acme/blog", "issue_comments")
                .await
                .unwrap(),
            Some(value)
        );
    }

    #[tokio::test]
    async fn cursor_is_updated_in_place() {
        let store = store().await;
        let first = "2026-09-29T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let second = "2026-09-29T11:00:00Z".parse::<DateTime<Utc>>().unwrap();
        store.set_cursor("r", "s", first).await.unwrap();
        store.set_cursor("r", "s", second).await.unwrap();
        assert_eq!(store.cursor("r", "s").await.unwrap(), Some(second));

        let count: i64 = sqlx::query("SELECT COUNT(*) AS n FROM repo_cursors")
            .fetch_one(store.db().pool())
            .await
            .unwrap()
            .get("n");
        assert_eq!(count, 1, "одна строка на пару репозиторий+поток");
    }

    #[tokio::test]
    async fn streams_have_independent_cursors() {
        let store = store().await;
        let value = Utc::now();
        store
            .set_cursor("r", "issue_comments", value)
            .await
            .unwrap();
        assert!(
            store
                .cursor("r", "review_comments")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn seen_comments_are_deduplicated() {
        let store = store().await;
        assert!(store.mark_seen("github", 1001).await.unwrap());
        assert!(!store.mark_seen("github", 1001).await.unwrap());
        // Другая платформа — независимое пространство id.
        assert!(store.mark_seen("gitlab", 1001).await.unwrap());
    }

    #[tokio::test]
    async fn state_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("momulus.sqlite");
        let value = "2026-09-29T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        {
            let store = Store::new(Db::open(&path).await.unwrap());
            store.set_cursor("r", "s", value).await.unwrap();
            assert!(store.mark_seen("github", 7).await.unwrap());
            store.db().close().await;
        }

        let store = Store::new(Db::open(&path).await.unwrap());
        assert_eq!(store.cursor("r", "s").await.unwrap(), Some(value));
        assert!(
            !store.mark_seen("github", 7).await.unwrap(),
            "память о комментарии сохранилась"
        );
    }
}
