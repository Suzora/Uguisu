//! Per-provider and per-host rate and concurrency limits.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::clock::{Clock, DefaultClock};
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::host::HostKey;

/// How a [`Throttle`] limits traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThrottleConfig {
    /// Sustained requests per minute (`None` = unlimited).
    pub per_minute: Option<u32>,
    /// Burst allowance above the sustained rate (default: 1, i.e. no burst).
    pub burst: u32,
    /// Maximum in-flight requests.
    pub concurrency: usize,
}

impl ThrottleConfig {
    /// `per_minute` requests with the given concurrency and no burst.
    pub const fn per_minute(per_minute: u32, concurrency: usize) -> Self {
        Self {
            per_minute: Some(per_minute),
            burst: 1,
            concurrency,
        }
    }

    /// No rate limit, only a concurrency cap.
    pub const fn concurrency_only(concurrency: usize) -> Self {
        Self {
            per_minute: None,
            burst: 1,
            concurrency,
        }
    }
}

/// Combines a token-bucket rate limiter with a concurrency semaphore.
#[derive(Clone)]
pub struct Throttle {
    limiter: Option<Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>>,
    semaphore: Arc<Semaphore>,
    config: ThrottleConfig,
}

impl std::fmt::Debug for Throttle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Throttle")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Held while a request is in flight; releases the concurrency slot on drop.
#[derive(Debug)]
pub struct ThrottlePermit {
    _permit: OwnedSemaphorePermit,
}

impl Throttle {
    /// Builds a throttle from its configuration.
    pub fn new(config: ThrottleConfig) -> Self {
        let limiter = config
            .per_minute
            .and_then(NonZeroU32::new)
            .map(|per_minute| {
                let burst = NonZeroU32::new(config.burst.max(1)).unwrap_or(NonZeroU32::MIN);
                Arc::new(RateLimiter::direct(
                    Quota::per_minute(per_minute).allow_burst(burst),
                ))
            });
        Self {
            limiter,
            semaphore: Arc::new(Semaphore::new(config.concurrency.max(1))),
            config,
        }
    }

    /// The configuration this throttle was built with.
    pub const fn config(&self) -> &ThrottleConfig {
        &self.config
    }

    /// Waits for a concurrency slot and for the rate limiter, unless cancelled.
    pub async fn acquire(&self, cancel: &CancellationToken) -> Option<ThrottlePermit> {
        let permit = tokio::select! {
            () = cancel.cancelled() => return None,
            p = Arc::clone(&self.semaphore).acquire_owned() => p.ok()?,
        };
        if let Some(limiter) = &self.limiter {
            tokio::select! {
                () = cancel.cancelled() => return None,
                () = limiter.until_ready() => {}
            }
        }
        Some(ThrottlePermit { _permit: permit })
    }

    /// Takes a concurrency slot without waiting, when one is free and the
    /// rate limiter allows a request right now.
    pub fn try_acquire(&self) -> Option<ThrottlePermit> {
        let permit = Arc::clone(&self.semaphore).try_acquire_owned().ok()?;
        if let Some(limiter) = &self.limiter {
            limiter.check().ok()?;
        }
        Some(ThrottlePermit { _permit: permit })
    }

    /// Free concurrency slots right now.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Whether a request could start right now without waiting for the rate limiter.
    pub fn ready(&self) -> bool {
        self.limiter.as_ref().is_none_or(|l| l.check().is_ok())
    }

    /// Approximate wait until the next request is allowed by the rate limiter.
    pub fn estimated_wait(&self) -> Duration {
        match &self.limiter {
            Some(l) => match l.check() {
                Ok(()) => Duration::ZERO,
                Err(not_until) => not_until.wait_time_from(DefaultClock::default().now()),
            },
            None => Duration::ZERO,
        }
    }
}

