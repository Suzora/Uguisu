//! End-to-end refresh benchmarks on a temporary database with a local
//! mock feed host (`docs/benchmarks/`). No network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use uguisu_core::config::{DataConfig, DiscoveryConfig, FeedConfig};
use uguisu_core::feed::RefreshOutcome;
use uguisu_core::ids::PodcastId;
use uguisu_engine::{Engine, EngineConfig, RefreshOptions};
use uguisu_http::CancellationToken;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A synthetic feed with `count` items and no `podcast:guid`, so every
/// benchmark podcast is its own show.
fn feed(count: usize, changed: Option<usize>) -> Vec<u8> {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\n<channel>\n<title>Bench Show</title>\n<link>https://bench.example/</link>\n<description>Generated</description>\n",
    );
    for i in 0..count {
        let day = 1 + (i % 28);
        let month = 1 + (i / 28) % 12;
        let year = 2018 + (i / 336) % 8;
        let title = if changed == Some(i) {
            format!("Episode {i}: revised")
        } else {
            format!("Episode {i}: the one about things")
        };
        let _ = writeln!(
            s,
            "<item><title>{title}</title><guid isPermaLink=\"false\">ep-{i}</guid>\
             <pubDate>{day:02} {mon} {year} 10:00:00 +0000</pubDate>\
             <description><![CDATA[<p>Show notes for <b>{i}</b>.</p>]]></description>\
             <itunes:duration>{}:{:02}:00</itunes:duration>\
             <enclosure url=\"https://cdn.bench.example/media/{i}.mp3\" type=\"audio/mpeg\" length=\"{}\"/>\
             </item>",
            (i % 3) + 1,
            i % 60,
            10_000_000 + i * 1000,
            mon = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"
            ][month - 1],
        );
    }
    s.push_str("</channel>\n</rss>\n");
    s.into_bytes()
}

struct Bench {
    _dir: tempfile::TempDir,
    server: MockServer,
    engine: Engine,
}

async fn setup() -> Bench {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let mut discovery = DiscoveryConfig::default();
    discovery.apple.enabled = false;
    discovery.gpoddernet.enabled = false;
    discovery.network.allow_private_hosts = vec!["127.0.0.1".to_owned()];
    let engine = Engine::open(
        EngineConfig::default()
            .with_data(DataConfig {
                data_dir: Some(dir.path().to_path_buf()),
                media_dir: None,
            })
            .with_feed(FeedConfig::default())
            .with_discovery(discovery),
    )
    .await
    .unwrap();
    Bench {
        _dir: dir,
        server,
        engine,
    }
}

async fn serve(server: &MockServer, p: &str, body: &[u8]) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(server)
        .await;
}

async fn add(b: &Bench, p: &str) -> PodcastId {
    b.engine
        .add_podcast(&format!("{}{p}", b.server.uri()), CancellationToken::new())
        .await
        .unwrap()
        .podcast
        .id
}

async fn refresh(b: &Bench, id: PodcastId, force: bool) -> RefreshOutcome {
    b.engine
        .refresh_podcast(
            id,
            RefreshOptions {
                force,
                cancel: None,
            },
        )
        .await
        .unwrap()
        .outcome
}

fn bench_refresh(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut g = c.benchmark_group("refresh");
    g.sample_size(10);
    for &n in &[1_000usize, 10_000] {
        let body = feed(n, None);
        let plus_one = feed(n + 1, None);
        let changed = feed(n, Some(n / 2));
        g.throughput(Throughput::Elements(n as u64));

        // Cold import: a fresh podcast per iteration on one engine.
        let b = rt.block_on(setup());
        rt.block_on(async {
            Mock::given(method("GET"))
                .and(path_regex(r"^/cold/\d+\.xml$"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
                .mount(&b.server)
                .await;
        });
        let counter = AtomicUsize::new(0);
        g.bench_function(BenchmarkId::new("cold_import", n), |bench| {
            let (b, counter) = (&b, &counter);
            bench.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let i = counter.fetch_add(1, Ordering::Relaxed);
                    let started = Instant::now();
                    add(b, &format!("/cold/{i}.xml")).await;
                    total += started.elapsed();
                }
                total
            });
        });
        rt.block_on(b.engine.close());

        // Steady state on one podcast.
        let b = rt.block_on(setup());
        rt.block_on(serve(&b.server, "/feed.xml", &body));
        let id = rt.block_on(add(&b, "/feed.xml"));

        g.bench_function(BenchmarkId::new("identical_fingerprint", n), |bench| {
            bench.to_async(&rt).iter(|| async {
                assert!(matches!(
                    refresh(&b, id, false).await,
                    RefreshOutcome::NotModified { .. }
                ));
            });
        });
        g.bench_function(
            BenchmarkId::new("identical_forced_full_parse", n),
            |bench| {
                bench.to_async(&rt).iter(|| async {
                    assert_eq!(refresh(&b, id, true).await, RefreshOutcome::Fetched);
                });
            },
        );
        // Alternate between n and n+1 items: each iteration adds or removes one.
        // Alternation must continue across criterion's calls, or the same
        // body would be served twice and hit the fingerprint.
        let flips = AtomicUsize::new(0);
        g.bench_function(BenchmarkId::new("one_added_or_missing", n), |bench| {
            let (b, plus_one, body, flips) = (&b, &plus_one, &body, &flips);
            bench.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let i = flips.fetch_add(1, Ordering::Relaxed);
                    b.server.reset().await;
                    serve(
                        &b.server,
                        "/feed.xml",
                        if i % 2 == 0 { plus_one } else { body },
                    )
                    .await;
                    let started = Instant::now();
                    assert_eq!(refresh(b, id, false).await, RefreshOutcome::Fetched);
                    total += started.elapsed();
                }
                total
            });
        });
        let flips = AtomicUsize::new(0);
        g.bench_function(BenchmarkId::new("one_updated", n), |bench| {
            let (b, changed, body, flips) = (&b, &changed, &body, &flips);
            bench.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let i = flips.fetch_add(1, Ordering::Relaxed);
                    b.server.reset().await;
                    serve(
                        &b.server,
                        "/feed.xml",
                        if i % 2 == 0 { changed } else { body },
                    )
                    .await;
                    let started = Instant::now();
                    assert_eq!(refresh(b, id, false).await, RefreshOutcome::Fetched);
                    total += started.elapsed();
                }
                total
            });
        });
        rt.block_on(b.engine.close());
    }
    g.finish();
}

criterion_group!(benches, bench_refresh);
criterion_main!(benches);
