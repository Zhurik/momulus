//! Publisher: публикует ревью, патчи, реакции и комментарии в GitHub.

use std::sync::Arc;

use async_trait::async_trait;
use llm_bot_core::{
    AckState, Error, Finding, GitAccess, JobRef, Patch, PrRef, Publisher, Result,
    config::GithubConfig,
};
use llm_bot_workspace::Git;
use url::Url;

use crate::app::ClientProvider;
use crate::backoff::Backoff;
use crate::error::from_octocrab;
use crate::models::{
    CommentRequest, CreatePullRequest, CreatedPullRequest, ReactionRequest, ReviewCommentRequest,
    ReviewRequest,
};

/// Сколько строк diff'а показываем в блоке suggestion — ровно одну строку.
const SUGGESTION_FENCE: &str = "suggestion";

/// Реализация [`Publisher`] для GitHub.
pub struct GithubPublisher {
    clients: Arc<dyn ClientProvider>,
    git: Git,
    config: GithubConfig,
    backoff: Backoff,
    /// Откуда берём токен для git push.
    access: Arc<dyn GitAccess>,
}

impl std::fmt::Debug for GithubPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubPublisher")
            .field("api_base", &self.config.api_base)
            .finish()
    }
}

impl GithubPublisher {
    pub fn new(
        clients: Arc<dyn ClientProvider>,
        git: Git,
        config: GithubConfig,
        access: Arc<dyn GitAccess>,
    ) -> GithubPublisher {
        GithubPublisher {
            clients,
            git,
            config,
            backoff: Backoff::default(),
            access,
        }
    }

    pub fn with_backoff(mut self, backoff: Backoff) -> Self {
        self.backoff = backoff;
        self
    }

    /// Тело inline-комментария: текст плюс блок предложения, если он есть.
    pub fn comment_body(finding: &Finding) -> String {
        let mut body = format!("**{}**: {}", finding.severity.as_str(), finding.body.trim());
        if let Some(suggestion) = finding.suggestion.as_deref().map(str::trim_end)
            && !suggestion.trim().is_empty()
        {
            body.push_str(&format!("\n\n```{SUGGESTION_FENCE}\n{suggestion}\n```"));
        }
        body
    }

    /// Собирает тело запроса создания ревью.
    pub fn review_request(pr: &PrRef, findings: &[Finding], summary: &str) -> ReviewRequest {
        ReviewRequest {
            commit_id: pr.head_sha.clone(),
            body: summary.to_string(),
            event: "COMMENT",
            comments: findings
                .iter()
                .map(|finding| ReviewCommentRequest {
                    path: finding.path.clone(),
                    line: finding.line,
                    side: "RIGHT",
                    body: GithubPublisher::comment_body(finding),
                })
                .collect(),
        }
    }

    /// POST без разбора ответа.
    async fn post_empty<B: serde::Serialize + Sync>(
        &self,
        pr: &PrRef,
        route: &str,
        body: &B,
    ) -> Result<()> {
        let client = self.clients.client(pr).await?;
        self.backoff
            .retry(route, || async {
                client
                    .post::<_, serde_json::Value>(route, Some(body))
                    .await
                    .map_err(from_octocrab)
                    .map(|_| ())
            })
            .await
    }

    /// Путь до реакций на комментарий нужного типа.
    fn reactions_route(pr: &PrRef, job: &JobRef) -> String {
        let kind = match job.comment.kind {
            llm_bot_core::CommentKind::Issue => "issues",
            llm_bot_core::CommentKind::Review => "pulls",
        };
        format!(
            "/repos/{}/{}/{kind}/comments/{}/reactions",
            pr.owner, pr.repo, job.comment.id
        )
    }

    /// Ветка, свободная на remote: при коллизии добавляем короткий SHA.
    async fn free_branch(&self, pr: &PrRef, wanted: &str, token: Option<&str>) -> Result<String> {
        let remote = pr.clone_url.clone();
        let exists = |name: &str| {
            let remote = remote.clone();
            let name = name.to_string();
            let git = self.git.clone();
            let token = token.map(str::to_string);
            async move {
                let out = git
                    .run_with_token(
                        None,
                        [
                            "ls-remote",
                            "--heads",
                            &remote,
                            &format!("refs/heads/{name}"),
                        ],
                        token.as_deref(),
                    )
                    .await?;
                Ok::<bool, Error>(!out.stdout.trim().is_empty())
            }
        };

        if !exists(wanted).await? {
            return Ok(wanted.to_string());
        }
        let suffixed = format!("{wanted}-{}", short_sha(&pr.head_sha));
        if !exists(&suffixed).await? {
            return Ok(suffixed);
        }
        Err(Error::Git(format!(
            "ветки {wanted} и {suffixed} уже заняты"
        )))
    }
}

/// Первые семь символов SHA.
fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

#[async_trait]
impl Publisher for GithubPublisher {
    async fn ack(&self, job: &JobRef, state: AckState) -> Result<()> {
        let content = match state {
            AckState::Received => "eyes",
            AckState::Succeeded => "+1",
            AckState::Failed => "-1",
        };
        let route = GithubPublisher::reactions_route(&job.pr, job);
        self.post_empty(&job.pr, &route, &ReactionRequest { content })
            .await
    }

