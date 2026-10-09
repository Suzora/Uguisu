//! The search engine: parallel provider fan-out, deadlines, streaming
//! snapshots and the outcome taxonomy.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uguisu_core::config::SearchConfig;
use uguisu_core::provider::ProviderId;
use uguisu_core::redact;

use crate::candidate::PodcastCandidate;
use crate::dedup::{self, DedupThresholds};
use crate::error::SearchOutcome;
use crate::merge;
use crate::provider::{ProviderContext, ProviderError};
use crate::query::NormalizedQuery;
use crate::rank::{self, RankContext, RankedCandidate, RankingWeights};
use crate::registry::{ManagedCall, ProviderRegistry};

/// Version of the JSON shape produced by [`SearchResponse`].
pub const RESPONSE_SCHEMA: u32 = 1;

/// A search request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchRequest {
    /// Free text (URLs are not searched; the caller should resolve them).
    pub query: String,
    /// Restrict to these providers (default: all enabled).
    #[serde(default)]
    pub providers: Option<Vec<ProviderId>>,
    /// Maximum results (default from config, capped).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Storefront / country hint.
    #[serde(default)]
    pub country: Option<String>,
    /// Bypass the cache.
    #[serde(default)]
    pub no_cache: bool,
}

/// How one provider fared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCallStatus {
    /// Answered.
    Ok,
    /// Not asked (disabled, circuit open, or not requested).
    Skipped,
    /// Answered with an error.
    Failed,
    /// Did not answer before the hard deadline.
    TimedOut,
    /// Cancelled by the caller.
    Cancelled,
}

/// Per-provider outcome of a search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderOutcome {
    /// Provider.
    pub provider: ProviderId,
    /// Status.
    pub status: ProviderCallStatus,
    /// Candidates contributed before dedup.
    pub candidates: usize,
    /// Wall-clock latency.
    pub latency_ms: u64,
    /// Served from cache.
    pub from_cache: bool,
    /// Error message when failed/skipped.
    pub error: Option<String>,
    /// Stable error kind.
    pub error_kind: Option<String>,
}

/// A looser query asked because every provider answered the query as typed
/// with nothing (ADR 0005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RelaxedQuery {
    /// The query sent to the providers.
    pub query: String,
    /// How each provider answered it.
    pub providers: Vec<ProviderOutcome>,
}

/// Timing information.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchTiming {
    /// Total time until this snapshot.
    pub total_ms: u64,
    /// Time until the first non-empty snapshot.
    pub first_results_ms: Option<u64>,
}

/// The result of a search (also each streamed snapshot).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchResponse {
    /// JSON shape version.
    pub schema: u32,
    /// Outcome.
    pub outcome: SearchOutcome,
    /// The normalized query.
    pub query: NormalizedQuery,
    /// Ranked results.
    pub results: Vec<RankedCandidate>,
    /// Provider outcomes for the query as typed.
    pub providers: Vec<ProviderOutcome>,
    /// Looser queries asked because the query as typed found nothing; when
    /// not empty, every result came from them, ranked against the query as typed.
    #[serde(default)]
    pub relaxed: Vec<RelaxedQuery>,
    /// Attribution strings for providers that contributed results.
    pub attribution: Vec<String>,
    /// Timing.
    pub timing: SearchTiming,
    /// Whether every provider has answered or timed out.
    pub complete: bool,
}

/// The engine.
#[derive(Debug, Clone)]
pub struct SearchEngine {
    registry: Arc<ProviderRegistry>,
    config: SearchConfig,
    weights: RankingWeights,
    thresholds: DedupThresholds,
}

struct Arrival {
    /// Index into the asked queries: 0 is the query as typed.
    slot: usize,
    call: ManagedCall<Vec<PodcastCandidate>>,
}

/// One query sent to the providers and how each answered it.
struct Asked {
    query: NormalizedQuery,
    outcomes: Vec<ProviderOutcome>,
}

