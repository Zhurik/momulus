//! Publisher против фейкового GitHub API и локального bare-remote.

use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;

use llm_bot_core::{
    AckState, CommentKind, CommentRef, Finding, JobId, JobRef, Patch, PrRef, Publisher, Severity,
    config::GithubConfig,
};
use llm_bot_github::GithubPublisher;
use llm_bot_workspace::Git;
use octocrab::Octocrab;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> Octocrab {
    Octocrab::builder()
        .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
        .base_uri(server.uri())
        .unwrap()
        .user_access_token("test-token".to_string())
        .build()
        .unwrap()
}

fn publisher(server: &MockServer) -> GithubPublisher {
    GithubPublisher::new(
        Arc::new(llm_bot_github::FixedClient(client(server))),
        Git::default(),
        GithubConfig::default(),
        Arc::new(llm_bot_core::LocalGitAccess),
    )
    .with_backoff(llm_bot_github::backoff::Backoff {
        attempts: 3,
        base: std::time::Duration::from_millis(1),
        max: std::time::Duration::from_millis(5),
    })
}

fn pr(clone_url: &str, head_repo: &str) -> PrRef {
    PrRef {
        platform: "github".into(),
        owner: "acme".into(),
        repo: "blog".into(),
        number: 42,
        head_sha: "abc1234def5678".into(),
        head_ref: "feature".into(),
        base_ref: "main".into(),
        head_repo: head_repo.into(),
        clone_url: clone_url.into(),
    }
}

fn finding(line: u32, suggestion: Option<&str>) -> Finding {
    Finding {
        path: "posts/dns.md".into(),
        line,
        severity: Severity::Typo,
        body: "опечатка".into(),
        suggestion: suggestion.map(str::to_string),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn posts_review_with_inline_comments() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls/42/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 9 })))
        .expect(1)
        .mount(&server)
        .await;

    publisher(&server)
        .post_review(
            &pr("https://github.com/acme/blog.git", "acme/blog"),
            &[finding(18, Some("исправленная строка")), finding(20, None)],
            "Нашёл две проблемы.",
        )
        .await
        .unwrap();

    let request = server.received_requests().await.unwrap().pop().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["event"], "COMMENT");
    assert_eq!(body["commit_id"], "abc1234def5678");
    assert_eq!(body["body"], "Нашёл две проблемы.");
    let comments = body["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0]["path"], "posts/dns.md");
    assert_eq!(comments[0]["line"], 18);
    assert_eq!(comments[0]["side"], "RIGHT");
    assert!(
        comments[0]["body"]
            .as_str()
            .unwrap()
            .contains("```suggestion\nисправленная строка\n```"),
        "{}",
        comments[0]["body"]
    );
    assert!(!comments[1]["body"].as_str().unwrap().contains("suggestion"));
}

#[tokio::test]
async fn review_without_findings_is_a_plain_comment_review() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls/42/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 9 })))
        .expect(1)
        .mount(&server)
        .await;

    publisher(&server)
        .post_review(
            &pr("https://github.com/acme/blog.git", "acme/blog"),
            &[],
            "Замечаний нет.",
        )
        .await
        .unwrap();

    let request = server.received_requests().await.unwrap().pop().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert!(body.get("comments").is_none(), "{body}");
}

#[tokio::test]
async fn acks_use_the_right_reaction_and_route() {
    for (state, content, kind, route) in [
        (
            AckState::Received,
            "eyes",
            CommentKind::Issue,
            "/repos/acme/blog/issues/comments/555/reactions",
        ),
        (
            AckState::Succeeded,
            "+1",
            CommentKind::Review,
            "/repos/acme/blog/pulls/comments/555/reactions",
        ),
        (
            AckState::Failed,
            "-1",
            CommentKind::Issue,
            "/repos/acme/blog/issues/comments/555/reactions",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
            .expect(1)
            .mount(&server)
            .await;

        let job = JobRef {
            id: JobId::new(),
            pr: pr("https://github.com/acme/blog.git", "acme/blog"),
            comment: CommentRef {
                id: 555,
                kind,
                author: "zhurik".into(),
                url: None,
            },
        };
        publisher(&server).ack(&job, state).await.unwrap();

        let request = server.received_requests().await.unwrap().pop().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["content"], content);
    }
}

