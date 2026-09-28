//! Рабочая копия на одну джобу: `git worktree` с гарантированной очисткой.

use std::path::{Path, PathBuf};

use llm_bot_core::{Error, Result};

use crate::git::Git;

/// Worktree, который удаляется при выходе из области видимости —
/// в том числе при ошибке и панике.
#[derive(Debug)]
pub struct Worktree {
    path: PathBuf,
    bare: PathBuf,
    git: Git,
    /// Снимаем флаг, если очистка уже сделана явно.
    armed: bool,
}

impl Worktree {
    /// Создаёт worktree на конкретном коммите в detached-состоянии.
    pub async fn create(git: &Git, bare: &Path, dest: &Path, sha: &str) -> Result<Worktree> {
        if dest.exists() {
            return Err(Error::Git(format!(
                "каталог {} уже существует",
                dest.display()
            )));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        git.run(
            Some(bare),
            [
                "worktree",
                "add",
                "--detach",
                "--force",
                &dest.to_string_lossy(),
                sha,
            ],
        )
        .await?;

        Ok(Worktree {
            path: dest.to_path_buf(),
            bare: bare.to_path_buf(),
            git: git.clone(),
            armed: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    /// Пути файлов, изменённых в рабочей копии (`git status --porcelain`).
    pub async fn dirty_files(&self) -> Result<Vec<String>> {
        let out = self
            .git
            .run(Some(&self.path), ["status", "--porcelain", "-z"])
            .await?;
        Ok(parse_porcelain_z(&out.stdout))
    }

    /// Есть ли в рабочей копии несохранённые изменения.
    pub async fn is_dirty(&self) -> Result<bool> {
        Ok(!self.dirty_files().await?.is_empty())
    }

    /// Diff рабочей копии относительно HEAD, включая новые файлы.
    pub async fn diff(&self) -> Result<String> {
        self.git
            .run(Some(&self.path), ["add", "--intent-to-add", "--all"])
            .await?;
        let out = self
            .git
            .run(Some(&self.path), ["diff", "--no-color", "HEAD"])
            .await?;
        Ok(out.stdout)
    }

    /// Явная очистка с диагностикой (в отличие от Drop, где ошибки только логируются).
    pub async fn cleanup(mut self) -> Result<()> {
        self.armed = false;
        remove_worktree_async(&self.git, &self.bare, &self.path).await
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Drop синхронный, поэтому тут блокирующий git — операция короткая.
        let status = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.bare)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        let removed = matches!(status, Ok(status) if status.success());
        if !removed && self.path.exists() {
            // Если git не справился, убираем каталог руками и чистим метаданные.
            if let Err(err) = std::fs::remove_dir_all(&self.path) {
                tracing::warn!(path = %self.path.display(), error = %err, "не удалось удалить worktree");
            }
            let _ = std::process::Command::new("git")
                .args(["worktree", "prune"])
                .current_dir(&self.bare)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

async fn remove_worktree_async(git: &Git, bare: &Path, path: &Path) -> Result<()> {
    let result = git
        .run(
            Some(bare),
            ["worktree", "remove", "--force", &path.to_string_lossy()],
        )
        .await;
    if result.is_err() && path.exists() {
        std::fs::remove_dir_all(path)?;
        git.run(Some(bare), ["worktree", "prune"]).await?;
    }
    Ok(())
}

/// Разбирает `git status --porcelain -z`: записи разделены нулевым байтом.
fn parse_porcelain_z(raw: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut parts = raw.split('\0').filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let status = &entry[..2];
        let path = entry[3..].to_string();
        // Для переименований следующий элемент — прежнее имя, оно нам не нужно.
        if status.starts_with('R') || status.starts_with('C') {
            let _ = parts.next();
        }
        files.push(path);
    }
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_output() {
        let raw = " M posts/a.mdx\0?? posts/b.mdx\0R  new.md\0old.md\0";
        assert_eq!(
            parse_porcelain_z(raw),
            vec![
                "new.md".to_string(),
                "posts/a.mdx".to_string(),
                "posts/b.mdx".to_string()
            ]
        );
    }

    #[test]
    fn empty_status_means_clean() {
        assert!(parse_porcelain_z("").is_empty());
    }
}
