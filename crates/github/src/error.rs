//! Translating octocrab errors into core errors with the right classification.

use momulus_core::Error;

/// Turns an octocrab error into a core error.
///
/// Preserving the transient/permanent split matters: it decides whether the job
/// gets retried.
pub fn from_octocrab(err: octocrab::Error) -> Error {
    match &err {
        octocrab::Error::GitHub { source, .. } => {
            let status = source.status_code.as_u16();
            let message = source.message.clone();
            // On GitHub a 403 can mean either "no permission" or "rate limited".
            if status == 403 && looks_like_rate_limit(&message) {
                return Error::RateLimited { retry_after: None };
            }
            Error::from_status(status, message)
        }
        octocrab::Error::Hyper { .. } | octocrab::Error::Service { .. } => {
            Error::Network(err.to_string())
        }
        octocrab::Error::Http { .. } | octocrab::Error::Uri { .. } => {
            Error::Network(err.to_string())
        }
        octocrab::Error::SerdeUrlEncoded { .. } | octocrab::Error::Serde { .. } => {
            Error::Internal(err.to_string())
        }
        _ => Error::Internal(err.to_string()),
    }
}

fn looks_like_rate_limit(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("rate limit") || lower.contains("abuse") || lower.contains("secondary rate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_message_is_transient() {
        assert!(looks_like_rate_limit("API rate limit exceeded for user"));
        assert!(looks_like_rate_limit(
            "You have exceeded a secondary rate limit"
        ));
        assert!(!looks_like_rate_limit(
            "Resource not accessible by integration"
        ));
    }
}
