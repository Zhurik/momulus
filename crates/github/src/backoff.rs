//! Повторы с экспоненциальной задержкой и джиттером, с учётом rate limit.

use std::time::Duration;

use llm_bot_core::Result;

/// Политика повторов для вызовов API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// Сколько всего попыток (включая первую).
    pub attempts: u32,
    pub base: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            attempts: 4,
            base: Duration::from_millis(500),
            max: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// Задержка перед попыткой номер `attempt` (нумерация с 1), без джиттера.
    pub fn delay(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(16);
        let scaled = self.base.saturating_mul(2u32.saturating_pow(exponent));
        scaled.min(self.max)
    }

    /// Задержка с джиттером ±25% — чтобы повторы не били в одну секунду.
    pub fn delay_with_jitter(&self, attempt: u32) -> Duration {
        jitter(self.delay(attempt))
    }

    /// Выполняет операцию, повторяя транзиентные ошибки.
    ///
    /// Если сервер сказал, сколько ждать (`Retry-After` через
    /// [`Error::RateLimited`]), уважаем его значение.
    pub async fn retry<T, F, Fut>(&self, what: &str, mut operation: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 1;
        loop {
            match operation().await {
                Ok(value) => return Ok(value),
                Err(err) if err.is_transient() && attempt < self.attempts => {
                    let wait = err
                        .retry_after()
                        .map(jitter)
                        .unwrap_or_else(|| self.delay_with_jitter(attempt));
                    tracing::warn!(
                        what,
                        attempt,
                        wait_ms = wait.as_millis() as u64,
                        error = %err,
                        "повторяем запрос"
                    );
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }
}

/// Добавляет к задержке случайные ±25%.
fn jitter(base: Duration) -> Duration {
    if base.is_zero() {
        return base;
    }
    let millis = base.as_millis() as u64;
    let spread = (millis / 4).max(1);
    let offset: u64 = rand::random_range(0..=2 * spread);
    Duration::from_millis(millis.saturating_sub(spread).saturating_add(offset))
}

/// Сколько ждать по заголовкам ответа GitHub.
///
/// Смотрим `Retry-After`, затем `X-RateLimit-Remaining`/`X-RateLimit-Reset`.
pub fn wait_from_headers(
    retry_after: Option<&str>,
    remaining: Option<&str>,
    reset_epoch: Option<&str>,
    now_epoch: i64,
) -> Option<Duration> {
    if let Some(seconds) = retry_after.and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(Duration::from_secs(seconds));
    }
    let remaining = remaining.and_then(|v| v.trim().parse::<u64>().ok())?;
    if remaining > 0 {
        return None;
    }
    let reset = reset_epoch.and_then(|v| v.trim().parse::<i64>().ok())?;
    let wait = reset.saturating_sub(now_epoch).max(0);
    Some(Duration::from_secs(wait as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_bot_core::Error;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn delay_grows_exponentially_and_is_capped() {
        let backoff = Backoff {
            attempts: 6,
            base: Duration::from_millis(100),
            max: Duration::from_millis(500),
        };
        assert_eq!(backoff.delay(1), Duration::from_millis(100));
        assert_eq!(backoff.delay(2), Duration::from_millis(200));
        assert_eq!(backoff.delay(3), Duration::from_millis(400));
        assert_eq!(
            backoff.delay(4),
            Duration::from_millis(500),
            "упёрлись в max"
        );
        assert_eq!(backoff.delay(10), Duration::from_millis(500));
    }

    #[test]
    fn jitter_stays_within_quarter() {
        let base = Duration::from_millis(1000);
        for _ in 0..100 {
            let value = jitter(base).as_millis() as u64;
            assert!((750..=1250).contains(&value), "{value}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retries_transient_errors_then_succeeds() {
        let calls = AtomicU32::new(0);
        let backoff = Backoff {
            attempts: 4,
            base: Duration::from_millis(10),
            max: Duration::from_millis(50),
        };
        let value = backoff
            .retry("test", || async {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(Error::HttpServer {
                        status: 502,
                        message: "bad gateway".into(),
                    })
                } else {
                    Ok(n)
                }
            })
            .await
            .unwrap();
        assert_eq!(value, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_after_attempts() {
        let calls = AtomicU32::new(0);
        let backoff = Backoff {
            attempts: 3,
            base: Duration::from_millis(1),
            max: Duration::from_millis(5),
        };
        let err = backoff
            .retry("test", || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(Error::RateLimited { retry_after: None })
            })
            .await
            .unwrap_err();
        assert!(matches!(err, Error::RateLimited { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn permanent_errors_are_not_retried() {
        let calls = AtomicU32::new(0);
        let err = Backoff::default()
            .retry("test", || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(Error::HttpClient {
                    status: 404,
                    message: "not found".into(),
                })
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::HttpClient { status: 404, .. }),
            "{err:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_after_header_wins() {
        assert_eq!(
            wait_from_headers(Some("30"), Some("0"), Some("999999"), 0),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn rate_limit_reset_is_used_when_quota_is_gone() {
        assert_eq!(
            wait_from_headers(None, Some("0"), Some("150"), 100),
            Some(Duration::from_secs(50))
        );
    }

    #[test]
    fn no_wait_while_quota_remains() {
        assert_eq!(wait_from_headers(None, Some("42"), Some("150"), 100), None);
        assert_eq!(wait_from_headers(None, None, None, 100), None);
    }

    #[test]
    fn past_reset_means_no_wait() {
        assert_eq!(
            wait_from_headers(None, Some("0"), Some("50"), 100),
            Some(Duration::from_secs(0))
        );
    }
}
