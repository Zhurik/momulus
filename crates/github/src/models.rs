//! Минимальные модели GitHub API: только поля, которые нам нужны.

use chrono::{DateTime, Utc};
use momulus_core::{CommentKind, CommentRef, PrRef};
use serde::{Deserialize, Serialize};

use crate::PLATFORM;

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub login: String,
    /// "User" или "Bot" — на комментарии ботов не реагируем.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
}

impl User {
    pub fn is_bot(&self) -> bool {
        self.kind.as_deref() == Some("Bot")
    }
}

/// Комментарий к issue или PR (`/repos/{o}/{r}/issues/comments`).
#[derive(Debug, Clone, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    #[serde(default)]
    pub body: Option<String>,
    pub user: User,
    pub html_url: String,
    pub issue_url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl IssueComment {
    /// Комментарий относится к pull request, а не к issue.
    ///
    /// В этом ответе API нет отдельного признака, зато html_url у PR всегда
    /// содержит `/pull/`.
    pub fn is_pull_request(&self) -> bool {
        self.html_url.contains("/pull/")
    }

    /// Номер PR из issue_url.
    pub fn pr_number(&self) -> Option<u64> {
        last_path_number(&self.issue_url)
    }

    pub fn comment_ref(&self) -> CommentRef {
        CommentRef {
            id: self.id,
            kind: CommentKind::Issue,
            author: self.user.login.clone(),
            url: Some(self.html_url.clone()),
        }
    }
}