impl SearchEngine {
    /// Creates an engine over a registry.
    pub fn new(registry: Arc<ProviderRegistry>, config: SearchConfig) -> Self {
        Self {
            registry,
            config,
            weights: RankingWeights::default(),
            thresholds: DedupThresholds::default(),
        }
    }

    /// Overrides the ranking weights.
    #[must_use]
    pub fn with_weights(mut self, weights: RankingWeights) -> Self {
        self.weights = weights;
        self
    }

    /// The registry.
    pub const fn registry(&self) -> &Arc<ProviderRegistry> {
        &self.registry
    }

    /// The configuration.
    pub const fn config(&self) -> &SearchConfig {
        &self.config
    }

    fn trust_map(&self) -> HashMap<ProviderId, f32> {
        self.registry
            .all()
            .into_iter()
            .filter_map(|id| self.registry.info(id).map(|i| (id, i.trust)))
            .collect()
    }

    /// Runs a search to completion.
    pub async fn search(
        &self,
        request: &SearchRequest,
        cancel: CancellationToken,
    ) -> SearchResponse {
        let mut rx = self.search_stream(request.clone(), cancel);
        let mut last = None;
        while let Some(snapshot) = rx.recv().await {
            last = Some(snapshot);
        }
        last.unwrap_or_else(|| Self::empty_response(request, SearchOutcome::NoResults))
    }

    fn empty_response(request: &SearchRequest, outcome: SearchOutcome) -> SearchResponse {
        SearchResponse {
            schema: RESPONSE_SCHEMA,
            outcome,
            query: NormalizedQuery::parse(&request.query),
            results: Vec::new(),
            providers: Vec::new(),
            relaxed: Vec::new(),
            attribution: Vec::new(),
            timing: SearchTiming::default(),
            complete: true,
        }
    }

