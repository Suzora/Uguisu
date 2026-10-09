//! Benchmarks for the discovery pipeline (`docs/benchmarks/`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uguisu_core::config::SearchConfig;
use uguisu_discovery::candidate::{PodcastCandidate, ProviderIdentity};
use uguisu_discovery::fuzzy::{fold_for_match, token_set_ratio};
use uguisu_discovery::provider::{
    Capabilities, DiscoveryProvider, ProviderContext, ProviderError, ProviderInfo, ProviderResponse,
};
use uguisu_discovery::query::NormalizedQuery;
use uguisu_discovery::rank::{RankContext, rank};
use uguisu_discovery::{
    DedupThresholds, DiscoveryCache, ProviderId, ProviderRegistry, RegistryConfig, SearchEngine,
    SearchRequest, dedup, merge,
};
use uguisu_http::ThrottleConfig;
use url::Url;

const WORDS: [&str; 16] = [
    "darknet",
    "diaries",
    "daily",
    "hack",
    "gemischtes",
    "dark",
    "net",
    "show",
    "cast",
    "radio",
    "hour",
    "history",
    "science",
    "true",
    "crime",
    "tech",
];

fn synthetic_candidates(n: usize, providers: &[ProviderId]) -> Vec<PodcastCandidate> {
    (0..n)
        .map(|i| {
            let provider = providers[i % providers.len()];
            let title = format!("{} {} {}", WORDS[i % 16], WORDS[(i / 16) % 16], i / 256);
            let identity = ProviderIdentity {
                provider,
                provider_ref: format!("{provider}-{i}"),
                confidence: 1.0,
                url: None,
                fetched_at: OffsetDateTime::UNIX_EPOCH,
            };
            let mut c = PodcastCandidate::new(title, identity);
            // every third candidate shares a feed with its predecessor → dedup work
            let feed_index = if i % 3 == 0 && i > 0 { i - 1 } else { i };
            c.feed_url = Some(Url::parse(&format!("https://feeds.test/{feed_index}")).unwrap());
            c.author = Some(format!("Host {}", i % 40));
            c.episode_count = Some(10);
            c.attribute_all_to(provider);
            c
        })
        .collect()
}

fn bench_normalize(c: &mut Criterion) {
    let queries: Vec<String> = (0..10_000)
        .map(|i| format!("{} {}! Ünïcode {}", WORDS[i % 16], WORDS[(i * 7) % 16], i))
        .collect();
    c.bench_function("normalize 10k queries", |b| {
        b.iter(|| {
            for q in &queries {
                black_box(NormalizedQuery::parse(q));
            }
        });
    });
}

fn bench_fuzzy(c: &mut Criterion) {
    let pairs: Vec<(String, String)> = (0..1000)
        .map(|i| {
            (
                fold_for_match(&format!("{} {}", WORDS[i % 16], WORDS[(i * 3) % 16])),
                fold_for_match(&format!("{} {} {}", WORDS[(i * 5) % 16], WORDS[i % 16], i)),
            )
        })
        .collect();
    c.bench_function("token_set_ratio 1k pairs", |b| {
        b.iter(|| {
            for (a, bb) in &pairs {
                black_box(token_set_ratio(a, bb));
            }
        });
    });
}

fn bench_rank(c: &mut Criterion) {
    let query = NormalizedQuery::parse("darknet diaries");
    let ctx = RankContext {
        trust: HashMap::from([(ProviderId::APPLE, 0.8), (ProviderId::PODCAST_INDEX, 0.9)]),
        now: Some(OffsetDateTime::UNIX_EPOCH),
        ..RankContext::default()
    };
    for n in [100usize, 1000] {
        let cands: Vec<_> = synthetic_candidates(n, &[ProviderId::APPLE])
            .into_iter()
            .map(|c| (c, vec![]))
            .collect();
        c.bench_with_input(BenchmarkId::new("rank", n), &cands, |b, cands| {
            b.iter(|| black_box(rank(&query, cands.clone(), &ctx)));
        });
    }
}

