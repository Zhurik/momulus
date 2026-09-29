//! Core errors and their classification for the retry policy.

use std::time::Duration;

/// Error class: decides whether retrying makes sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Temporary problem (network, 5xx, 429, timeout) — worth retrying.
    Transient,
    /// Permanent problem — retrying will not help.
    Permanent,
}

/// Core error type. Library crates return exactly this.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Network(String),

    #[error("server error (HTTP {status}): {message}")]
    HttpServer { status: u16, message: String },

    #[error("client error (HTTP {status}): {message}")]
    HttpClient { status: u16, message: String },

    #[error("rate limited{}", .retry_after.map(|d| format!(", retry after {}s", d.as_secs())).unwrap_or_default())]
    RateLimited { retry_after: Option<Duration> },

    #[error("timed out after {}s", .0.as_secs())]
    Timeout(Duration),

    #[error("invalid model output: {0}")]
    InvalidOutput(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("skill error: {0}")]
    Skill(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("runner error: {0}")]
    Runner(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("input too large: {0}")]
    InputTooLarge(String),

    #[error("cancelled")]
    Cancelled,

    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Classifies the error for the retry policy.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Network(_)
            | Error::HttpServer { .. }
            | Error::RateLimited { .. }
            | Error::Timeout(_) => ErrorKind::Transient,
            _ => ErrorKind::Permanent,
        }
    }

    /// Whether the operation may be retried.
    pub fn is_transient(&self) -> bool {
        self.kind() == ErrorKind::Transient
    }

    /// How long to wait before retrying, when the server told us.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Error::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }

    /// Builds an error from an HTTP status: 429 and 5xx are transient, the rest is not.
    pub fn from_status(status: u16, message: impl Into<String>) -> Self {
        match status {
            429 => Error::RateLimited { retry_after: None },
            500..=599 => Error::HttpServer {
                status,
                message: message.into(),
            },
            _ => Error::HttpClient {
                status,
                message: message.into(),
            },
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Error::Internal(message.into())
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::TimedOut => Error::Network(err.to_string()),
            std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted => Error::Network(err.to_string()),
            _ => Error::Internal(err.to_string()),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_errors_are_retryable() {
        assert!(Error::Network("boom".into()).is_transient());
        assert!(
            Error::HttpServer {
                status: 502,
                message: "bad gateway".into()
            }
            .is_transient()
        );
        assert!(Error::RateLimited { retry_after: None }.is_transient());
        assert!(Error::Timeout(Duration::from_secs(1)).is_transient());
    }

    #[test]
    fn permanent_errors_are_not_retryable() {
        assert!(!Error::InvalidOutput("bad json".into()).is_transient());
        assert!(!Error::Config("missing field".into()).is_transient());
        assert!(
            !Error::HttpClient {
                status: 404,
                message: "not found".into()
            }
            .is_transient()
        );
        assert!(!Error::Unsupported("forks".into()).is_transient());
    }

    #[test]
    fn from_status_classifies() {
        assert_eq!(
            Error::from_status(429, "slow down").kind(),
            ErrorKind::Transient
        );
        assert_eq!(Error::from_status(503, "oops").kind(), ErrorKind::Transient);
        assert_eq!(Error::from_status(404, "nope").kind(), ErrorKind::Permanent);
        assert_eq!(Error::from_status(422, "nope").kind(), ErrorKind::Permanent);
    }

    #[test]
    fn retry_after_is_exposed() {
        let err = Error::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        };
        assert_eq!(err.retry_after(), Some(Duration::from_secs(30)));
        assert_eq!(err.to_string(), "rate limited, retry after 30s");
    }
}
