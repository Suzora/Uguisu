//! The persistent retry schedule: exponential backoff with jitter, capped,
//! with `Retry-After` preferred when the server sends one.

use std::time::Duration;

use time::OffsetDateTime;
use uguisu_core::config::DownloadConfig;
use uguisu_http::RetryPolicy;

/// Computes when the next attempt may start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPlan {
    policy: RetryPolicy,
}

impl RetryPlan {
    /// Builds the plan from the download configuration: `max_attempts`
    /// attempts, `backoff_base · 2^(attempt-1)` capped at `backoff_max`,
    /// up to 25 % jitter, `Retry-After` honoured up to `backoff_max`.
    #[must_use]
    pub fn from_config(cfg: &DownloadConfig) -> Self {
        Self {
            policy: RetryPolicy {
                max_attempts: cfg.max_attempts.max(1),
                base: cfg.backoff_base,
                cap: cfg.backoff_max,
                max_retry_after: cfg.backoff_max,
                jitter: true,
            },
        }
    }

    /// A plan without jitter (tests).
    #[must_use]
    pub fn deterministic(max_attempts: u32, base: Duration, cap: Duration) -> Self {
        Self {
            policy: RetryPolicy {
                max_attempts: max_attempts.max(1),
                base,
                cap,
                max_retry_after: cap,
                jitter: false,
            },
        }
    }

    /// The attempt budget.
    #[must_use]
    pub const fn max_attempts(&self) -> u32 {
        self.policy.max_attempts
    }

    /// Whether another attempt is allowed after `attempts_made` attempts.
    #[must_use]
    pub const fn allows_another(&self, attempts_made: u32) -> bool {
        attempts_made < self.policy.max_attempts
    }

    /// Delay before the attempt following `attempts_made` (1-based count of
    /// attempts already made). A `Retry-After` at or below the cap wins;
    /// a longer one is ignored in favour of the backoff.
    #[must_use]
    pub fn delay(&self, attempts_made: u32, retry_after: Option<Duration>) -> Duration {
        let attempt = attempts_made.max(1);
        if let Some(d) = self.policy.delay(attempt, retry_after) {
            return d;
        }
        self.policy.delay(attempt, None).unwrap_or(self.policy.cap)
    }

    /// The instant the next attempt may start, rounded **up** to a whole
    /// second: the schedule is persisted at second precision and the
    /// claim query compares whole seconds, so rounding down could fire a
    /// retry (and a `Retry-After`) early.
    #[must_use]
    pub fn next_attempt_at(
        &self,
        now: OffsetDateTime,
        attempts_made: u32,
        retry_after: Option<Duration>,
    ) -> OffsetDateTime {
        ceil_second(now + self.delay(attempts_made, retry_after))
    }
}

/// Rounds a timestamp up to the next whole second (identity on whole seconds).
fn ceil_second(at: OffsetDateTime) -> OffsetDateTime {
    if at.nanosecond() == 0 {
        return at;
    }
    let floor = at.replace_nanosecond(0).unwrap_or(at);
    floor + Duration::from_secs(1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn backoff_doubles_caps_and_prefers_retry_after() {
        let p = RetryPlan::deterministic(8, Duration::from_secs(30), Duration::from_secs(6 * 3600));
        assert_eq!(p.delay(1, None), Duration::from_secs(30));
        assert_eq!(p.delay(2, None), Duration::from_secs(60));
        assert_eq!(p.delay(3, None), Duration::from_secs(120));
        assert_eq!(p.delay(20, None), Duration::from_secs(6 * 3600));
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(7 * 3600))),
            Duration::from_secs(30),
            "a Retry-After beyond the cap falls back to the backoff"
        );
        assert!(p.allows_another(7));
        assert!(!p.allows_another(8));
        let cfg = DownloadConfig::default();
        let from_cfg = RetryPlan::from_config(&cfg);
        assert_eq!(from_cfg.max_attempts(), 8);
    }

    proptest! {
        #[test]
        fn delays_stay_within_bounds(
            attempts in 1u32..64,
            base_ms in 1u64..100_000,
            cap_factor in 1u64..1000,
            ra in prop::option::of(0u64..100_000_000),
        ) {
            let base = Duration::from_millis(base_ms);
            let cap = Duration::from_millis(base_ms * cap_factor);
            let jittered = RetryPlan {
                policy: RetryPolicy { max_attempts: 8, base, cap, max_retry_after: cap, jitter: true },
            };
            let d = jittered.delay(attempts, ra.map(Duration::from_millis));
            match ra.map(Duration::from_millis) {
                Some(r) if r <= cap => prop_assert_eq!(d, r),
                _ => {
                    prop_assert!(d >= base.min(cap));
                    prop_assert!(d <= cap + cap / 4);
                }
            }
        }
    }
}
