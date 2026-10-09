//! Provider registry: wraps every provider with throttling, health tracking,
//! timeouts and caching, and exposes their status.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use time::OffsetDateTime;
use uguisu_core::provider::ProviderId;
use uguisu_core::redact;
use uguisu_http::Throttle;

use crate::cache::{CacheKind, CachedValue, DiscoveryCache};
use crate::candidate::PodcastCandidate;
use crate::provider::{
    DiscoveryProvider, ProviderContext, ProviderError, ProviderInfo, ProviderRef, ProviderResponse,
};
use crate::query::NormalizedQuery;

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum CircuitState {
    /// Requests flow.
    Closed,
    /// Requests are skipped until the cool-down ends.
    Open {
        /// Seconds until the next probe.
        retry_in_secs: u64,
    },
    /// One probe request is allowed.
    HalfOpen,
}

/// Health of one provider.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProviderHealth {
    /// Circuit state.
    pub circuit: CircuitState,
    /// Consecutive failures.
    pub consecutive_failures: u32,
    /// Last successful call.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_success: Option<OffsetDateTime>,
    /// Last failure and its kind.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_failure: Option<OffsetDateTime>,
    /// Kind of the last failure.
    pub last_error: Option<String>,
    /// Exponentially weighted average latency of successful calls.
    pub avg_latency_ms: Option<f64>,
    /// Total calls attempted (excluding cache hits).
    pub total_calls: u64,
    /// Total failures.
    pub total_failures: u64,
}

#[derive(Debug)]
struct HealthState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
    half_open_probe_in_flight: bool,
    last_success: Option<OffsetDateTime>,
    last_failure: Option<OffsetDateTime>,
    last_error: Option<String>,
    ewma_latency_ms: Option<f64>,
    total_calls: u64,
    total_failures: u64,
}

impl HealthState {
    const fn new() -> Self {
        Self {
            consecutive_failures: 0,
            open_until: None,
            half_open_probe_in_flight: false,
            last_success: None,
            last_failure: None,
            last_error: None,
            ewma_latency_ms: None,
            total_calls: 0,
            total_failures: 0,
        }
    }

    fn snapshot(&self) -> ProviderHealth {
        ProviderHealth {
            circuit: self.circuit(),
            consecutive_failures: self.consecutive_failures,
            last_success: self.last_success,
            last_failure: self.last_failure,
            last_error: self.last_error.clone(),
            avg_latency_ms: self.ewma_latency_ms,
            total_calls: self.total_calls,
            total_failures: self.total_failures,
        }
    }

    fn circuit(&self) -> CircuitState {
        match self.open_until {
            Some(until) if Instant::now() < until => CircuitState::Open {
                retry_in_secs: until.saturating_duration_since(Instant::now()).as_secs(),
            },
            Some(_) => CircuitState::HalfOpen,
            None => CircuitState::Closed,
        }
    }
}

/// Registry tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryConfig {
    /// Consecutive transient failures before the circuit opens.
    pub failure_threshold: u32,
    /// First cool-down; doubles per additional failure.
    pub base_cooldown: Duration,
    /// Longest cool-down.
    pub max_cooldown: Duration,
    /// Cool-down after an authentication failure (until credentials change).
    pub auth_cooldown: Duration,
    /// Per-call timeout.
    pub call_timeout: Duration,
    /// Default TTL for search results.
    pub search_ttl: Duration,
    /// Default TTL for lookups.
    pub lookup_ttl: Duration,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            base_cooldown: Duration::from_secs(30),
            max_cooldown: Duration::from_secs(600),
            auth_cooldown: Duration::from_secs(3600),
            call_timeout: Duration::from_secs(6),
            search_ttl: Duration::from_secs(900),
            lookup_ttl: Duration::from_secs(86_400),
        }
    }
}

/// A provider plus its runtime wrappers.
#[derive(Clone)]
pub struct ManagedProvider {
    provider: Arc<dyn DiscoveryProvider>,
    info: ProviderInfo,
    enabled: bool,
    disabled_reason: Option<String>,
    throttle: Throttle,
    health: Arc<Mutex<HealthState>>,
}

impl std::fmt::Debug for ManagedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedProvider")
            .field("id", &self.info.id)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

