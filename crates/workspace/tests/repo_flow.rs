//! Integration tests against a real git repository in a temporary directory.

use std::path::Path;
use std::process::Command;

use momulus_workspace::{Git, RepoCache};

/// Runs git inside the test repository.
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
        .expect("git starts");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with a main branch and a feature branch on top of it.
fn fixture_repo(dir: &Path) -> (String, String) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(dir.join("hello.mdx"), "# Hello\n\nold text\ntail\n").unwrap();
    std::fs::write(dir.join("keep.txt"), "unchanged\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "-m", "initial"]);
    let base = git(dir, &["rev-parse", "HEAD"]);

    git(dir, &["checkout", "--quiet", "-b", "feature"]);
    std::fs::write(
        dir.join("hello.mdx"),
        "# Hello\n\nnew text\none more line\ntail\n",
    )
    .unwrap();
    std::fs::write(dir.join("new.md"), "a brand new file\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "-m", "edits"]);
    let head = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["checkout", "--quiet", "main"]);
    (base, head)
}

fn refspecs() -> Vec<String> {
    vec!["+refs/heads/*:refs/heads/*".to_string()]
}

#[tokio::test]
async fn clones_into_bare_cache_and_fetches_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    let (_base, head) = fixture_repo(&origin);

    let cache = RepoCache::new(tmp.path().join("repos"), Git::default());
    let url = origin.to_string_lossy().to_string();
    let bare = cache
        .sync("acme", "blog", &url, &refspecs(), None)
        .await
        .unwrap();

    assert_eq!(bare, tmp.path().join("repos/acme/blog.git"));
    assert!(bare.join("HEAD").exists(), "the bare clone was created");
    assert!(cache.has_commit(&bare, &head).await.unwrap());
    assert!(
        !cache
            .has_commit(&bare, "0".repeat(40).as_str())
            .await
            .unwrap()
    );

    // A new commit in origin is picked up by the second sync call.
    std::fs::write(origin.join("later.md"), "later\n").unwrap();
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "--quiet", "-m", "more"]);
    let later = git(&origin, &["rev-parse", "HEAD"]);
    assert!(!cache.has_commit(&bare, &later).await.unwrap());

    cache
        .sync("acme", "blog", &url, &refspecs(), None)
        .await
        .unwrap();
    assert!(cache.has_commit(&bare, &later).await.unwrap());
}

#[tokio::test]
async fn computes_pr_diff_and_line_mapping() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    fixture_repo(&origin);

    let cache = RepoCache::new(tmp.path().join("repos"), Git::default());
    let bare = cache
        .sync("acme", "blog", &origin.to_string_lossy(), &refspecs(), None)
        .await
        .unwrap();

    let index = cache.pr_diff_index(&bare, "main", "feature").await.unwrap();

    let mut files = index.reviewable_files();
    files.sort();
    assert_eq!(files, vec!["hello.mdx", "new.md"]);

    // Lines 3 and 4 of hello.mdx changed in the new version — they are commentable.
    assert!(index.is_commentable("hello.mdx", 3));
    assert!(index.is_commentable("hello.mdx", 4));
    assert!(!index.is_commentable("hello.mdx", 500));
    assert!(
        !index.is_commentable("keep.txt", 1),
        "file outside the diff"
    );

    let hello = index.get("hello.mdx").unwrap();
    assert_eq!(hello.added_lines(), vec![3, 4]);
}

#[tokio::test]
async fn worktree_is_created_and_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    let (_base, head) = fixture_repo(&origin);

    let cache = RepoCache::new(tmp.path().join("repos"), Git::default());
    let bare = cache
        .sync("acme", "blog", &origin.to_string_lossy(), &refspecs(), None)
        .await
        .unwrap();

    let dest = tmp.path().join("work/job-1");
    {
        let worktree = cache.worktree(&bare, &dest, &head).await.unwrap();
        assert!(dest.join("new.md").exists(), "working copy at the PR head");
        assert!(!worktree.is_dirty().await.unwrap());

        // An edit in the working copy shows up as dirty and lands in the diff.
        std::fs::write(dest.join("hello.mdx"), "# Hello\n\nagent edit\n").unwrap();
        std::fs::write(dest.join("added-by-agent.md"), "new\n").unwrap();
        assert!(worktree.is_dirty().await.unwrap());
        let dirty = worktree.dirty_files().await.unwrap();
        assert_eq!(dirty, vec!["added-by-agent.md", "hello.mdx"]);
        let diff = worktree.diff().await.unwrap();
        assert!(diff.contains("agent edit"), "{diff}");
        assert!(diff.contains("added-by-agent.md"), "{diff}");
    }

    assert!(
        !dest.exists(),
        "the worktree is removed when it goes out of scope"
    );
    let list = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(&bare)
        .output()
        .unwrap();
    let list = String::from_utf8_lossy(&list.stdout);
    assert!(
        !list.contains("job-1"),
        "worktree metadata was pruned: {list}"
    );
}

#[tokio::test]
async fn worktree_is_removed_even_on_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    let (_base, head) = fixture_repo(&origin);

    let cache = RepoCache::new(tmp.path().join("repos"), Git::default());
    let bare = cache
        .sync("acme", "blog", &origin.to_string_lossy(), &refspecs(), None)
        .await
        .unwrap();

    let dest = tmp.path().join("work/job-panic");
    let worktree = cache.worktree(&bare, &dest, &head).await.unwrap();
    assert!(dest.exists());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = worktree;
        panic!("the job blew up");
    }));
    assert!(result.is_err());
    assert!(!dest.exists(), "the worktree is gone after a panic");
}

#[tokio::test]
async fn explicit_cleanup_reports_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    let (_base, head) = fixture_repo(&origin);

    let cache = RepoCache::new(tmp.path().join("repos"), Git::default());
    let bare = cache
        .sync("acme", "blog", &origin.to_string_lossy(), &refspecs(), None)
        .await
        .unwrap();

    let dest = tmp.path().join("work/job-2");
    let worktree = cache.worktree(&bare, &dest, &head).await.unwrap();
    worktree.cleanup().await.unwrap();
    assert!(!dest.exists());
}

#[tokio::test]
async fn second_worktree_at_the_same_path_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    let (_base, head) = fixture_repo(&origin);

    let cache = RepoCache::new(tmp.path().join("repos"), Git::default());
    let bare = cache
        .sync("acme", "blog", &origin.to_string_lossy(), &refspecs(), None)
        .await
        .unwrap();

    let dest = tmp.path().join("work/job-3");
    let _first = cache.worktree(&bare, &dest, &head).await.unwrap();
    let err = cache.worktree(&bare, &dest, &head).await.unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
}