    /// Streams snapshots: one whenever a provider answers (the first no later
    /// than the soft deadline), the last marked `complete` once every
    /// provider answered or the hard deadline passed.
    pub fn search_stream(
        &self,
        request: SearchRequest,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<SearchResponse> {
        let (tx, rx) = mpsc::channel(8);
        let engine = self.clone();
        tokio::spawn(async move { engine.run(request, cancel, tx).await });
        rx
    }

    #[allow(clippy::too_many_lines)] // the aggregation loop reads best as one piece
    async fn run(
        self,
        request: SearchRequest,
        cancel: CancellationToken,
        tx: mpsc::Sender<SearchResponse>,
    ) {
        let started = Instant::now();
        let query = NormalizedQuery::parse(&request.query);
        let limit = request
            .limit
            .unwrap_or(self.config.default_limit)
            .clamp(1, self.config.max_limit);
        // A URL query, bare-domain forms included, is never logged: it may
        // be a private feed whose credential redact::urls cannot find.
        if query.is_empty() || query.is_url() {
            let _ = tx
                .send(Self::empty_response(&request, SearchOutcome::NoResults))
                .await;
            return;
        }
        tracing::info!(query = %redact::urls(&query.raw), limit, "search started");

        let enabled = self.registry.enabled();
        let mut outcomes: Vec<ProviderOutcome> = Vec::new();
        let mut targets: Vec<ProviderId> = Vec::new();
        match &request.providers {
            Some(wanted) => {
                for id in wanted {
                    if enabled.contains(id) {
                        targets.push(*id);
                    } else {
                        let reason = self
                            .registry
                            .status()
                            .into_iter()
                            .find(|s| s.id == *id)
                            .and_then(|s| s.disabled_reason)
                            .unwrap_or_else(|| "unknown or disabled provider".to_owned());
                        outcomes.push(skipped(*id, reason));
                    }
                }
            }
            None => targets = enabled,
        }
        if targets.is_empty() {
            let mut resp = Self::empty_response(&request, SearchOutcome::NoProvidersEnabled);
            resp.providers = outcomes;
            let _ = tx.send(resp).await;
            return;
        }

        let child = cancel.child_token();
        let hard_deadline = Instant::now() + self.config.hard_deadline;
        let ctx = ProviderContext {
            cancel: child.clone(),
            deadline: Some(hard_deadline),
            limit,
            country: request.country.clone(),
        };
        let use_cache = !request.no_cache;
        let mut asked = vec![Asked {
            query: query.clone(),
            outcomes,
        }];
        let mut arrivals = self.fan_out(&asked, 0, &targets, &ctx, use_cache);
        let mut pending: Vec<(usize, ProviderId)> = targets.iter().map(|id| (0, *id)).collect();

        let trust = self.trust_map();
        let rank_ctx = RankContext {
            trust,
            weights: self.weights.clone(),
            now: None,
        };
        let mut collected: Vec<PodcastCandidate> = Vec::new();
        let mut first_results_ms = None;
        let mut emitted_any = false;
        let soft = tokio::time::sleep(self.config.soft_deadline);
        tokio::pin!(soft);
        let hard = tokio::time::sleep_until(tokio::time::Instant::from_std(hard_deadline));
        tokio::pin!(hard);
        let mut soft_fired = false;

        loop {
            let arrival = tokio::select! {
                () = cancel.cancelled() => {
                    for (slot, id) in pending.drain(..) {
                        asked[slot].outcomes.push(ProviderOutcome { provider: id, status: ProviderCallStatus::Cancelled, candidates: 0, latency_ms: millis(started.elapsed()), from_cache: false, error: Some("cancelled".into()), error_kind: Some("cancelled".into()) });
                    }
                    child.cancel();
                    break;
                }
                () = &mut hard => {
                    for (slot, id) in pending.drain(..) {
                        tracing::warn!(provider = %id, "provider timed out at the hard deadline");
                        asked[slot].outcomes.push(ProviderOutcome { provider: id, status: ProviderCallStatus::TimedOut, candidates: 0, latency_ms: millis(started.elapsed()), from_cache: false, error: Some("no answer before the hard deadline".into()), error_kind: Some("timeout".into()) });
                    }
                    child.cancel();
                    break;
                }
                () = &mut soft, if !soft_fired => {
                    soft_fired = true;
                    if !pending.is_empty() {
                        let snapshot = self.snapshot(&collected, &asked, &rank_ctx, limit, started, &mut first_results_ms, false);
                        emitted_any = true;
                        if tx.send(snapshot).await.is_err() {
                            child.cancel();
                            return;
                        }
                    }
                    continue;
                }
                a = arrivals.recv() => match a {
                    Some(a) => a,
                    None => break,
                },
            };

            let Arrival { slot, call } = arrival;
            pending.retain(|p| *p != (slot, call.provider));
            let latency_ms = millis(call.latency);
            let outcomes = &mut asked[slot].outcomes;
            match call.result {
                Ok(cands) => {
                    tracing::debug!(provider = %call.provider, count = cands.len(), from_cache = call.from_cache, "provider results received");
                    outcomes.push(ProviderOutcome {
                        provider: call.provider,
                        status: ProviderCallStatus::Ok,
                        candidates: cands.len(),
                        latency_ms,
                        from_cache: call.from_cache,
                        error: None,
                        error_kind: None,
                    });
                    collected.extend(cands);
                }
                Err(e) => {
                    let status = match &e {
                        _ if call.skipped_by_circuit => ProviderCallStatus::Skipped,
                        ProviderError::Timeout => ProviderCallStatus::TimedOut,
                        ProviderError::Cancelled => ProviderCallStatus::Cancelled,
                        _ => ProviderCallStatus::Failed,
                    };
                    outcomes.push(ProviderOutcome {
                        provider: call.provider,
                        status,
                        candidates: 0,
                        latency_ms,
                        from_cache: false,
                        error: Some(e.to_string()),
                        error_kind: Some(e.kind().to_owned()),
                    });
                }
            }
            if !pending.is_empty() && (soft_fired || !collected.is_empty()) {
                let snapshot = self.snapshot(
                    &collected,
                    &asked,
                    &rank_ctx,
                    limit,
                    started,
                    &mut first_results_ms,
                    false,
                );
                emitted_any = true;
                if tx.send(snapshot).await.is_err() {
                    child.cancel();
                    return;
                }
            }
            if pending.is_empty() {
                // Only a query every asked provider answered, with nothing,
                // is relaxed: a failure or a timeout is not evidence of a typo.
                let relax = asked.len() == 1
                    && collected.is_empty()
                    && asked[0]
                        .outcomes
                        .iter()
                        .filter(|o| o.error_kind.as_deref() != Some(NOT_ASKED))
                        .all(|o| o.status == ProviderCallStatus::Ok);
                let variants = if relax { query.relaxed() } else { Vec::new() };
                if variants.is_empty() {
                    break;
                }
                let raws: Vec<String> = variants
                    .iter()
                    .map(|v| redact::urls(&v.raw).to_string())
                    .collect();
                tracing::info!(query = %redact::urls(&query.raw), relaxed = ?raws, "search relaxed");
                asked.extend(variants.into_iter().map(|query| Asked {
                    query,
                    outcomes: Vec::new(),
                }));
                arrivals = self.fan_out(&asked, 1, &targets, &ctx, use_cache);
                pending = (1..asked.len())
                    .flat_map(|slot| targets.iter().map(move |id| (slot, *id)))
                    .collect();
            }
        }

        let final_snapshot = self.snapshot(
            &collected,
            &asked,
            &rank_ctx,
            limit,
            started,
            &mut first_results_ms,
            true,
        );
        tracing::info!(query = %redact::urls(&query.raw), outcome = final_snapshot.outcome.as_str(), results = final_snapshot.results.len(), total_ms = final_snapshot.timing.total_ms, "search completed");
        let _ = emitted_any;
        let _ = tx.send(final_snapshot).await;
    }

    /// Asks every target for each query in `asked[first..]`, concurrently.
    fn fan_out(
        &self,
        asked: &[Asked],
        first: usize,
        targets: &[ProviderId],
        ctx: &ProviderContext,
        use_cache: bool,
    ) -> mpsc::Receiver<Arrival> {
        let calls = (asked.len() - first) * targets.len();
        let (tx, rx) = mpsc::channel::<Arrival>(calls.max(1));
        for (slot, a) in asked.iter().enumerate().skip(first) {
            for id in targets {
                let registry = Arc::clone(&self.registry);
                let query = a.query.clone();
                let ctx = ctx.clone();
                let tx = tx.clone();
                let id = *id;
                tokio::spawn(async move {
                    let call = registry.search(id, &query, &ctx, use_cache).await;
                    let _ = tx.send(Arrival { slot, call }).await;
                });
            }
        }
        rx
    }

    fn snapshot(
        &self,
        collected: &[PodcastCandidate],
        asked: &[Asked],
        rank_ctx: &RankContext,
        limit: usize,
        started: Instant,
        first_results_ms: &mut Option<u64>,
        complete: bool,
    ) -> SearchResponse {
        let groups = dedup::group(collected.to_vec(), &self.thresholds);
        let merged: Vec<_> = groups
            .into_iter()
            .map(|g| (merge::merge(g.members, &rank_ctx.trust), g.ambiguities))
            .collect();
        let query = &asked[0].query;
        let outcomes = &asked[0].outcomes;
        let mut results = rank::rank(query, merged, rank_ctx);
        results.truncate(limit);
        let total_ms = millis(started.elapsed());
        if !results.is_empty() && first_results_ms.is_none() {
            *first_results_ms = Some(total_ms);
        }
        // A provider behind an open breaker was asked for and is down; only
        // one that was never asked for leaves nothing enabled.
        let asked_count = outcomes
            .iter()
            .filter(|o| o.error_kind.as_deref() != Some(NOT_ASKED))
            .count();
        let ok = outcomes
            .iter()
            .filter(|o| o.status == ProviderCallStatus::Ok)
            .count();
        let outcome = if !results.is_empty() {
            SearchOutcome::Results
        } else if complete && asked_count > 0 && ok == 0 {
            SearchOutcome::AllProvidersFailed
        } else if complete && asked_count == 0 {
            SearchOutcome::NoProvidersEnabled
        } else {
            SearchOutcome::NoResults
        };
        let mut attribution: Vec<String> = asked
            .iter()
            .flat_map(|a| &a.outcomes)
            .filter(|o| o.status == ProviderCallStatus::Ok && o.candidates > 0)
            .filter_map(|o| {
                self.registry
                    .info(o.provider)
                    .and_then(|i| i.attribution)
                    .map(str::to_owned)
            })
            .collect();
        attribution.sort();
        attribution.dedup();
        let sorted = |outcomes: &[ProviderOutcome]| {
            let mut v = outcomes.to_vec();
            v.sort_by_key(|o| o.provider);
            v
        };
        let relaxed = asked[1..]
            .iter()
            .map(|a| RelaxedQuery {
                query: a.query.raw.clone(),
                providers: sorted(&a.outcomes),
            })
            .collect();
        SearchResponse {
            schema: RESPONSE_SCHEMA,
            outcome,
            query: query.clone(),
            results,
            providers: sorted(outcomes),
            relaxed,
            attribution,
            timing: SearchTiming {
                total_ms,
                first_results_ms: *first_results_ms,
            },
            complete,
        }
    }
}

/// Milliseconds as u64 (saturating).
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The `error_kind` of a provider the search did not ask.
const NOT_ASKED: &str = "skipped";

fn skipped(id: ProviderId, reason: String) -> ProviderOutcome {
    ProviderOutcome {
        provider: id,
        status: ProviderCallStatus::Skipped,
        candidates: 0,
        latency_ms: 0,
        from_cache: false,
        error: Some(reason),
        error_kind: Some(NOT_ASKED.into()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use async_trait::async_trait;
    use time::OffsetDateTime;
    use uguisu_http::ThrottleConfig;
    use url::Url;

    use super::*;
    use crate::cache::DiscoveryCache;
    use crate::candidate::ProviderIdentity;
    use crate::provider::{Capabilities, DiscoveryProvider, ProviderInfo, ProviderResponse};
    use crate::registry::RegistryConfig;

    struct Stub {
        id: ProviderId,
        titles: Vec<(&'static str, &'static str)>,
        delay: Duration,
        fail: Option<fn() -> ProviderError>,
        trust: f32,
        /// Only this query gets `titles`, after `delay`; any other is
        /// answered at once with nothing.
        answers_to: Option<&'static str>,
    }

    #[async_trait]
    impl DiscoveryProvider for Stub {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                id: self.id,
                name: "stub",
                attribution: Some("Data from stub"),
                docs_url: "",
                capabilities: Capabilities {
                    search: true,
                    ..Capabilities::default()
                },
                requires_credentials: false,
                throttle: ThrottleConfig::concurrency_only(4),
                trust: self.trust,
            }
        }

        async fn search(
            &self,
            q: &NormalizedQuery,
            _ctx: &ProviderContext,
        ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
            if self.answers_to.is_some_and(|a| a != q.raw) {
                return Ok(ProviderResponse::new(Vec::new()));
            }
            tokio::time::sleep(self.delay).await;
            if let Some(f) = self.fail {
                return Err(f());
            }
            let cands = self
                .titles
                .iter()
                .enumerate()
                .map(|(i, (title, feed))| {
                    let identity = ProviderIdentity {
                        provider: self.id,
                        provider_ref: format!("{}-{i}", self.id),
                        confidence: 1.0,
                        url: None,
                        fetched_at: OffsetDateTime::UNIX_EPOCH,
                    };
                    let mut c = PodcastCandidate::new(*title, identity);
                    c.feed_url = Some(Url::parse(feed).unwrap());
                    c.attribute_all_to(self.id);
                    c
                })
                .collect();
            Ok(ProviderResponse::new(cands))
        }
    }

    fn engine(stubs: Vec<Stub>, soft_ms: u64, hard_ms: u64) -> SearchEngine {
        let mut registry = ProviderRegistry::new(
            RegistryConfig {
                call_timeout: Duration::from_secs(5),
                ..RegistryConfig::default()
            },
            DiscoveryCache::new(100),
        );
        for s in stubs {
            registry.register(Arc::new(s), None);
        }
        let config = SearchConfig {
            soft_deadline: Duration::from_millis(soft_ms),
            hard_deadline: Duration::from_millis(hard_ms),
            provider_timeout: Duration::from_secs(5),
            default_limit: 25,
            max_limit: 100,
        };
        SearchEngine::new(Arc::new(registry), config)
    }

    fn apple() -> Stub {
        Stub {
            id: ProviderId::APPLE,
            titles: vec![
                (
                    "Darknet Diaries",
                    "https://feeds.megaphone.fm/darknetdiaries",
                ),
                ("Dark Net", "https://darknet.test/rss"),
            ],
            delay: Duration::ZERO,
            fail: None,
            trust: 0.8,
            answers_to: None,
        }
    }

    fn pi() -> Stub {
        Stub {
            id: ProviderId::PODCAST_INDEX,
            titles: vec![(
                "Darknet Diaries",
                "http://feeds.megaphone.fm/darknetdiaries/",
            )],
            delay: Duration::ZERO,
            fail: None,
            trust: 0.9,
            answers_to: None,
        }
    }

    fn req(q: &str) -> SearchRequest {
        SearchRequest {
            query: q.into(),
            ..SearchRequest::default()
        }
    }

    #[tokio::test]
    async fn merges_across_providers_and_reports_outcomes() {
        let e = engine(vec![apple(), pi()], 2000, 8000);
        let r = e
            .search(&req("darknet diaries"), CancellationToken::new())
            .await;
        assert_eq!(r.outcome, SearchOutcome::Results);
        assert!(r.complete);
        assert_eq!(
            r.results.len(),
            2,
            "Apple+PI entries for the same feed are merged"
        );
        assert_eq!(r.results[0].candidate.title, "Darknet Diaries");
        assert_eq!(r.results[0].candidate.providers().len(), 2);
        assert_eq!(
            r.results[0].candidate.primary_provider(),
            Some(ProviderId::PODCAST_INDEX)
        );
        assert_eq!(r.providers.len(), 2);
        assert!(
            r.providers
                .iter()
                .all(|p| p.status == ProviderCallStatus::Ok)
        );
        assert_eq!(r.attribution, vec!["Data from stub"]);
        assert!(r.timing.first_results_ms.is_some());
        assert_eq!(r.schema, RESPONSE_SCHEMA);
    }

    #[tokio::test]
    async fn one_failing_provider_is_survivable() {
        let mut bad = pi();
        bad.fail = Some(|| ProviderError::Unavailable("503".into()));
        let e = engine(vec![apple(), bad], 2000, 8000);
        let r = e.search(&req("darknet"), CancellationToken::new()).await;
        assert_eq!(r.outcome, SearchOutcome::Results);
        let failed = r
            .providers
            .iter()
            .find(|p| p.provider == ProviderId::PODCAST_INDEX)
            .unwrap();
        assert_eq!(failed.status, ProviderCallStatus::Failed);
        assert_eq!(failed.error_kind.as_deref(), Some("unavailable"));
    }

    #[tokio::test]
    async fn all_failing_none_enabled_and_empty_query() {
        let mut a = apple();
        a.fail = Some(|| ProviderError::Unavailable("down".into()));
        let mut b = pi();
        b.fail = Some(|| ProviderError::Timeout);
        let e = engine(vec![a, b], 2000, 8000);
        let r = e.search(&req("darknet"), CancellationToken::new()).await;
        assert_eq!(r.outcome, SearchOutcome::AllProvidersFailed);
        assert!(r.results.is_empty());

        let e = engine(vec![], 2000, 8000);
        assert_eq!(
            e.search(&req("darknet"), CancellationToken::new())
                .await
                .outcome,
            SearchOutcome::NoProvidersEnabled
        );

        let e = engine(vec![apple()], 2000, 8000);
        assert_eq!(
            e.search(&req("   "), CancellationToken::new())
                .await
                .outcome,
            SearchOutcome::NoResults
        );
        assert_eq!(
            e.search(&req("https://example.com/feed"), CancellationToken::new())
                .await
                .outcome,
            SearchOutcome::NoResults
        );
        let r = e
            .search(
                &SearchRequest {
                    query: "darknet".into(),
                    providers: Some(vec![ProviderId::GPODDER_NET]),
                    ..SearchRequest::default()
                },
                CancellationToken::new(),
            )
            .await;
        assert_eq!(r.outcome, SearchOutcome::NoProvidersEnabled);
        assert_eq!(r.providers[0].status, ProviderCallStatus::Skipped);
    }

    #[tokio::test]
    async fn open_breaker_is_a_failure() {
        let mut a = apple();
        a.fail = Some(|| ProviderError::Unavailable("down".into()));
        let e = engine(vec![a], 2000, 8000);
        let mut r = e.search(&req("darknet"), CancellationToken::new()).await;
        for _ in 0..RegistryConfig::default().failure_threshold {
            r = e.search(&req("darknet"), CancellationToken::new()).await;
        }
        assert_eq!(r.providers[0].status, ProviderCallStatus::Skipped, "{r:?}");
        assert_eq!(r.outcome, SearchOutcome::AllProvidersFailed);
    }

    #[tokio::test]
    async fn no_results_when_providers_answer_empty() {
        let empty = Stub {
            id: ProviderId::GPODDER_NET,
            titles: vec![],
            delay: Duration::ZERO,
            fail: None,
            trust: 0.6,
            answers_to: None,
        };
        let e = engine(vec![empty], 2000, 8000);
        let r = e.search(&req("nothing"), CancellationToken::new()).await;
        assert_eq!(r.outcome, SearchOutcome::NoResults);
        assert_eq!(r.providers[0].status, ProviderCallStatus::Ok);
    }

    #[tokio::test]
    async fn a_slow_provider_yields_partial_results() {
        let mut slow = pi();
        slow.delay = Duration::from_secs(3);
        let e = engine(vec![apple(), slow], 50, 300);
        let started = Instant::now();
        let r = e
            .search(&req("darknet diaries"), CancellationToken::new())
            .await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(r.outcome, SearchOutcome::Results);
        assert!(r.complete);
        let slow_outcome = r
            .providers
            .iter()
            .find(|p| p.provider == ProviderId::PODCAST_INDEX)
            .unwrap();
        assert_eq!(slow_outcome.status, ProviderCallStatus::TimedOut);
        assert_eq!(r.results[0].candidate.providers(), vec![ProviderId::APPLE]);
    }

    #[tokio::test]
    async fn streaming_emits_early_snapshot_then_completes() {
        let mut slow = pi();
        slow.delay = Duration::from_millis(400);
        let e = engine(vec![apple(), slow], 100, 5000);
        let mut rx = e.search_stream(req("darknet diaries"), CancellationToken::new());
        let first = rx.recv().await.unwrap();
        assert!(!first.complete);
        assert_eq!(first.outcome, SearchOutcome::Results);
        assert_eq!(
            first.results[0].candidate.providers(),
            vec![ProviderId::APPLE]
        );
        let mut last = first;
        while let Some(s) = rx.recv().await {
            last = s;
        }
        assert!(last.complete);
        assert_eq!(last.results[0].candidate.providers().len(), 2);
    }

    #[tokio::test]
    async fn cancellation_stops_the_search() {
        let mut slow = apple();
        slow.delay = Duration::from_secs(5);
        let e = engine(vec![slow], 50, 10_000);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c2.cancel();
        });
        let started = Instant::now();
        let r = e.search(&req("darknet"), cancel).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(r.providers[0].status, ProviderCallStatus::Cancelled);
    }

    #[tokio::test]
    async fn cache_hits_are_reported_limit_applied() {
        let e = engine(vec![apple()], 2000, 8000);
        let cancel = CancellationToken::new();
        let first = e.search(&req("darknet"), cancel.clone()).await;
        assert!(!first.providers[0].from_cache);
        let second = e.search(&req("darknet"), cancel.clone()).await;
        assert!(second.providers[0].from_cache);
        let limited = e
            .search(
                &SearchRequest {
                    query: "darknet".into(),
                    limit: Some(1),
                    ..SearchRequest::default()
                },
                cancel,
            )
            .await;
        assert_eq!(limited.results.len(), 1);
    }

    fn only_for(mut stub: Stub, query: &'static str) -> Stub {
        stub.answers_to = Some(query);
        stub
    }

    #[tokio::test]
    async fn typo_recalled_by_relaxation() {
        let e = engine(
            vec![only_for(apple(), "darknet"), only_for(pi(), "darknet")],
            2000,
            8000,
        );
        let r = e
            .search(&req("Darknet Diariez"), CancellationToken::new())
            .await;
        assert_eq!(r.outcome, SearchOutcome::Results);
        assert!(r.complete);
        assert_eq!(r.query.raw, "Darknet Diariez");
        assert_eq!(r.results[0].candidate.title, "Darknet Diaries");
        let relaxed: Vec<&str> = r.relaxed.iter().map(|q| q.query.as_str()).collect();
        assert_eq!(relaxed, ["darknet", "diariez"]);
        assert_eq!(r.relaxed[0].providers.len(), 2);
        assert!(r.relaxed[0].providers.iter().all(|p| p.candidates > 0));
        assert!(
            r.providers
                .iter()
                .all(|p| p.status == ProviderCallStatus::Ok && p.candidates == 0),
            "{:?}",
            r.providers
        );
        assert_eq!(r.attribution, vec!["Data from stub"]);
    }

    #[tokio::test]
    async fn reads_a_response_without_relaxed() {
        // A server built before relaxation leaves the key out, and the CLI's
        // `--server` search decodes its body into this type.
        let e = engine(vec![only_for(apple(), "darknet")], 2000, 8000);
        let r = e.search(&req("darknet"), CancellationToken::new()).await;
        let mut body = serde_json::to_value(&r).unwrap();
        body.as_object_mut().unwrap().remove("relaxed").unwrap();
        let read: SearchResponse = serde_json::from_value(body).unwrap();
        assert_eq!(read, r);
    }

    #[tokio::test]
    async fn relaxes_only_unanimous_empty_answers() {
        let e = engine(vec![apple()], 2000, 8000);
        let found = e
            .search(&req("darknet diariez"), CancellationToken::new())
            .await;
        assert_eq!(found.outcome, SearchOutcome::Results);
        assert!(found.relaxed.is_empty());

        let mut bad = pi();
        bad.fail = Some(|| ProviderError::Unavailable("503".into()));
        let e = engine(vec![only_for(apple(), "darknet"), bad], 2000, 8000);
        let failed = e
            .search(&req("darknet diariez"), CancellationToken::new())
            .await;
        assert_eq!(failed.outcome, SearchOutcome::NoResults);
        assert!(failed.relaxed.is_empty());

        let mut slow = pi();
        slow.delay = Duration::from_secs(3);
        let e = engine(vec![only_for(apple(), "darknet"), slow], 50, 300);
        let timed_out = e
            .search(&req("darknet diariez"), CancellationToken::new())
            .await;
        assert!(timed_out.relaxed.is_empty());

        let e = engine(vec![only_for(apple(), "example")], 2000, 8000);
        let url = e
            .search(&req("https://example.com/feed"), CancellationToken::new())
            .await;
        assert_eq!(url.outcome, SearchOutcome::NoResults);
        assert!(url.relaxed.is_empty());
    }

    #[tokio::test]
    async fn relaxation_keeps_the_hard_deadline() {
        let mut slow = apple();
        slow.delay = Duration::from_secs(3);
        let e = engine(vec![only_for(slow, "darknet")], 50, 300);
        let started = Instant::now();
        let r = e
            .search(&req("darknet diariez"), CancellationToken::new())
            .await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(r.complete);
        assert_eq!(r.outcome, SearchOutcome::NoResults);
        assert_eq!(r.providers[0].status, ProviderCallStatus::Ok);
        assert_eq!(
            r.relaxed[0].providers[0].status,
            ProviderCallStatus::TimedOut
        );
    }
}