/// Комментарий в треде ревью (`/repos/{o}/{r}/pulls/comments`).
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    #[serde(default)]
    pub body: Option<String>,
    pub user: User,
    pub html_url: String,
    pub pull_request_url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ReviewComment {
    pub fn pr_number(&self) -> Option<u64> {
        last_path_number(&self.pull_request_url)
    }

    pub fn comment_ref(&self) -> CommentRef {
        CommentRef {
            id: self.id,
            kind: CommentKind::Review,
            author: self.user.login.clone(),
            url: Some(self.html_url.clone()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    pub full_name: String,
    #[serde(default)]
    pub clone_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Branch {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
    #[serde(default)]
    pub repo: Option<Repo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub state: String,
    pub head: Branch,
    pub base: Branch,
    #[serde(default)]
    pub draft: Option<bool>,
}

impl PullRequest {
    /// Собирает платформенно-независимую ссылку на PR.
    pub fn pr_ref(&self, owner: &str, repo: &str) -> PrRef {
        let base_full_name = format!("{owner}/{repo}");
        PrRef {
            platform: PLATFORM.to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            number: self.number,
            head_sha: self.head.sha.clone(),
            head_ref: self.head.ref_name.clone(),
            base_ref: self.base.ref_name.clone(),
            head_repo: self
                .head
                .repo
                .as_ref()
                .map(|r| r.full_name.clone())
                .unwrap_or_else(|| base_full_name.clone()),
            clone_url: self
                .base
                .repo
                .as_ref()
                .and_then(|r| r.clone_url.clone())
                .unwrap_or_else(|| format!("https://github.com/{base_full_name}.git")),
        }
    }

    pub fn is_open(&self) -> bool {
        self.state == "open"
    }
}

/// Установка приложения.
#[derive(Debug, Clone, Deserialize)]
pub struct Installation {
    pub id: u64,
}

/// Ответ `/installation/repositories`.
#[derive(Debug, Clone, Deserialize)]
pub struct InstallationRepositories {
    pub repositories: Vec<Repo>,
}

/// Ответ создания pull request.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatedPullRequest {
    pub number: u64,
    pub html_url: String,
}

/// Тело запроса `POST /repos/{o}/{r}/pulls/{n}/reviews`.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewRequest {
    pub commit_id: String,
    pub body: String,
    pub event: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<ReviewCommentRequest>,
}

/// Inline-комментарий ревью.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewCommentRequest {
    pub path: String,
    pub line: u32,
    pub side: &'static str,
    pub body: String,
}

/// Тело запроса создания PR.
#[derive(Debug, Clone, Serialize)]
pub struct CreatePullRequest {
    pub title: String,
    pub head: String,
    pub base: String,
    pub body: String,
    pub maintainer_can_modify: bool,
}

/// Тело запроса обычного комментария.
#[derive(Debug, Clone, Serialize)]
pub struct CommentRequest {
    pub body: String,
}

/// Тело запроса реакции.
#[derive(Debug, Clone, Serialize)]
pub struct ReactionRequest {
    pub content: &'static str,
}

/// Последнее числовое звено пути URL.
fn last_path_number(url: &str) -> Option<u64> {
    url.rsplit('/')
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUE_COMMENT: &str = r#"{
        "id": 1001,
        "body": "/llm proofread",
        "user": { "login": "zhurik", "type": "User" },
        "html_url": "https://github.com/acme/blog/pull/42#issuecomment-1001",
        "issue_url": "https://api.github.com/repos/acme/blog/issues/42",
        "created_at": "2026-09-29T10:00:00Z",
        "updated_at": "2026-09-29T10:00:00Z"
    }"#;

    #[test]
    fn parses_issue_comment() {
        let comment: IssueComment = serde_json::from_str(ISSUE_COMMENT).unwrap();
        assert_eq!(comment.id, 1001);
        assert_eq!(comment.body.as_deref(), Some("/llm proofread"));
        assert!(comment.is_pull_request());
        assert_eq!(comment.pr_number(), Some(42));
        assert!(!comment.user.is_bot());
        assert_eq!(comment.comment_ref().kind, CommentKind::Issue);
    }

    #[test]
    fn issue_comment_on_a_plain_issue_is_skipped() {
        let raw = ISSUE_COMMENT.replace("/pull/42#issuecomment", "/issues/42#issuecomment");
        let comment: IssueComment = serde_json::from_str(&raw).unwrap();
        assert!(!comment.is_pull_request());
    }

    #[test]
    fn detects_bot_authors() {
        let raw = ISSUE_COMMENT.replace("\"type\": \"User\"", "\"type\": \"Bot\"");
        let comment: IssueComment = serde_json::from_str(&raw).unwrap();
        assert!(comment.user.is_bot());
    }

    #[test]
    fn tolerates_missing_body() {
        let raw = ISSUE_COMMENT.replace("\"body\": \"/llm proofread\",", "");
        let comment: IssueComment = serde_json::from_str(&raw).unwrap();
        assert!(comment.body.is_none());
    }

    #[test]
    fn parses_review_comment() {
        let raw = r#"{
            "id": 2002,
            "body": "/llm review",
            "user": { "login": "zhurik", "type": "User" },
            "html_url": "https://github.com/acme/blog/pull/7#discussion_r2002",
            "pull_request_url": "https://api.github.com/repos/acme/blog/pulls/7",
            "created_at": "2026-09-29T10:00:00Z",
            "updated_at": "2026-09-29T10:05:00Z"
        }"#;
        let comment: ReviewComment = serde_json::from_str(raw).unwrap();
        assert_eq!(comment.pr_number(), Some(7));
        assert_eq!(comment.comment_ref().kind, CommentKind::Review);
    }

    const PULL: &str = r#"{
        "number": 42,
        "state": "open",
        "draft": false,
        "head": {
            "ref": "feature",
            "sha": "abc123",
            "repo": { "full_name": "acme/blog", "clone_url": "https://github.com/acme/blog.git" }
        },
        "base": {
            "ref": "main",
            "sha": "def456",
            "repo": { "full_name": "acme/blog", "clone_url": "https://github.com/acme/blog.git" }
        }
    }"#;

    #[test]
    fn builds_pr_ref() {
        let pull: PullRequest = serde_json::from_str(PULL).unwrap();
        let pr = pull.pr_ref("acme", "blog");
        assert_eq!(pr.platform, "github");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.head_sha, "abc123");
        assert_eq!(pr.head_ref, "feature");
        assert_eq!(pr.base_ref, "main");
        assert_eq!(pr.clone_url, "https://github.com/acme/blog.git");
        assert!(!pr.is_fork());
        assert!(pull.is_open());
    }

    #[test]
    fn detects_fork_head() {
        let raw = PULL.replacen("acme/blog", "contributor/blog", 1);
        let pull: PullRequest = serde_json::from_str(&raw).unwrap();
        assert!(pull.pr_ref("acme", "blog").is_fork());
    }

    #[test]
    fn missing_head_repo_means_same_repo() {
        let pull: PullRequest = serde_json::from_str(
            r#"{"number":1,"state":"open","head":{"ref":"f","sha":"s"},"base":{"ref":"main","sha":"b"}}"#,
        )
        .unwrap();
        let pr = pull.pr_ref("acme", "blog");
        assert!(!pr.is_fork());
        assert_eq!(pr.clone_url, "https://github.com/acme/blog.git");
    }

    #[test]
    fn parses_installation_repositories() {
        let raw = r#"{"total_count":1,"repositories":[{"full_name":"acme/blog","clone_url":"https://github.com/acme/blog.git"}]}"#;
        let repos: InstallationRepositories = serde_json::from_str(raw).unwrap();
        assert_eq!(repos.repositories[0].full_name, "acme/blog");
    }

    #[test]
    fn extracts_trailing_numbers() {
        assert_eq!(last_path_number("https://x/repos/a/b/issues/42"), Some(42));
        assert_eq!(last_path_number("https://x/repos/a/b/pulls/7/"), Some(7));
        assert_eq!(last_path_number("https://x/repos/a/b/issues/abc"), None);
    }
}
