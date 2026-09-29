//! Реестр скиллов, который можно перечитать на SIGHUP.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use llm_bot_core::{Result, SkillCatalog};
use llm_bot_skills::{Registry, Skill};
use tokio::sync::RwLock;

/// Разделяемый реестр: один объект и для Trigger'а, и для пайплайна.
#[derive(Debug)]
pub struct SharedRegistry {
    dir: PathBuf,
    registry: RwLock<Registry>,
}

impl SharedRegistry {
    /// Читает каталог скиллов.
    pub fn load(dir: &Path) -> Result<Arc<SharedRegistry>> {
        let registry = Registry::load(dir)?;
        Ok(Arc::new(SharedRegistry {
            dir: dir.to_path_buf(),
            registry: RwLock::new(registry),
        }))
    }

    /// Перечитывает каталог. При ошибке прежний реестр остаётся в силе.
    pub async fn reload(&self) -> Result<usize> {
        let fresh = Registry::load(&self.dir)?;
        let count = fresh.len();
        *self.registry.write().await = fresh;
        Ok(count)
    }

    /// Копия скилла по имени.
    pub async fn skill(&self, name: &str) -> Option<Skill> {
        self.registry.read().await.get(name).ok().cloned()
    }

    pub async fn names(&self) -> Vec<String> {
        self.registry
            .read()
            .await
            .names()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[async_trait]
impl SkillCatalog for SharedRegistry {
    async fn contains(&self, skill: &str) -> bool {
        self.registry.read().await.contains(skill)
    }

    async fn help_text(&self) -> String {
        self.registry.read().await.help_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skill.toml"),
            "mode = \"review\"\ntools = [\"read\", \"write\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: описание {name}\n---\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn loads_and_answers_queries() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "proofread");
        let shared = SharedRegistry::load(tmp.path()).unwrap();

        assert!(shared.contains("proofread").await);
        assert!(!shared.contains("translate").await);
        assert!(shared.skill("proofread").await.is_some());
        assert!(shared.help_text().await.contains("/llm proofread"));
    }

    #[tokio::test]
    async fn reload_picks_up_new_skills() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "proofread");
        let shared = SharedRegistry::load(tmp.path()).unwrap();
        assert_eq!(shared.names().await, vec!["proofread"]);

        write_skill(tmp.path(), "review");
        assert_eq!(shared.reload().await.unwrap(), 2);
        assert!(shared.contains("review").await);
    }

    #[tokio::test]
    async fn broken_reload_keeps_the_old_registry() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "proofread");
        let shared = SharedRegistry::load(tmp.path()).unwrap();

        // Ломаем контракт: patch без tools.
        let broken = tmp.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("skill.toml"), "mode = \"patch\"\n").unwrap();
        std::fs::write(
            broken.join("SKILL.md"),
            "---\nname: broken\ndescription: x\n---\n",
        )
        .unwrap();

        assert!(shared.reload().await.is_err());
        assert!(shared.contains("proofread").await, "прежний реестр цел");
        assert!(!shared.contains("broken").await);
    }
}