/// Status exposed by the API and CLI.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProviderStatus {
    /// Identifier.
    pub id: ProviderId,
    /// Name.
    pub name: &'static str,
    /// Whether searches use this provider.
    pub enabled: bool,
    /// Why it is disabled, when it is.
    pub disabled_reason: Option<String>,
    /// Whether credentials are required.
    pub requires_credentials: bool,
    /// Attribution text.
    pub attribution: Option<&'static str>,
    /// Documentation link.
    pub docs_url: &'static str,
    /// Capabilities.
    pub capabilities: crate::provider::Capabilities,
    /// Health.
    pub health: ProviderHealth,
    /// Trust weight.
    pub trust: f32,
}

/// Result of one managed call.
#[derive(Debug)]
pub struct ManagedCall<T> {
    /// The provider.
    pub provider: ProviderId,
    /// The result.
    pub result: Result<T, ProviderError>,
    /// Whether it came from the cache.
    pub from_cache: bool,
    /// Whether the circuit breaker skipped the call.
    pub skipped_by_circuit: bool,
    /// Wall-clock time.
    pub latency: Duration,
}

/// Holds every provider and the shared cache.
#[derive(Clone)]
pub struct ProviderRegistry {
    providers: Vec<ManagedProvider>,
    cache: DiscoveryCache,
    config: RegistryConfig,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field("providers", &self.providers)
            .finish_non_exhaustive()
    }
}

impl ProviderRegistry {
    /// Creates an empty registry.
    pub fn new(config: RegistryConfig, cache: DiscoveryCache) -> Self {
        Self {
            providers: Vec::new(),
            cache,
            config,
        }
    }

    /// Registers a provider. `disabled_reason` marks it disabled (e.g. missing credentials).
    pub fn register(
        &mut self,
        provider: Arc<dyn DiscoveryProvider>,
        disabled_reason: Option<String>,
    ) {
        let info = provider.info();
        let throttle = Throttle::new(info.throttle.clone());
        self.providers.push(ManagedProvider {
            provider,
            enabled: disabled_reason.is_none(),
            disabled_reason,
            throttle,
            health: Arc::new(Mutex::new(HealthState::new())),
            info,
        });
    }

    /// The cache.
    pub const fn cache(&self) -> &DiscoveryCache {
        &self.cache
    }

    /// The configuration.
    pub const fn config(&self) -> &RegistryConfig {
        &self.config
    }

