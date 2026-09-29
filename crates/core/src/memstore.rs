//! Реализации трейтов в памяти — для тестов и для `--dry-run`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::traits::{CursorStore, GitAccess, SkillCatalog};
use crate::types::PrRef;

/// Курсоры и метки в памяти.
#[derive(Debug, Default)]
pub struct MemoryCursorStore {
    cursors: Mutex<BTreeMap<(String, String), DateTime<Utc>>>,
    seen: Mutex<BTreeSet<(String, u64)>>,
}

impl MemoryCursorStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CursorStore for MemoryCursorStore {
    async fn cursor(&self, repo_key: &str, stream: &str) -> Result<Option<DateTime<Utc>>> {
        Ok(self
            .cursors
            .lock()
            .expect("mutex")
            .get(&(repo_key.to_string(), stream.to_string()))
            .copied())
    }

    async fn set_cursor(&self, repo_key: &str, stream: &str, value: DateTime<Utc>) -> Result<()> {
        self.cursors
            .lock()
            .expect("mutex")
            .insert((repo_key.to_string(), stream.to_string()), value);
        Ok(())
    }

    async fn mark_seen(&self, platform: &str, comment_id: u64) -> Result<bool> {
        Ok(self
            .seen
            .lock()
            .expect("mutex")
            .insert((platform.to_string(), comment_id)))
    }
}

/// Фиксированный список скиллов — для тестов платформенного кода.
#[derive(Debug, Clone)]
pub struct StaticSkillCatalog {
    skills: Vec<String>,
}

impl StaticSkillCatalog {
    pub fn new<I, S>(skills: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        StaticSkillCatalog {
            skills: skills.into_iter().map(Into::into).collect(),
        }
    }
}

#[async_trait]
impl SkillCatalog for StaticSkillCatalog {
    async fn contains(&self, skill: &str) -> bool {
        self.skills.iter().any(|s| s == skill)
    }

    async fn help_text(&self) -> String {
        let mut out = String::from("Доступные команды:\n\n");
        for skill in &self.skills {
            out.push_str(&format!("- `/llm {skill}`\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cursor_roundtrip() {
        let store = MemoryCursorStore::new();
        assert!(store.cursor("github:a/b", "issue").await.unwrap().is_none());
        let now = Utc::now();
        store.set_cursor("github:a/b", "issue", now).await.unwrap();
        assert_eq!(
            store.cursor("github:a/b", "issue").await.unwrap(),
            Some(now)
        );
        // Другой поток того же репозитория — свой курсор.
        assert!(
            store
                .cursor("github:a/b", "review")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn mark_seen_is_idempotent() {
        let store = MemoryCursorStore::new();
        assert!(store.mark_seen("github", 1).await.unwrap());
        assert!(!store.mark_seen("github", 1).await.unwrap());
        assert!(store.mark_seen("gitlab", 1).await.unwrap());
    }

    #[tokio::test]
    async fn static_catalog_answers() {
        let catalog = StaticSkillCatalog::new(["proofread", "translate"]);
        assert!(catalog.contains("proofread").await);
        assert!(!catalog.contains("nope").await);
        assert!(catalog.help_text().await.contains("/llm translate"));
    }
}

/// Доступ к git без авторизации: локальный remote в тестах и прогонах на диске.
#[derive(Debug, Clone, Default)]
pub struct LocalGitAccess;

#[async_trait]
impl GitAccess for LocalGitAccess {
    async fn git_token(&self, _pr: &PrRef) -> Result<Option<String>> {
        Ok(None)
    }

    fn refspecs(&self, _pr: &PrRef) -> Vec<String> {
        vec!["+refs/heads/*:refs/heads/*".to_string()]
    }
}
