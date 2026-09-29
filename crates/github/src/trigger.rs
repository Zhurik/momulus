//! Trigger: опрашивает GitHub и превращает команды в джобы.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use llm_bot_core::{
    Command, CommandParseError, CursorStore, Error, Job, PrRef, Result, SkillCatalog, Trigger,
};
use octocrab::Octocrab;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::PLATFORM;
use crate::backoff::Backoff;
use crate::error::from_octocrab;
use crate::models::{
    CommentRequest, Installation, InstallationRepositories, IssueComment, PullRequest,
    ReviewComment,
};

/// Имена потоков комментариев — они же ключи курсоров.
pub const STREAM_ISSUE: &str = "issue_comments";
pub const STREAM_REVIEW: &str = "review_comments";

/// Сколько комментариев берём за один запрос.
const PER_PAGE: u8 = 100;

/// Настройки опроса.
#[derive(Debug, Clone)]
pub struct TriggerConfig {
    pub poll_interval: Duration,
    /// Кому разрешено запускать команды.
    pub allowed_users: Vec<String>,
    /// Белый список "owner/repo"; пусто — все репозитории установок.
    pub repos: Vec<String>,
}

impl TriggerConfig {
    fn is_allowed_user(&self, login: &str) -> bool {
        self.allowed_users
            .iter()
            .any(|u| u.eq_ignore_ascii_case(login))
    }

    fn is_allowed_repo(&self, full_name: &str) -> bool {
        self.repos.is_empty() || self.repos.iter().any(|r| r.eq_ignore_ascii_case(full_name))
    }
}

/// Откуда Trigger берёт клиентов и список репозиториев.
pub enum ClientSource {
    /// Обычный режим: все установки приложения.
    App(Arc<crate::AppAuth>),
    /// Фиксированный клиент и заранее известные репозитории —
    /// для одного репозитория и для тестов.
    Fixed {
        client: Octocrab,
        repos: Vec<String>,
    },
}

/// Репозиторий, который опрашиваем.
struct Target {
    client: Octocrab,
    owner: String,
    repo: String,
}

impl Target {
    fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    fn repo_key(&self) -> String {
        format!("{PLATFORM}:{}/{}", self.owner, self.repo)
    }
}

/// Реализация [`Trigger`] через периодический опрос REST API.
pub struct GithubTrigger {
    config: TriggerConfig,
    source: ClientSource,
    store: Arc<dyn CursorStore>,
    catalog: Arc<dyn SkillCatalog>,
    backoff: Backoff,
}

/// Параметры запроса списка комментариев.
#[derive(Debug, Serialize)]
struct ListParams<'a> {
    sort: &'a str,
    direction: &'a str,
    per_page: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
}

impl GithubTrigger {
    pub fn new(
        config: TriggerConfig,
        source: ClientSource,
        store: Arc<dyn CursorStore>,
        catalog: Arc<dyn SkillCatalog>,
    ) -> GithubTrigger {
        GithubTrigger {
            config,
            source,
            store,
            catalog,
            backoff: Backoff::default(),
        }
    }

    pub fn with_backoff(mut self, backoff: Backoff) -> Self {
        self.backoff = backoff;
        self
    }

    /// Один проход опроса: возвращает найденные джобы.
    pub async fn poll_once(&self) -> Result<Vec<Job>> {
        let mut jobs = Vec::new();
        for target in self.targets().await? {
            if !self.config.is_allowed_repo(&target.full_name()) {
                continue;
            }
            jobs.extend(self.poll_issue_comments(&target).await?);
            jobs.extend(self.poll_review_comments(&target).await?);
        }
        Ok(jobs)
    }

    /// Репозитории, которые надо опросить.
    async fn targets(&self) -> Result<Vec<Target>> {
        match &self.source {
            ClientSource::Fixed { client, repos } => Ok(repos
                .iter()
                .filter_map(|full| full.split_once('/'))
                .map(|(owner, repo)| Target {
                    client: client.clone(),
                    owner: owner.to_string(),
                    repo: repo.to_string(),
                })
                .collect()),
            ClientSource::App(auth) => {
                let installations: Vec<Installation> = self
                    .backoff
                    .retry("/app/installations", || async {
                        auth.app_client()
                            .get("/app/installations", None::<&()>)
                            .await
                            .map_err(from_octocrab)
                    })
                    .await?;

                let mut targets = Vec::new();
                for installation in installations {
                    let client = auth.installation_client(installation.id)?;
                    let repos: InstallationRepositories = self
                        .backoff
                        .retry("/installation/repositories", || async {
                            client
                                .get("/installation/repositories", None::<&()>)
                                .await
                                .map_err(from_octocrab)
                        })
                        .await?;
                    for repo in repos.repositories {
                        if let Some((owner, name)) = repo.full_name.split_once('/') {
                            targets.push(Target {
                                client: client.clone(),
                                owner: owner.to_string(),
                                repo: name.to_string(),
                            });
                        }
                    }
                }
                Ok(targets)
            }
        }
    }

