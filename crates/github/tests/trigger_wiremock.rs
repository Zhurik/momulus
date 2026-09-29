//! Trigger against a fake GitHub API.

use std::sync::Arc;
use std::time::Duration;

use momulus_core::{CommentKind, CursorStore, MemoryCursorStore, StaticSkillCatalog};
use momulus_github::backoff::Backoff;
use momulus_github::trigger::{ClientSource, STREAM_ISSUE, STREAM_REVIEW};
use momulus_github::{GithubTrigger, TriggerConfig};
use octocrab::Octocrab;
use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// An octocrab client pointed at the mock server.
fn client(server: &MockServer) -> Octocrab {
    Octocrab::builder()
        .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
        .base_uri(server.uri())
        .unwrap()
        .user_access_token("test-token".to_string())
        .build()
        .unwrap()
}

fn trigger(
    server: &MockServer,
    store: Arc<MemoryCursorStore>,
    allowed: Vec<String>,
) -> GithubTrigger {
    GithubTrigger::new(
        TriggerConfig {
            poll_interval: Duration::from_millis(10),
            allowed_users: allowed,
            repos: vec!["acme/blog".to_string()],
        },
        ClientSource::Fixed {
            client: client(server),
            repos: vec!["acme/blog".to_string()],
        },
        store,
        Arc::new(StaticSkillCatalog::new(["proofread", "translate"])),
    )
    .with_backoff(Backoff {
        attempts: 3,
        base: Duration::from_millis(1),
        max: Duration::from_millis(5),
    })
}

fn issue_comment(id: u64, body: &str, author: &str, updated: &str) -> serde_json::Value {
    json!({
        "id": id,
        "body": body,
        "user": { "login": author, "type": "User" },
        "html_url": format!("https://github.com/acme/blog/pull/42#issuecomment-{id}"),
        "issue_url": "https://api.github.com/repos/acme/blog/issues/42",
        "created_at": updated,
        "updated_at": updated
    })
}

fn pull_json() -> serde_json::Value {
    json!({
        "number": 42,
        "state": "open",
        "head": {
            "ref": "feature",
            "sha": "abc1234def",
            "repo": { "full_name": "acme/blog", "clone_url": "https://github.com/acme/blog.git" }
        },
        "base": {
            "ref": "main",
            "sha": "base999",
            "repo": { "full_name": "acme/blog", "clone_url": "https://github.com/acme/blog.git" }
        }
    })
}

/// An empty review-comment stream: most tests do not need it.
async fn mock_empty_review_comments(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
}

#[tokio::test]
async fn command_in_issue_comment_becomes_a_job() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .and(query_param("sort", "updated"))
        .and(query_param_is_missing("since"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            issue_comment(1001, "just chatting", "zhurik", "2026-09-29T09:00:00Z"),
            issue_comment(1002, "/llm proofread", "zhurik", "2026-09-29T10:00:00Z"),
        ])))
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json()))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    let jobs = trigger(&server, store.clone(), vec!["zhurik".into()])
        .poll_once()
        .await
        .unwrap();

    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.command.skill, "proofread");
    assert_eq!(job.comment.id, 1002);
    assert_eq!(job.comment.kind, CommentKind::Issue);
    assert_eq!(job.pr.number, 42);
    assert_eq!(job.pr.head_sha, "abc1234def");
    assert_eq!(job.pr.base_ref, "main");

    // The cursor moved to the newest comment.
    let cursor = store
        .cursor("github:acme/blog", STREAM_ISSUE)
        .await
        .unwrap()
        .expect("the cursor was stored");
    assert_eq!(cursor.to_rfc3339(), "2026-09-29T10:00:00+00:00");
}

#[tokio::test]
async fn second_poll_sends_since_and_skips_duplicates() {
    let server = MockServer::start().await;
    // The first request has no since, the second one does.
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .and(query_param_is_missing("since"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([issue_comment(
                1002,
                "/llm proofread",
                "zhurik",
                "2026-09-29T10:00:00Z"
            )])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .and(query_param("since", "2026-09-29T10:00:00+00:00"))
        // GitHub treats since as inclusive — the same comment comes back.
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([issue_comment(
                1002,
                "/llm proofread",
                "zhurik",
                "2026-09-29T10:00:00Z"
            )])),
        )
        .expect(1)
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json()))
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    let trigger = trigger(&server, store.clone(), vec!["zhurik".into()]);

    assert_eq!(trigger.poll_once().await.unwrap().len(), 1);
    assert_eq!(
        trigger.poll_once().await.unwrap().len(),
        0,
        "deduplicated by comment id"
    );
}

#[tokio::test]
async fn command_from_a_stranger_is_ignored_silently() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([issue_comment(
                1003,
                "/llm proofread",
                "stranger",
                "2026-09-29T10:00:00Z"
            )])),
        )
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;

    let store = Arc::new(MemoryCursorStore::new());
    let jobs = trigger(&server, store, vec!["zhurik".into()])
        .poll_once()
        .await
        .unwrap();

    assert!(jobs.is_empty());
    // No comments, no PR lookup — nothing at all.
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| r.method == wiremock::http::Method::GET),
        "the bot has nothing to say to a stranger"
    );
}

