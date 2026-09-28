//! Тонкая обёртка над `git` через `tokio::process::Command`.
//!
//! Токен никогда не попадает ни в URL на диске, ни в аргументы команды:
//! он передаётся в credential helper через переменную окружения.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use llm_bot_core::{Error, Redactor, Result};
use tokio::process::Command;

/// Переменная, из которой credential helper берёт пароль.
const TOKEN_ENV: &str = "LLM_BOT_GIT_TOKEN";

/// Credential helper, который отдаёт токен из окружения и ничего не пишет на диск.
const CREDENTIAL_HELPER: &str = concat!(
    "!f() { ",
    "echo username=x-access-token; ",
    "echo password=\"$LLM_BOT_GIT_TOKEN\"; ",
    "}; f"
);

/// Результат выполнения git.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    /// stdout без хвостового перевода строки.
    pub fn trimmed(&self) -> &str {
        self.stdout.trim_end_matches(['\n', '\r'])
    }

    /// Непустые строки stdout.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.stdout.lines().filter(|line| !line.is_empty())
    }
}

/// Запускает git, маскируя секреты во всём, что уходит в логи и ошибки.
#[derive(Debug, Clone, Default)]
pub struct Git {
    redactor: Redactor,
}

impl Git {
    pub fn new(redactor: Redactor) -> Self {
        Git { redactor }
    }

    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// Запускает git без доступа к сети (или с уже настроенными креденшлами).
    pub async fn run<I, S>(&self, cwd: Option<&Path>, args: I) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_inner(cwd, args, None).await
    }

    /// Запускает git с токеном для доступа к удалённому репозиторию.
    pub async fn run_with_token<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        token: Option<&str>,
    ) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_inner(cwd, args, token).await
    }

    async fn run_inner<I, S>(
        &self,
        cwd: Option<&Path>,
        args: I,
        token: Option<&str>,
    ) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            // Без пользовательских конфигов: поведение одинаковое у разработчика,
            // в контейнере и в тестах.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(token) = token {
            command
                .arg("-c")
                .arg(format!("credential.helper={CREDENTIAL_HELPER}"))
                .env(TOKEN_ENV, token);
        }

        let args: Vec<String> = args
            .into_iter()
            .map(|a| a.as_ref().to_string_lossy().to_string())
            .collect();
        command.args(&args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        let printable = self.redactor.redact(&format!("git {}", args.join(" ")));
        tracing::debug!(command = %printable, "running git");

        let output = command.output().await.map_err(|e| {
            Error::Git(format!(
                "{printable}: не удалось запустить git: {}",
                self.redactor.redact(&e.to_string())
            ))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = self
            .redactor
            .redact(&String::from_utf8_lossy(&output.stderr));

        if !output.status.success() {
            let message = format!(
                "{printable} завершился с кодом {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            );
            return Err(if is_transient(&stderr) {
                Error::Network(message)
            } else {
                Error::Git(message)
            });
        }

        Ok(GitOutput {
            stdout: self.redactor.redact(&stdout),
            stderr,
        })
    }
}

/// Похоже ли на временную сетевую проблему (тогда джобу можно повторить).
fn is_transient(stderr: &str) -> bool {
    const MARKERS: [&str; 8] = [
        "could not resolve host",
        "connection timed out",
        "connection reset",
        "the remote end hung up",
        "early eof",
        "failed to connect",
        "returned error: 5",
        "rpc failed",
    ];
    let lower = stderr.to_ascii_lowercase();
    MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_git_and_returns_stdout() {
        let git = Git::default();
        let out = git.run(None, ["--version"]).await.unwrap();
        assert!(out.trimmed().starts_with("git version"), "{out:?}");
    }

    #[tokio::test]
    async fn failure_includes_stderr() {
        let git = Git::default();
        let err = git.run(None, ["cat-file", "-p", "nope"]).await.unwrap_err();
        assert!(matches!(err, Error::Git(_)), "{err:?}");
    }

    #[tokio::test]
    async fn token_never_appears_in_error_text() {
        let git = Git::new(Redactor::new().with_secret("ghs_supersecrettoken"));
        let err = git
            .run_with_token(
                None,
                [
                    "ls-remote",
                    "https://x-access-token:ghs_supersecrettoken@127.0.0.1:1/x.git",
                ],
                Some("ghs_supersecrettoken"),
            )
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(!text.contains("ghs_supersecrettoken"), "{text}");
    }

    #[test]
    fn network_errors_are_classified_as_transient() {
        assert!(is_transient(
            "fatal: unable to access: Could not resolve host: github.com"
        ));
        assert!(is_transient("error: RPC failed; curl 92"));
        assert!(!is_transient("fatal: not a git repository"));
    }
}