fn bench_dedup_merge(c: &mut Criterion) {
    let trust = HashMap::from([
        (ProviderId::APPLE, 0.8),
        (ProviderId::PODCAST_INDEX, 0.9),
        (ProviderId::GPODDER_NET, 0.6),
    ]);
    for n in [100usize, 1000] {
        let cands = synthetic_candidates(
            n,
            &[
                ProviderId::APPLE,
                ProviderId::PODCAST_INDEX,
                ProviderId::GPODDER_NET,
            ],
        );
        c.bench_with_input(BenchmarkId::new("dedup+merge", n), &cands, |b, cands| {
            b.iter(|| {
                let groups = dedup::group(cands.clone(), &DedupThresholds::default());
                let merged: Vec<_> = groups
                    .into_iter()
                    .map(|g| merge::merge(g.members, &trust))
                    .collect();
                black_box(merged)
            });
        });
    }
}

struct Stub {
    id: ProviderId,
    delay: Duration,
    candidates: Vec<PodcastCandidate>,
}

#[async_trait]
impl DiscoveryProvider for Stub {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.id,
            name: "stub",
            attribution: None,
            docs_url: "",
            capabilities: Capabilities {
                search: true,
                ..Capabilities::default()
            },
            requires_credentials: false,
            throttle: ThrottleConfig::concurrency_only(8),
            trust: 0.8,
        }
    }

    async fn search(
        &self,
        _q: &NormalizedQuery,
        _ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(ProviderResponse::new(self.candidates.clone()))
    }
}

fn engine(delay: Duration) -> SearchEngine {
    let mut registry = ProviderRegistry::new(RegistryConfig::default(), DiscoveryCache::new(1000));
    for (i, id) in [
        ProviderId::APPLE,
        ProviderId::PODCAST_INDEX,
        ProviderId::GPODDER_NET,
    ]
    .into_iter()
    .enumerate()
    {
        // Candidate j of every provider shares feed j, so the engine merges 25 groups of three.
        let _ = i;
        let candidates = synthetic_candidates(25, &[id]);
        registry.register(
            Arc::new(Stub {
                id,
                delay,
                candidates,
            }),
            None,
        );
    }
    SearchEngine::new(
        Arc::new(registry),
        SearchConfig {
            soft_deadline: Duration::from_secs(2),
            hard_deadline: Duration::from_secs(8),
            provider_timeout: Duration::from_secs(6),
            default_limit: 25,
            max_limit: 100,
        },
    )
}

fn bench_search(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let uncached = engine(Duration::ZERO);
    c.bench_function("search 3 providers uncached (25 candidates each)", |b| {
        b.to_async(&rt).iter(|| async {
            let req = SearchRequest {
                query: "darknet diaries".into(),
                no_cache: true,
                ..SearchRequest::default()
            };
            black_box(uncached.search(&req, CancellationToken::new()).await)
        });
    });
    let cached = engine(Duration::ZERO);
    rt.block_on(async {
        let req = SearchRequest {
            query: "darknet diaries".into(),
            ..SearchRequest::default()
        };
        cached.search(&req, CancellationToken::new()).await;
    });
    c.bench_function("search 3 providers cached", |b| {
        b.to_async(&rt).iter(|| async {
            let req = SearchRequest {
                query: "darknet diaries".into(),
                ..SearchRequest::default()
            };
            black_box(cached.search(&req, CancellationToken::new()).await)
        });
    });
    let slow = engine(Duration::from_millis(20));
    c.bench_function(
        "search 3 providers with 20ms latency each (parallel)",
        |b| {
            b.to_async(&rt).iter(|| async {
                let req = SearchRequest {
                    query: "darknet diaries".into(),
                    no_cache: true,
                    ..SearchRequest::default()
                };
                black_box(slow.search(&req, CancellationToken::new()).await)
            });
        },
    );
}

criterion_group!(
    benches,
    bench_normalize,
    bench_fuzzy,
    bench_rank,
    bench_dedup_merge,
    bench_search
);
criterion_main!(benches);