#[tokio::test]
async fn comment_is_posted_to_the_pull_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
        .expect(1)
        .mount(&server)
        .await;

    publisher(&server)
        .comment(
            &pr("https://github.com/acme/blog.git", "acme/blog"),
            "нечего делать",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn rate_limited_review_is_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls/42/reviews"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "message": "You have exceeded a secondary rate limit"
        })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls/42/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 9 })))
        .expect(1)
        .mount(&server)
        .await;

    publisher(&server)
        .post_review(
            &pr("https://github.com/acme/blog.git", "acme/blog"),
            &[],
            "итог",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn permanent_error_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls/42/reviews"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Unprocessable Entity"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let err = publisher(&server)
        .post_review(
            &pr("https://github.com/acme/blog.git", "acme/blog"),
            &[],
            "итог",
        )
        .await
        .unwrap_err();
    assert!(!err.is_transient(), "{err:?}");
}

/// Готовит bare-remote и рабочую копию с изменением.
fn patch_fixture(tmp: &Path) -> (String, Patch) {
    let origin = tmp.join("origin.git");
    let seed = tmp.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(seed.join("hello.mdx"), "# Привет\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "--quiet", "-m", "начало"]);
    git(&seed, &["branch", "feature"]);
    git(
        &seed,
        &["clone", "--bare", "--quiet", ".", origin.to_str().unwrap()],
    );

    let work = tmp.join("work");
    git(
        tmp,
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    std::fs::write(work.join("hello.en.mdx"), "# Hello\n").unwrap();

    (
        origin.to_string_lossy().to_string(),
        Patch {
            worktree: work,
            branch: "llm/translate-en-42".into(),
            commit_message: "llm-bot: перевод".into(),
            title: "llm-bot: translate для #42".into(),
            body: "тело PR".into(),
            files: vec!["hello.en.mdx".into()],
        },
    )
}

#[tokio::test]
async fn pushes_branch_and_opens_pull_request() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, patch) = patch_fixture(tmp.path());

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 43,
            "html_url": "https://github.com/acme/blog/pull/43"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let url = publisher(&server)
        .push_and_open_pr(&pr(&origin, "acme/blog"), &patch)
        .await
        .unwrap();
    assert_eq!(url.as_str(), "https://github.com/acme/blog/pull/43");

    // Ветка появилась в bare-remote, и в ней есть наш файл.
    let branches = git(Path::new(&origin), &["branch", "--list"]);
    assert!(branches.contains("llm/translate-en-42"), "{branches}");
    let files = git(
        Path::new(&origin),
        &["ls-tree", "--name-only", "llm/translate-en-42"],
    );
    assert!(files.contains("hello.en.mdx"), "{files}");

    // Автор коммита — бот из конфига.
    let author = git(
        Path::new(&origin),
        &["log", "-1", "--format=%an <%ae>", "llm/translate-en-42"],
    );
    assert_eq!(author, "llm-bot <llm-bot@users.noreply.github.com>");

    // База нового PR — head-ветка исходного.
    let request = server.received_requests().await.unwrap().pop().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["base"], "feature");
    assert_eq!(body["head"], "llm/translate-en-42");
    assert_eq!(body["title"], "llm-bot: translate для #42");
}

#[tokio::test]
async fn branch_collision_gets_a_sha_suffix() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, patch) = patch_fixture(tmp.path());
    // Занимаем желаемое имя ветки заранее.
    git(
        Path::new(&origin),
        &["branch", "llm/translate-en-42", "main"],
    );

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 44,
            "html_url": "https://github.com/acme/blog/pull/44"
        })))
        .mount(&server)
        .await;

    publisher(&server)
        .push_and_open_pr(&pr(&origin, "acme/blog"), &patch)
        .await
        .unwrap();

    let branches = git(Path::new(&origin), &["branch", "--list"]);
    assert!(
        branches.contains("llm/translate-en-42-abc1234"),
        "к имени добавлен short SHA: {branches}"
    );

    let request = server.received_requests().await.unwrap().pop().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["head"], "llm/translate-en-42-abc1234");
}

#[tokio::test]
async fn fork_pull_request_is_rejected_before_any_git_work() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, patch) = patch_fixture(tmp.path());

    let server = MockServer::start().await;
    let err = publisher(&server)
        .push_and_open_pr(&pr(&origin, "contributor/blog"), &patch)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("форка"), "{err}");
    assert!(!err.is_transient());
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "к API не обращались"
    );
}
