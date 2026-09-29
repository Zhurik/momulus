//! A thin wrapper around `git` built on `tokio::process::Command`.
//!
//! The token never reaches an on-disk URL or the command line: it is handed to
//! a credential helper through an environment variable.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use momulus_core::{Error, Redactor, Result};
use tokio::process::Command;

/// The variable the credential helper reads the password from.
const TOKEN_ENV: &str = "MOMULUS_GIT_TOKEN";

/// A credential helper that echoes the token from the environment and writes nothing to disk.
const CREDENTIAL_HELPER: &str = concat!(
    "!f() { ",
    "echo username=x-access-token; ",
    "echo password=\"$MOMULUS_GIT_TOKEN\"; ",
    "}; f"
);

/// Result of running git.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    /// stdout without the trailing newline.
    pub fn trimmed(&self) -> &str {
        self.stdout.trim_end_matches(['\n', '\r'])
    }

    /// Non-empty lines of stdout.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.stdout.lines().filter(|line| !line.is_empty())
    }
}

/// Runs git, masking secrets in everything that reaches logs and errors.
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

    /// Runs git without network access (or with credentials already in place).
    pub async fn run<I, S>(&self, cwd: Option<&Path>, args: I) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_inner(cwd, args, None).await
    }

    /// Runs git with a token for accessing the remote repository.
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
            // No user configs: behaviour is identical on a developer machine,
            // inside the container and in tests.
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
                "{printable}: could not start git: {}",
                self.redactor.redact(&e.to_string())
            ))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = self
            .redactor
            .redact(&String::from_utf8_lossy(&output.stderr));

        if !output.status.success() {
            let message = format!(
                "{printable} exited with code {}: {}",
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

/// Whether this looks like a temporary network problem (then the job can be retried).
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
