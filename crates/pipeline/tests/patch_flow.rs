//! Сборка патча из настоящей рабочей копии.

use std::path::Path;
use std::process::Command;

use momulus_pipeline::collect_patch;
use momulus_workspace::{Git, RepoCache};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git запускается");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Репозиторий с одной статьёй; возвращает head-коммит.
fn fixture_repo(dir: &Path) -> String {
    std::fs::create_dir_all(dir.join("posts")).unwrap();
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(dir.join("posts/hello.mdx"), "# Привет\n\nтекст\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "-m", "статья"]);
    git(dir, &["rev-parse", "HEAD"])
}

async fn worktree_fixture(tmp: &Path) -> (RepoCache, std::path::PathBuf, String) {
    let origin = tmp.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    let head = fixture_repo(&origin);
    let cache = RepoCache::new(tmp.join("repos"), Git::default());
    let bare = cache
        .sync(
            "acme",
            "blog",
            &origin.to_string_lossy(),
            &["+refs/heads/*:refs/heads/*".to_string()],
            None,
        )
        .await
        .unwrap();
    (cache, bare, head)
}

#[tokio::test]
async fn collects_changes_made_in_the_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let (cache, bare, head) = worktree_fixture(tmp.path()).await;
    let worktree = cache
        .worktree(&bare, &tmp.path().join("work/job"), &head)
        .await
        .unwrap();

    // Так работает patch-скилл: правит существующий файл и создаёт новый.
    std::fs::write(
        worktree.path().join("posts/hello.mdx"),
        "# Привет\n\nисправленный текст\n",
    )
    .unwrap();
    std::fs::write(worktree.path().join("posts/hello.en.mdx"), "# Hello\n").unwrap();

    let patch = collect_patch(
        &worktree,
        "llm/translate-en-42".into(),
        "momulus: translate для #42".into(),
        "тело PR".into(),
        "momulus: перевод".into(),
    )
    .await
    .unwrap()
    .expect("изменения есть");

    assert_eq!(patch.branch, "llm/translate-en-42");
    assert_eq!(
        patch.files,
        vec![
            "posts/hello.en.mdx".to_string(),
            "posts/hello.mdx".to_string()
        ]
    );
    assert_eq!(patch.worktree, worktree.path());
}

#[tokio::test]
async fn no_changes_means_no_patch() {
    let tmp = tempfile::tempdir().unwrap();
    let (cache, bare, head) = worktree_fixture(tmp.path()).await;
    let worktree = cache
        .worktree(&bare, &tmp.path().join("work/job"), &head)
        .await
        .unwrap();

    let patch = collect_patch(
        &worktree,
        "llm/translate-en-42".into(),
        "t".into(),
        "b".into(),
        "c".into(),
    )
    .await
    .unwrap();

    assert!(patch.is_none(), "скилл ничего не изменил");
}

#[tokio::test]
async fn rewriting_a_file_with_the_same_content_is_not_a_change() {
    let tmp = tempfile::tempdir().unwrap();
    let (cache, bare, head) = worktree_fixture(tmp.path()).await;
    let worktree = cache
        .worktree(&bare, &tmp.path().join("work/job"), &head)
        .await
        .unwrap();

    // Агент «переписал» файл тем же содержимым — коммитить нечего.
    std::fs::write(
        worktree.path().join("posts/hello.mdx"),
        "# Привет\n\nтекст\n",
    )
    .unwrap();

    let patch = collect_patch(
        &worktree,
        "llm/x".into(),
        "t".into(),
        "b".into(),
        "c".into(),
    )
    .await
    .unwrap();
    assert!(patch.is_none());
}