#[tokio::test]
async fn unknown_skill_gets_a_help_comment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([issue_comment(
                1004,
                "/llm proofraed",
                "zhurik",
                "2026-09-29T10:00:00Z"
            )])),
        )
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    let jobs = trigger(&server, store, vec!["zhurik".into()])
        .poll_once()
        .await
        .unwrap();
    assert!(jobs.is_empty());

    let posted = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method == wiremock::http::Method::POST)
        .expect("a help comment was posted");
    let body: serde_json::Value = serde_json::from_slice(&posted.body).unwrap();
    let text = body["body"].as_str().unwrap();
    assert!(text.contains("proofraed"), "{text}");
    assert!(text.contains("/llm proofread"), "{text}");
}

#[tokio::test]
async fn malformed_command_gets_a_help_comment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([issue_comment(
                1005,
                "/llm",
                "zhurik",
                "2026-09-29T10:00:00Z"
            )])),
        )
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    assert!(
        trigger(&server, store, vec!["zhurik".into()])
            .poll_once()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn command_in_review_thread_becomes_a_job() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": 2001,
            "body": "/llm review",
            "user": { "login": "zhurik", "type": "User" },
            "html_url": "https://github.com/acme/blog/pull/42#discussion_r2001",
            "pull_request_url": "https://api.github.com/repos/acme/blog/pulls/42",
            "created_at": "2026-09-29T10:00:00Z",
            "updated_at": "2026-09-29T10:00:00Z"
        }])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json()))
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    // The catalog has "review" here — this checks the review-comment stream.
    let trigger = GithubTrigger::new(
        TriggerConfig {
            poll_interval: Duration::from_millis(10),
            allowed_users: vec!["zhurik".into()],
            repos: Vec::new(),
        },
        ClientSource::Fixed {
            client: client(&server),
            repos: vec!["acme/blog".into()],
        },
        store.clone(),
        Arc::new(StaticSkillCatalog::new(["review"])),
    );
    let jobs = trigger.poll_once().await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].comment.kind, CommentKind::Review);
    assert!(
        store
            .cursor("github:acme/blog", STREAM_REVIEW)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn closed_pull_request_is_answered_not_queued() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([issue_comment(
                1006,
                "/llm proofread",
                "zhurik",
                "2026-09-29T10:00:00Z"
            )])),
        )
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;
    let mut closed = pull_json();
    closed["state"] = json!("closed");
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(closed))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/blog/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    assert!(
        trigger(&server, store, vec!["zhurik".into()])
            .poll_once()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rate_limit_is_retried_with_backoff() {
    let server = MockServer::start().await;
    // The first response is a 429, the second one is a normal list.
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "message": "API rate limit exceeded",
            "documentation_url": "https://docs.github.com"
        })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([issue_comment(
                1007,
                "/llm proofread",
                "zhurik",
                "2026-09-29T10:00:00Z"
            )])),
        )
        .expect(1)
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json()))
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    let jobs = trigger(&server, store, vec!["zhurik".into()])
        .poll_once()
        .await
        .unwrap();
    assert_eq!(
        jobs.len(),
        1,
        "after the retry the command was parsed after all"
    );
}

#[tokio::test]
async fn server_error_after_all_attempts_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "boom" })))
        .expect(3)
        .mount(&server)
        .await;

    let store = Arc::new(MemoryCursorStore::new());
    let err = trigger(&server, store, vec!["zhurik".into()])
        .poll_once()
        .await
        .unwrap_err();
    assert!(err.is_transient(), "{err:?}");
}

#[tokio::test]
async fn repo_outside_allowlist_is_not_polled() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryCursorStore::new());
    let trigger = GithubTrigger::new(
        TriggerConfig {
            poll_interval: Duration::from_millis(10),
            allowed_users: vec!["zhurik".into()],
            repos: vec!["other/repo".into()],
        },
        ClientSource::Fixed {
            client: client(&server),
            repos: vec!["acme/blog".into()],
        },
        store,
        Arc::new(StaticSkillCatalog::new(["proofread"])),
    );

    assert!(trigger.poll_once().await.unwrap().is_empty());
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "the API was not called at all"
    );
}

#[tokio::test]
async fn app_mode_discovers_repositories_of_installations() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/app/installations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 777 }])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/installation/repositories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 1,
            "repositories": [
                { "full_name": "acme/blog", "clone_url": "https://github.com/acme/blog.git" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/app/installations/777/access_tokens"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "token": "ghs_installation_token",
            "expires_at": "2026-09-29T12:00:00Z",
            "permissions": { "contents": "write", "pull_requests": "write" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/blog/issues/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    mock_empty_review_comments(&server).await;

    let key = include_str!("data/test-app-key.pem");
    let auth = momulus_github::AppAuth::new(12345, key.as_bytes(), &server.uri()).unwrap();
    let trigger = GithubTrigger::new(
        TriggerConfig {
            poll_interval: Duration::from_millis(10),
            allowed_users: vec!["zhurik".into()],
            repos: Vec::new(),
        },
        ClientSource::App(auth),
        Arc::new(MemoryCursorStore::new()),
        Arc::new(StaticSkillCatalog::new(["proofread"])),
    );

    assert!(trigger.poll_once().await.unwrap().is_empty());
}
