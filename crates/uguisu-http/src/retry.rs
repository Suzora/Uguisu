//! Retry policy for idempotent requests.

use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use http::HeaderMap;

/// Exponential backoff with a cap and optional jitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts including the first one. `1` disables retries.
    pub max_attempts: u32,
    /// Delay before the second attempt; doubles each time.
    pub base: Duration,
    /// Upper bound for a computed delay (a `Retry-After` header may exceed it up to `max_retry_after`).
    pub cap: Duration,
    /// Largest `Retry-After` value honoured; longer waits fail immediately.
    pub max_retry_after: Duration,
    /// Add up to 25 % random jitter.
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(10),
            max_retry_after: Duration::from_secs(30),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// No retries at all.
    pub fn none() -> Self {
        Self {
            max_attempts: 1,
            ..Self::default()
        }
    }

    /// Delay before `attempt` (1-based: the delay before the second attempt is `attempt = 1`).
    /// Returns `None` when the server asked to wait longer than `max_retry_after`.
    pub fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        if let Some(ra) = retry_after {
            return (ra <= self.max_retry_after).then_some(ra);
        }
        let exp = self
            .base
            .saturating_mul(1u32 << attempt.min(16).saturating_sub(1));
        let mut delay = exp.min(self.cap);
        if self.jitter && delay > Duration::ZERO {
            let r = u32::try_from(pseudo_random() % 26).unwrap_or(0); // 0..=25 percent
            delay += delay.mul_f64(f64::from(r) / 100.0);
        }
        Some(delay)
    }
}

/// Parses a `Retry-After` header (delay-seconds or HTTP-date).
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = httpdate::parse_http_date(raw).ok()?;
    Some(
        when.duration_since(std::time::SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

fn pseudo_random() -> u64 {
    // Good enough for jitter; avoids pulling in a random-number crate.
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let p = RetryPolicy {
            jitter: false,
            ..RetryPolicy::default()
        };
        assert_eq!(p.delay(1, None), Some(Duration::from_millis(500)));
        assert_eq!(p.delay(2, None), Some(Duration::from_secs(1)));
        assert_eq!(p.delay(3, None), Some(Duration::from_secs(2)));
        assert_eq!(p.delay(10, None), Some(Duration::from_secs(10)));
    }

    #[test]
    fn retry_after_wins_and_is_bounded() {
        let p = RetryPolicy::default();
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(3))),
            Some(Duration::from_secs(3))
        );
        assert_eq!(p.delay(1, Some(Duration::from_secs(3600))), None);
    }

    #[test]
    fn jitter_stays_within_25_percent() {
        let p = RetryPolicy::default();
        for _ in 0..50 {
            let d = p.delay(1, None).unwrap_or_default();
            assert!(
                d >= Duration::from_millis(500) && d <= Duration::from_millis(625),
                "{d:?}"
            );
        }
    }

    #[test]
    fn parses_retry_after_seconds_and_dates() {
        let mut h = HeaderMap::new();
        h.insert(http::header::RETRY_AFTER, "7".parse().expect("header"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        h.insert(
            http::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().expect("header"),
        );
        assert_eq!(
            retry_after(&h),
            Some(Duration::ZERO),
            "dates in the past mean now"
        );
        h.insert(http::header::RETRY_AFTER, "soon".parse().expect("header"));
        assert_eq!(retry_after(&h), None);
    }
}