/// One [`Throttle`] per host, created on first use with a shared default
/// configuration. Entries are never evicted: the map is bounded by the
/// number of distinct hosts a library talks to.
#[derive(Debug)]
pub struct HostThrottles {
    default: ThrottleConfig,
    map: std::sync::Mutex<std::collections::HashMap<HostKey, Throttle>>,
}

impl HostThrottles {
    /// Builds the registry; every host gets `default` until overridden.
    pub fn new(default: ThrottleConfig) -> Self {
        Self {
            default,
            map: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The default per-host configuration.
    pub const fn default_config(&self) -> &ThrottleConfig {
        &self.default
    }

    /// Sets a host-specific configuration (replaces any existing throttle
    /// for that host; permits already handed out stay valid).
    pub fn configure(&self, host: HostKey, config: ThrottleConfig) {
        self.lock().insert(host, Throttle::new(config));
    }

    /// The throttle for `host`, creating it with the default on first use.
    pub fn get(&self, host: &HostKey) -> Throttle {
        self.lock()
            .entry(host.clone())
            .or_insert_with(|| Throttle::new(self.default.clone()))
            .clone()
    }

    /// Takes a slot for `host` without waiting.
    pub fn try_acquire(&self, host: &HostKey) -> Option<ThrottlePermit> {
        self.get(host).try_acquire()
    }

    /// Hosts that currently have no free concurrency slot.
    pub fn saturated(&self) -> Vec<HostKey> {
        self.lock()
            .iter()
            .filter(|(_, t)| t.available_permits() == 0)
            .map(|(k, _)| k.clone())
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<HostKey, Throttle>> {
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::time::Instant;

    use super::*;

    #[test]
    fn host_throttles_track_saturation_per_host() {
        let hosts = HostThrottles::new(ThrottleConfig::concurrency_only(1));
        let a: HostKey = "https://a.example:443".parse().unwrap();
        let b: HostKey = "https://b.example:443".parse().unwrap();
        let pa = hosts.try_acquire(&a).expect("first slot for a");
        assert!(hosts.try_acquire(&a).is_none(), "a is saturated");
        assert_eq!(hosts.saturated(), vec![a.clone()]);
        let _pb = hosts.try_acquire(&b).expect("b is independent");
        let mut sat = hosts.saturated();
        sat.sort();
        assert_eq!(sat, vec![a.clone(), b]);
        drop(pa);
        assert!(hosts.saturated().iter().all(|k| *k != a));
        hosts.configure(a.clone(), ThrottleConfig::concurrency_only(2));
        let _p1 = hosts.try_acquire(&a).unwrap();
        let _p2 = hosts.try_acquire(&a).unwrap();
        assert!(hosts.try_acquire(&a).is_none());
    }

    #[tokio::test]
    async fn concurrency_is_bounded() {
        let t = Throttle::new(ThrottleConfig::concurrency_only(1));
        let cancel = CancellationToken::new();
        let first = t.acquire(&cancel).await.expect("permit");
        let second = tokio::time::timeout(Duration::from_millis(50), t.acquire(&cancel)).await;
        assert!(
            second.is_err(),
            "second acquire must wait while the first permit is held"
        );
        drop(first);
        assert!(t.acquire(&cancel).await.is_some());
    }

    #[tokio::test]
    async fn rate_limit_delays_second_request() {
        // 60/min = one per second; the second call must wait roughly a second.
        let t = Throttle::new(ThrottleConfig::per_minute(60, 4));
        let cancel = CancellationToken::new();
        let _a = t.acquire(&cancel).await;
        assert!(!t.ready());
        let start = Instant::now();
        let _b = t.acquire(&cancel).await;
        assert!(
            start.elapsed() >= Duration::from_millis(900),
            "{:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn cancellation_aborts_wait() {
        let t = Throttle::new(ThrottleConfig::per_minute(1, 1));
        let cancel = CancellationToken::new();
        let _a = t.acquire(&cancel).await;
        cancel.cancel();
        assert!(t.acquire(&cancel).await.is_none());
        let _ = DefaultClock::default().now();
    }
}