    /// Ids of enabled providers, in registration order.
    pub fn enabled(&self) -> Vec<ProviderId> {
        self.providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.info.id)
            .collect()
    }

    /// Ids of all registered providers.
    pub fn all(&self) -> Vec<ProviderId> {
        self.providers.iter().map(|p| p.info.id).collect()
    }

    /// Info of a provider.
    pub fn info(&self, id: ProviderId) -> Option<&ProviderInfo> {
        self.providers
            .iter()
            .find(|p| p.info.id == id)
            .map(|p| &p.info)
    }

    /// Status of every provider.
    pub fn status(&self) -> Vec<ProviderStatus> {
        self.providers
            .iter()
            .map(|p| ProviderStatus {
                id: p.info.id,
                name: p.info.name,
                enabled: p.enabled,
                disabled_reason: p.disabled_reason.clone(),
                requires_credentials: p.info.requires_credentials,
                attribution: p.info.attribution,
                docs_url: p.info.docs_url,
                capabilities: p.info.capabilities,
                health: p
                    .health
                    .lock()
                    .map_or_else(|_| HealthState::new().snapshot(), |h| h.snapshot()),
                trust: p.info.trust,
            })
            .collect()
    }

    fn managed(&self, id: ProviderId) -> Option<&ManagedProvider> {
        self.providers.iter().find(|p| p.info.id == id)
    }

    /// Runs a search on one provider through cache, circuit breaker, throttle and timeout.
    pub async fn search(
        &self,
        id: ProviderId,
        query: &NormalizedQuery,
        ctx: &ProviderContext,
        use_cache: bool,
    ) -> ManagedCall<Vec<PodcastCandidate>> {
        let started = Instant::now();
        let Some(p) = self.managed(id) else {
            return ManagedCall {
                provider: id,
                result: Err(ProviderError::Unsupported("unknown provider".into())),
                from_cache: false,
                skipped_by_circuit: false,
                latency: Duration::ZERO,
            };
        };
        let cache_key = format!(
            "{}|{}|{}",
            query.ascii,
            ctx.limit,
            ctx.country.as_deref().unwrap_or("")
        );
        if use_cache
            && let Some(CachedValue::Search(v)) =
                self.cache.get(id, CacheKind::Search, &cache_key).await
        {
            tracing::debug!(provider = %id, query = %redact::urls(&query.raw), "provider search served from cache");
            return ManagedCall {
                provider: id,
                result: Ok(v),
                from_cache: true,
                skipped_by_circuit: false,
                latency: started.elapsed(),
            };
        }
        let call = self
            .guarded(
                p,
                ctx,
                |prov, ctx| async move { prov.search(query, &ctx).await },
            )
            .await;
        let result = match call {
            Ok(resp) => {
                let ttl = effective_ttl(self.config.search_ttl, resp.cache_max_age);
                self.cache
                    .put(
                        id,
                        CacheKind::Search,
                        &cache_key,
                        CachedValue::Search(resp.value.clone()),
                        ttl,
                    )
                    .await;
                Ok(resp.value)
            }
            Err(GuardError::Skipped) => {
                return ManagedCall {
                    provider: id,
                    result: Err(ProviderError::Unavailable("circuit open".into())),
                    from_cache: false,
                    skipped_by_circuit: true,
                    latency: started.elapsed(),
                };
            }
            Err(GuardError::Provider(e)) => Err(e),
        };
        ManagedCall {
            provider: id,
            result,
            from_cache: false,
            skipped_by_circuit: false,
            latency: started.elapsed(),
        }
    }

    /// Runs a lookup on one provider through the same guards.
    pub async fn lookup(
        &self,
        id: ProviderId,
        reference: &ProviderRef,
        ctx: &ProviderContext,
        use_cache: bool,
    ) -> ManagedCall<Option<PodcastCandidate>> {
        let started = Instant::now();
        let Some(p) = self.managed(id) else {
            return ManagedCall {
                provider: id,
                result: Err(ProviderError::Unsupported("unknown provider".into())),
                from_cache: false,
                skipped_by_circuit: false,
                latency: Duration::ZERO,
            };
        };
        let cache_key = reference.cache_key();
        if use_cache
            && let Some(CachedValue::Lookup(v)) =
                self.cache.get(id, CacheKind::Lookup, &cache_key).await
        {
            return ManagedCall {
                provider: id,
                result: Ok(*v),
                from_cache: true,
                skipped_by_circuit: false,
                latency: started.elapsed(),
            };
        }
        let call = self
            .guarded(p, ctx, |prov, ctx| async move {
                prov.lookup(reference, &ctx).await
            })
            .await;
        let result = match call {
            Ok(resp) => {
                let ttl = effective_ttl(self.config.lookup_ttl, resp.cache_max_age);
                self.cache
                    .put(
                        id,
                        CacheKind::Lookup,
                        &cache_key,
                        CachedValue::Lookup(Box::new(resp.value.clone())),
                        ttl,
                    )
                    .await;
                Ok(resp.value)
            }
            Err(GuardError::Skipped) => {
                return ManagedCall {
                    provider: id,
                    result: Err(ProviderError::Unavailable("circuit open".into())),
                    from_cache: false,
                    skipped_by_circuit: true,
                    latency: started.elapsed(),
                };
            }
            Err(GuardError::Provider(e)) => Err(e),
        };
        ManagedCall {
            provider: id,
            result,
            from_cache: false,
            skipped_by_circuit: false,
            latency: started.elapsed(),
        }
    }

    /// Circuit breaker + throttle + timeout around a provider call, with health bookkeeping.
    async fn guarded<T, F, Fut>(
        &self,
        p: &ManagedProvider,
        ctx: &ProviderContext,
        call: F,
    ) -> Result<ProviderResponse<T>, GuardError>
    where
        F: FnOnce(Arc<dyn DiscoveryProvider>, ProviderContext) -> Fut,
        Fut: std::future::Future<Output = Result<ProviderResponse<T>, ProviderError>>,
    {
        let id = p.info.id;
        // Circuit check.
        {
            let mut h = p.health.lock().map_err(|_| GuardError::Skipped)?;
            match h.circuit() {
                CircuitState::Open { .. } => {
                    tracing::debug!(provider = %id, "provider skipped: circuit open");
                    return Err(GuardError::Skipped);
                }
                CircuitState::HalfOpen => {
                    if h.half_open_probe_in_flight {
                        return Err(GuardError::Skipped);
                    }
                    h.half_open_probe_in_flight = true;
                }
                CircuitState::Closed => {}
            }
            h.total_calls += 1;
        }
        let Some(_permit) = p.throttle.acquire(&ctx.cancel).await else {
            self.record(p, Err(&ProviderError::Cancelled), Duration::ZERO);
            return Err(GuardError::Provider(ProviderError::Cancelled));
        };
        let timeout = ctx.remaining().map_or(self.config.call_timeout, |r| {
            r.min(self.config.call_timeout)
        });
        let started = Instant::now();
        tracing::debug!(provider = %id, timeout_ms = timeout.as_millis(), "provider request started");
        let outcome = tokio::select! {
            () = ctx.cancel.cancelled() => Err(ProviderError::Cancelled),
            r = tokio::time::timeout(timeout, call(Arc::clone(&p.provider), ctx.clone())) => match r {
                Ok(inner) => inner,
                Err(_) => Err(ProviderError::Timeout),
            },
        };
        let latency = started.elapsed();
        match &outcome {
            Ok(_) => {
                tracing::debug!(provider = %id, latency_ms = latency.as_millis(), "provider request completed");
            }
            Err(e) => {
                tracing::warn!(provider = %id, latency_ms = latency.as_millis(), error = %redact::urls(&e.to_string()), kind = e.kind(), "provider request failed");
            }
        }
        self.record(p, outcome.as_ref().map(|_| ()), latency);
        outcome.map_err(GuardError::Provider)
    }

    fn record(&self, p: &ManagedProvider, outcome: Result<(), &ProviderError>, latency: Duration) {
        let Ok(mut h) = p.health.lock() else { return };
        h.half_open_probe_in_flight = false;
        let now = OffsetDateTime::now_utc();
        match outcome {
            Ok(()) => {
                h.consecutive_failures = 0;
                h.open_until = None;
                h.last_success = Some(now);
                let ms = latency.as_secs_f64() * 1000.0;
                h.ewma_latency_ms =
                    Some(h.ewma_latency_ms.map_or(ms, |prev| prev * 0.8 + ms * 0.2));
            }
            Err(ProviderError::Cancelled) => {}
            Err(e) => {
                h.total_failures += 1;
                h.last_failure = Some(now);
                h.last_error = Some(e.kind().to_owned());
                if e.is_auth() {
                    h.consecutive_failures = h.consecutive_failures.saturating_add(1);
                    h.open_until = Some(Instant::now() + self.config.auth_cooldown);
                } else if e.is_transient() {
                    h.consecutive_failures = h.consecutive_failures.saturating_add(1);
                    if h.consecutive_failures >= self.config.failure_threshold {
                        let extra = h.consecutive_failures - self.config.failure_threshold;
                        let cooldown = self
                            .config
                            .base_cooldown
                            .saturating_mul(1u32 << extra.min(8))
                            .min(self.config.max_cooldown);
                        let cooldown = match e {
                            ProviderError::RateLimited {
                                retry_after: Some(ra),
                            } => cooldown.max(*ra).min(self.config.max_cooldown),
                            _ => cooldown,
                        };
                        h.open_until = Some(Instant::now() + cooldown);
                        tracing::warn!(provider = %p.info.id, failures = h.consecutive_failures, cooldown_secs = cooldown.as_secs(), "provider circuit opened");
                    }
                }
            }
        }
    }
}