    async fn poll_issue_comments(&self, target: &Target) -> Result<Vec<Job>> {
        let since = self.store.cursor(&target.repo_key(), STREAM_ISSUE).await?;
        let route = format!("/repos/{}/{}/issues/comments", target.owner, target.repo);
        let comments: Vec<IssueComment> = self.list(target, &route, since).await?;

        let mut jobs = Vec::new();
        let mut newest = since;
        for comment in comments {
            newest = max_time(newest, comment.updated_at);
            if !comment.is_pull_request() || comment.user.is_bot() {
                continue;
            }
            let Some(number) = comment.pr_number() else {
                continue;
            };
            if let Some(job) = self
                .handle_comment(
                    target,
                    number,
                    comment.body.as_deref().unwrap_or_default(),
                    &comment.user.login,
                    comment.comment_ref(),
                )
                .await?
            {
                jobs.push(job);
            }
        }

        if let Some(newest) = newest {
            self.store
                .set_cursor(&target.repo_key(), STREAM_ISSUE, newest)
                .await?;
        }
        Ok(jobs)
    }

    async fn poll_review_comments(&self, target: &Target) -> Result<Vec<Job>> {
        let since = self.store.cursor(&target.repo_key(), STREAM_REVIEW).await?;
        let route = format!("/repos/{}/{}/pulls/comments", target.owner, target.repo);
        let comments: Vec<ReviewComment> = self.list(target, &route, since).await?;

        let mut jobs = Vec::new();
        let mut newest = since;
        for comment in comments {
            newest = max_time(newest, comment.updated_at);
            if comment.user.is_bot() {
                continue;
            }
            let Some(number) = comment.pr_number() else {
                continue;
            };
            if let Some(job) = self
                .handle_comment(
                    target,
                    number,
                    comment.body.as_deref().unwrap_or_default(),
                    &comment.user.login,
                    comment.comment_ref(),
                )
                .await?
            {
                jobs.push(job);
            }
        }

        if let Some(newest) = newest {
            self.store
                .set_cursor(&target.repo_key(), STREAM_REVIEW, newest)
                .await?;
        }
        Ok(jobs)
    }