    async fn post_review(&self, pr: &PrRef, findings: &[Finding], summary: &str) -> Result<()> {
        let route = format!(
            "/repos/{}/{}/pulls/{}/reviews",
            pr.owner, pr.repo, pr.number
        );
        let body = GithubPublisher::review_request(pr, findings, summary);
        self.post_empty(pr, &route, &body).await
    }

    async fn push_and_open_pr(&self, pr: &PrRef, patch: &Patch) -> Result<Url> {
        if pr.is_fork() {
            return Err(Error::Unsupported(format!(
                "PR из форка {}: push в него недоступен",
                pr.head_repo
            )));
        }

        let token = self.access.git_token(pr).await?;
        let worktree = patch.worktree.as_path();

        // Коммит делает оркестратор: агент git не касается.
        self.git.run(Some(worktree), ["add", "--all"]).await?;
        self.git
            .run(
                Some(worktree),
                [
                    "-c",
                    &format!("user.name={}", self.config.bot_name),
                    "-c",
                    &format!("user.email={}", self.config.bot_email),
                    "commit",
                    "--quiet",
                    "-m",
                    &patch.commit_message,
                ],
            )
            .await?;

        let branch = self
            .free_branch(pr, &patch.branch, token.as_deref())
            .await?;
        self.git
            .run_with_token(
                Some(worktree),
                [
                    "push",
                    "--quiet",
                    &pr.clone_url,
                    &format!("HEAD:refs/heads/{branch}"),
                ],
                token.as_deref(),
            )
            .await?;

        let route = format!("/repos/{}/{}/pulls", pr.owner, pr.repo);
        let body = CreatePullRequest {
            title: patch.title.clone(),
            head: branch.clone(),
            // База — head-ветка исходного PR: изменения вливаются в него.
            base: pr.head_ref.clone(),
            body: patch.body.clone(),
            maintainer_can_modify: true,
        };
        let client = self.clients.client(pr).await?;
        let created: CreatedPullRequest = self
            .backoff
            .retry(&route, || async {
                client
                    .post::<_, CreatedPullRequest>(&route, Some(&body))
                    .await
                    .map_err(from_octocrab)
            })
            .await?;

        Url::parse(&created.html_url)
            .map_err(|e| Error::Internal(format!("ссылка на PR не разбирается: {e}")))
    }

    async fn comment(&self, pr: &PrRef, body: &str) -> Result<()> {
        let route = format!(
            "/repos/{}/{}/issues/{}/comments",
            pr.owner, pr.repo, pr.number
        );
        self.post_empty(
            pr,
            &route,
            &CommentRequest {
                body: body.to_string(),
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_bot_core::{CommentKind, CommentRef, JobId, Severity};

    fn pr() -> PrRef {
        PrRef {
            platform: "github".into(),
            owner: "acme".into(),
            repo: "blog".into(),
            number: 42,
            head_sha: "abc1234def5678".into(),
            head_ref: "feature".into(),
            base_ref: "main".into(),
            head_repo: "acme/blog".into(),
            clone_url: "https://github.com/acme/blog.git".into(),
        }
    }

    fn finding(suggestion: Option<&str>) -> Finding {
        Finding {
            path: "posts/dns.md".into(),
            line: 18,
            severity: Severity::Typo,
            body: "Тавтология: «красивое красивое»".into(),
            suggestion: suggestion.map(str::to_string),
        }
    }

    #[test]
    fn comment_body_without_suggestion() {
        let body = GithubPublisher::comment_body(&finding(None));
        assert_eq!(body, "**typo**: Тавтология: «красивое красивое»");
    }

    #[test]
    fn comment_body_with_suggestion_block() {
        let body = GithubPublisher::comment_body(&finding(Some("красивое доменное имя\n")));
        insta::assert_snapshot!("suggestion_comment", body);
    }

    #[test]
    fn blank_suggestion_is_ignored() {
        let body = GithubPublisher::comment_body(&finding(Some("   ")));
        assert!(!body.contains("suggestion"), "{body}");
    }

    #[test]
    fn review_request_payload_snapshot() {
        let request = GithubPublisher::review_request(
            &pr(),
            &[finding(Some("красивое доменное имя")), finding(None)],
            "Нашёл две проблемы.",
        );
        insta::assert_json_snapshot!("review_request", request);
    }

    #[test]
    fn review_request_without_findings_omits_comments() {
        let request = GithubPublisher::review_request(&pr(), &[], "Замечаний нет.");
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("comments").is_none(), "{json}");
        assert_eq!(json["event"], "COMMENT");
        assert_eq!(json["commit_id"], "abc1234def5678");
    }

    #[test]
    fn reactions_route_depends_on_comment_kind() {
        let job = |kind| JobRef {
            id: JobId::new(),
            pr: pr(),
            comment: CommentRef {
                id: 555,
                kind,
                author: "zhurik".into(),
                url: None,
            },
        };
        assert_eq!(
            GithubPublisher::reactions_route(&pr(), &job(CommentKind::Issue)),
            "/repos/acme/blog/issues/comments/555/reactions"
        );
        assert_eq!(
            GithubPublisher::reactions_route(&pr(), &job(CommentKind::Review)),
            "/repos/acme/blog/pulls/comments/555/reactions"
        );
    }

    #[test]
    fn short_sha_is_seven_chars() {
        assert_eq!(short_sha("abc1234def5678"), "abc1234");
        assert_eq!(short_sha("abc"), "abc");
    }
}