enum GuardError {
    Skipped,
    Provider(ProviderError),
}

fn effective_ttl(configured: Duration, provider_max_age: Option<Duration>) -> Duration {
    match provider_max_age {
        Some(max_age) => configured.min(max_age),
        None => configured,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicU32, Ordering};

    use async_trait::async_trait;
    use uguisu_http::ThrottleConfig;

    use super::*;
    use crate::candidate::ProviderIdentity;
    use crate::provider::Capabilities;

    struct Flaky {
        fail_first: u32,
        calls: AtomicU32,
        delay: Duration,
    }

    #[async_trait]
    impl DiscoveryProvider for Flaky {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                id: ProviderId::new("flaky"),
                name: "Flaky",
                attribution: None,
                docs_url: "",
                capabilities: Capabilities {
                    search: true,
                    ..Capabilities::default()
                },
                requires_credentials: false,
                throttle: ThrottleConfig::concurrency_only(2),
                trust: 0.5,
            }
        }

        async fn search(
            &self,
            query: &NormalizedQuery,
            _ctx: &ProviderContext,
        ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            if n < self.fail_first {
                return Err(ProviderError::Unavailable("boom".into()));
            }
            let identity = ProviderIdentity {
                provider: ProviderId::new("flaky"),
                provider_ref: "1".into(),
                confidence: 1.0,
                url: None,
                fetched_at: OffsetDateTime::UNIX_EPOCH,
            };
            Ok(ProviderResponse {
                value: vec![PodcastCandidate::new(query.raw.clone(), identity)],
                cache_max_age: Some(Duration::from_secs(5)),
            })
        }
    }

    fn registry(fail_first: u32, delay: Duration) -> ProviderRegistry {
        let mut r = ProviderRegistry::new(
            RegistryConfig {
                failure_threshold: 2,
                base_cooldown: Duration::from_millis(200),
                call_timeout: Duration::from_millis(100),
                ..RegistryConfig::default()
            },
            DiscoveryCache::new(100),
        );
        r.register(
            Arc::new(Flaky {
                fail_first,
                calls: AtomicU32::new(0),
                delay,
            }),
            None,
        );
        r
    }

    #[tokio::test]
    async fn circuit_opens_after_threshold_and_recovers() {
        let r = registry(2, Duration::ZERO);
        let id = ProviderId::new("flaky");
        let q = NormalizedQuery::parse("x");
        let ctx = ProviderContext::default();
        assert!(r.search(id, &q, &ctx, false).await.result.is_err());
        assert!(r.search(id, &q, &ctx, false).await.result.is_err());
        let skipped = r.search(id, &q, &ctx, false).await;
        assert!(
            skipped.skipped_by_circuit,
            "third call is skipped by the open circuit"
        );
        assert!(matches!(
            r.status()[0].health.circuit,
            CircuitState::Open { .. }
        ));
        tokio::time::sleep(Duration::from_millis(250)).await;
        let probe = r.search(id, &q, &ctx, false).await;
        assert!(
            probe.result.is_ok(),
            "half-open probe succeeds and closes the circuit"
        );
        assert_eq!(r.status()[0].health.circuit, CircuitState::Closed);
        assert_eq!(r.status()[0].health.consecutive_failures, 0);
        assert!(r.status()[0].health.avg_latency_ms.is_some());
    }

    #[tokio::test]
    async fn cache_serves_repeat_queries_with_provider_ttl() {
        let r = registry(0, Duration::ZERO);
        let id = ProviderId::new("flaky");
        let q = NormalizedQuery::parse("Darknet Diaries");
        let ctx = ProviderContext::default();
        let first = r.search(id, &q, &ctx, true).await;
        assert!(!first.from_cache && first.result.is_ok());
        let second = r
            .search(
                id,
                &NormalizedQuery::parse("darknet   diaries!"),
                &ctx,
                true,
            )
            .await;
        assert!(second.from_cache, "normalized key hits the cache");
        let bypass = r.search(id, &q, &ctx, false).await;
        assert!(!bypass.from_cache);
        assert_eq!(r.cache().stats().hits, 1);
    }

    #[tokio::test]
    async fn timeout_counts_as_transient_failure() {
        let r = registry(0, Duration::from_millis(300));
        let id = ProviderId::new("flaky");
        let call = r
            .search(
                id,
                &NormalizedQuery::parse("x"),
                &ProviderContext::default(),
                false,
            )
            .await;
        assert!(matches!(call.result, Err(ProviderError::Timeout)));
        assert_eq!(r.status()[0].health.consecutive_failures, 1);
        assert_eq!(r.status()[0].health.last_error.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn disabled_and_unknown_providers() {
        let mut r = ProviderRegistry::new(RegistryConfig::default(), DiscoveryCache::new(1));
        r.register(
            Arc::new(Flaky {
                fail_first: 0,
                calls: AtomicU32::new(0),
                delay: Duration::ZERO,
            }),
            Some("no key".into()),
        );
        assert!(r.enabled().is_empty());
        assert_eq!(r.all().len(), 1);
        assert_eq!(r.status()[0].disabled_reason.as_deref(), Some("no key"));
        let call = r
            .search(
                ProviderId::new("nope"),
                &NormalizedQuery::parse("x"),
                &ProviderContext::default(),
                false,
            )
            .await;
        assert!(matches!(call.result, Err(ProviderError::Unsupported(_))));
    }

    #[test]
    fn effective_ttl_honours_provider_max_age() {
        assert_eq!(
            effective_ttl(Duration::from_secs(900), Some(Duration::from_secs(60))),
            Duration::from_secs(60)
        );
        assert_eq!(
            effective_ttl(Duration::from_secs(900), Some(Duration::from_secs(9000))),
            Duration::from_secs(900)
        );
        assert_eq!(
            effective_ttl(Duration::from_secs(900), None),
            Duration::from_secs(900)
        );
    }
}