    /// Запрос списка комментариев с курсором.
    async fn list<T: serde::de::DeserializeOwned>(
        &self,
        target: &Target,
        route: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<T>> {
        let params = ListParams {
            sort: "updated",
            direction: "asc",
            per_page: PER_PAGE,
            since: since.map(|t| t.to_rfc3339()),
        };
        self.backoff
            .retry(route, || async {
                target
                    .client
                    .get::<Vec<T>, _, _>(route, Some(&params))
                    .await
                    .map_err(from_octocrab)
            })
            .await
    }

    /// Разбирает комментарий и, если это годная команда, делает джобу.
    ///
    /// Всё, что не команда или не от разрешённого пользователя, игнорируется молча.
    async fn handle_comment(
        &self,
        target: &Target,
        pr_number: u64,
        body: &str,
        author: &str,
        comment: llm_bot_core::CommentRef,
    ) -> Result<Option<Job>> {
        let parsed = match Command::parse(body) {
            Ok(command) => Ok(command),
            // Не команда — молчим.
            Err(CommandParseError::NotACommand) => return Ok(None),
            Err(err) => Err(err),
        };

        if !self.config.is_allowed_user(author) {
            tracing::debug!(author, "команда от пользователя вне allowed_users");
            return Ok(None);
        }

        // Один комментарий обрабатываем ровно один раз.
        if !self.store.mark_seen(PLATFORM, comment.id).await? {
            return Ok(None);
        }

        let command = match parsed {
            Ok(command) => command,
            Err(err) => {
                self.reply_help(target, pr_number, &err.to_string()).await?;
                return Ok(None);
            }
        };

        if !self.catalog.contains(&command.skill).await {
            self.reply_help(
                target,
                pr_number,
                &format!("неизвестный скилл \"{}\"", command.skill),
            )
            .await?;
            return Ok(None);
        }

        let pull = self.fetch_pull(target, pr_number).await?;
        if !pull.is_open() {
            self.reply(
                target,
                pr_number,
                &format!(
                    "Команда `{}` не выполнена: pull request уже закрыт.",
                    command.to_command_line()
                ),
            )
            .await?;
            return Ok(None);
        }

        let pr: PrRef = pull.pr_ref(&target.owner, &target.repo);
        Ok(Some(Job::new(pr, command, comment)))
    }

    async fn fetch_pull(&self, target: &Target, number: u64) -> Result<PullRequest> {
        let route = format!("/repos/{}/{}/pulls/{number}", target.owner, target.repo);
        self.backoff
            .retry(&route, || async {
                target
                    .client
                    .get::<PullRequest, _, _>(&route, None::<&()>)
                    .await
                    .map_err(from_octocrab)
            })
            .await
    }

    /// Отвечает подсказкой на непонятную команду.
    async fn reply_help(&self, target: &Target, pr_number: u64, reason: &str) -> Result<()> {
        let body = format!(
            "Не понял команду: {reason}\n\n{}",
            self.catalog.help_text().await.trim_end()
        );
        self.reply(target, pr_number, &body).await
    }

    async fn reply(&self, target: &Target, pr_number: u64, body: &str) -> Result<()> {
        let route = format!(
            "/repos/{}/{}/issues/{pr_number}/comments",
            target.owner, target.repo
        );
        let request = CommentRequest {
            body: body.to_string(),
        };
        self.backoff
            .retry(&route, || async {
                target
                    .client
                    .post::<_, serde_json::Value>(&route, Some(&request))
                    .await
                    .map_err(from_octocrab)
                    .map(|_| ())
            })
            .await
    }
}

fn max_time(current: Option<DateTime<Utc>>, candidate: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match current {
        Some(current) if current >= candidate => Some(current),
        _ => Some(candidate),
    }
}

#[async_trait::async_trait]
impl Trigger for GithubTrigger {
    async fn run(&self, tx: mpsc::Sender<Job>, shutdown: CancellationToken) -> Result<()> {
        // Считаем подряд идущие сбои, чтобы не молотить API в пустую.
        let mut failures: u32 = 0;
        loop {
            match self.poll_once().await {
                Ok(jobs) => {
                    failures = 0;
                    for job in jobs {
                        if tx.send(job).await.is_err() {
                            tracing::info!("очередь закрыта, останавливаем опрос");
                            return Ok(());
                        }
                    }
                }
                Err(Error::Cancelled) => return Ok(()),
                Err(err) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(error = %err, failures, "опрос не удался");
                }
            }

            let wait = if failures == 0 {
                self.config.poll_interval
            } else {
                self.backoff
                    .delay_with_jitter(failures)
                    .max(self.config.poll_interval)
            };

            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TriggerConfig {
        TriggerConfig {
            poll_interval: Duration::from_secs(45),
            allowed_users: vec!["Zhurik".into()],
            repos: vec!["acme/blog".into()],
        }
    }

    #[test]
    fn user_allowlist_is_case_insensitive() {
        assert!(config().is_allowed_user("zhurik"));
        assert!(!config().is_allowed_user("stranger"));
    }

    #[test]
    fn repo_allowlist_filters() {
        assert!(config().is_allowed_repo("Acme/Blog"));
        assert!(!config().is_allowed_repo("other/repo"));
        let mut open = config();
        open.repos.clear();
        assert!(open.is_allowed_repo("anything/goes"));
    }

    #[test]
    fn list_params_serialize_with_and_without_since() {
        let with = serde_json::to_value(ListParams {
            sort: "updated",
            direction: "asc",
            per_page: 100,
            since: Some("2026-09-29T10:00:00+00:00".into()),
        })
        .unwrap();
        assert_eq!(with["since"], "2026-09-29T10:00:00+00:00");

        let without = serde_json::to_value(ListParams {
            sort: "updated",
            direction: "asc",
            per_page: 100,
            since: None,
        })
        .unwrap();
        assert!(without.get("since").is_none(), "{without}");
    }

    #[test]
    fn newest_timestamp_wins() {
        let older = "2026-09-29T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let newer = "2026-09-29T11:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(max_time(None, older), Some(older));
        assert_eq!(max_time(Some(older), newer), Some(newer));
        assert_eq!(max_time(Some(newer), older), Some(newer));
    }
}
