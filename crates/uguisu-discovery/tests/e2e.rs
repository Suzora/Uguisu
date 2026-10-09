//! End-to-end: three wiremock-backed providers → search → dedup/merge/rank →
//! resolve the top result to a verified feed; outage, cache and streaming
//! variants.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

mod common;

use common::{discovery, req};
use tokio_util::sync::CancellationToken;
use uguisu_discovery::testing::Fixture;
use uguisu_discovery::{ProviderCallStatus, ProviderId, SearchOutcome, SearchRequest};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FEED: &str = include_str!("../../../tests/fixtures/feeds/probe/rss_itunes_podcast.xml");
const MEGAPHONE: &str = "feeds.megaphone.fm/darknetdiaries";

/// Mounts the three `search_darknet` fixtures with the Darknet Diaries feed
/// URL rewritten to the mock feed so resolution can succeed offline.
async fn mount_all(server: &MockServer) {
    let feed = format!("{}/feed.xml", server.uri());
    for (provider, case) in [
        ("apple", "search_darknet"),
        ("podcastindex", "search_darknet"),
        ("gpoddernet", "search_darknet"),
    ] {
        let mut f = Fixture::load(provider, case).unwrap();
        let body = f
            .response
            .body
            .to_string()
            .replace(&format!("https://{MEGAPHONE}"), &feed)
            .replace(&format!("http://{MEGAPHONE}"), &feed);
        f.response.body = serde_json::from_str(&body).unwrap();
        f.mount(server).await;
    }
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(FEED),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn misspelled_query_merges_three_providers_and_resolves() {
    let server = MockServer::start().await;
    mount_all(&server).await;
    // The providers are queried with the raw query; the fixtures answer to "darknet diaries",
    // so we search for that here and check the ranking of the fuzzy variants in the unit tests.
    let d = discovery(&server, 2000, 8000);
    let started = Instant::now();
    let r = d
        .engine
        .search(&req("darknet diaries"), CancellationToken::new())
        .await;
    assert_eq!(r.outcome, SearchOutcome::Results);
    assert!(r.complete);
    assert!(
        r.providers
            .iter()
            .all(|p| p.status == ProviderCallStatus::Ok),
        "{:?}",
        r.providers
    );
    // Three healthy providers finish under the soft deadline, so a search
    // never waits it out.
    assert!(
        started.elapsed() < Duration::from_millis(2000),
        "{:?}",
        started.elapsed()
    );
    assert!(
        r.providers.iter().all(|p| p.latency_ms < 2000),
        "{:?}",
        r.providers
    );

    let top = &r.results[0];
    assert_eq!(top.rank, 1);
    assert_eq!(top.candidate.title, "Darknet Diaries");
    assert_eq!(
        top.candidate.providers().len(),
        3,
        "Apple, Podcast Index and gpodder.net entries merged: {:?}",
        top.candidate.providers()
    );
    assert_eq!(
        top.candidate.provenance.get("feed_url"),
        Some(&ProviderId::PODCAST_INDEX)
    );
    assert_eq!(
        top.candidate.provenance.get("artwork"),
        Some(&ProviderId::APPLE)
    );
    assert_eq!(
        top.candidate.podcast_guid.as_deref(),
        Some("11111111-1111-5111-8111-111111111111")
    );
    assert_eq!(top.candidate.itunes_id, Some(1_296_350_485));
    assert!(
        top.candidate.popularity_score().is_some(),
        "gpodder subscribers carried over"
    );
    assert!(
        top.explanation
            .signals
            .iter()
            .any(|s| s.name == "agreement" && s.value >= 0.99)
    );
    assert_eq!(r.attribution.len(), 3);
    assert!(
        r.results
            .iter()
            .any(|x| x.candidate.title == "Darknet Deep Dive"
                && x.explanation
                    .signals
                    .iter()
                    .any(|s| s.name == "feed_quality" && s.value == 0.0)),
        "dead feed gets zero feed quality"
    );

    let feed = d
        .resolver
        .resolve_candidate(&top.candidate, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(feed.title.as_deref(), Some("Example Diaries"));
    assert_eq!(feed.items_with_media, 3);
    assert!(
        feed.provenance
            .iter()
            .any(|s| format!("{:?}", s.kind) == "Validate" && s.ok)
    );
}

#[tokio::test]
async fn outages_yield_partial_then_all_failed() {
    let server = MockServer::start().await;
    mount_all(&server).await;
    // gpodder answers 503, Podcast Index hangs beyond the hard deadline, Apple works.
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/byterm"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .with_priority(1)
        .mount(&server)
        .await;
    // 503s get one quick retry from the HTTP client; the hard deadline leaves room for it to fail
    // before Podcast Index's 5 s stall trips the deadline.
    let d = discovery(&server, 100, 3000);
    let started = Instant::now();
    let r = d
        .engine
        .search(&req("darknet diaries"), CancellationToken::new())
        .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(r.outcome, SearchOutcome::Results);
    let status = |id: ProviderId| {
        r.providers
            .iter()
            .find(|p| p.provider == id)
            .unwrap()
            .status
    };
    assert_eq!(status(ProviderId::APPLE), ProviderCallStatus::Ok);
    assert_eq!(status(ProviderId::GPODDER_NET), ProviderCallStatus::Failed);
    assert_eq!(
        status(ProviderId::PODCAST_INDEX),
        ProviderCallStatus::TimedOut
    );
    assert_eq!(r.results[0].candidate.providers(), vec![ProviderId::APPLE]);

    // Everything down.
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .with_priority(1)
        .mount(&server)
        .await;
    let r = d
        .engine
        .search(
            &SearchRequest {
                no_cache: true,
                ..req("darknet diaries")
            },
            CancellationToken::new(),
        )
        .await;
    assert_eq!(r.outcome, SearchOutcome::AllProvidersFailed);
    assert!(r.results.is_empty());
    assert_eq!(
        status(ProviderId::APPLE),
        ProviderCallStatus::Ok,
        "first response untouched"
    );
    let apple = r
        .providers
        .iter()
        .find(|p| p.provider == ProviderId::APPLE)
        .unwrap();
    assert_eq!(apple.status, ProviderCallStatus::Failed);
    assert_eq!(apple.error_kind.as_deref(), Some("rate_limited"));
}

#[tokio::test]
async fn cache_serves_repeat_searches() {
    let server = MockServer::start().await;
    mount_all(&server).await;
    let d = discovery(&server, 2000, 8000);
    let first = d
        .engine
        .search(&req("darknet diaries"), CancellationToken::new())
        .await;
    assert!(first.providers.iter().all(|p| !p.from_cache));
    let second = d
        .engine
        .search(&req("Darknet   Diaries!"), CancellationToken::new())
        .await;
    assert!(
        second.providers.iter().all(|p| p.from_cache),
        "{:?}",
        second.providers
    );
    assert_eq!(second.results[0].candidate.title, "Darknet Diaries");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path().contains("search"))
            .count(),
        3,
        "each provider hit exactly once"
    );
    assert_eq!(d.registry.cache().stats().hits, 3);
}

#[tokio::test]
async fn streaming_delivers_early_snapshot() {
    let server = MockServer::start().await;
    mount_all(&server).await;
    Mock::given(method("GET"))
        .and(path("/search/byterm"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(700))
                .set_body_json(serde_json::json!({"status": "true", "feeds": [], "count": 0})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let d = discovery(&server, 100, 5000);
    let mut rx = d
        .engine
        .search_stream(req("darknet diaries"), CancellationToken::new());
    let started = Instant::now();
    let first = rx.recv().await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "first snapshot must not wait for the slow provider"
    );
    assert!(!first.complete);
    assert_eq!(first.outcome, SearchOutcome::Results);
    let mut last = first;
    while let Some(s) = rx.recv().await {
        last = s;
    }
    assert!(last.complete);
    assert_eq!(last.providers.len(), 3);
}

#[tokio::test]
async fn misspelling_recalled_from_recordings() {
    let server = MockServer::start().await;
    for provider in ["apple", "gpoddernet"] {
        for case in ["search_misspelled", "search_partial"] {
            Fixture::load_live(provider, case)
                .unwrap()
                .mount(&server)
                .await;
        }
        // The second relaxed query, "diariez", finds nothing either.
        let mut f = Fixture::load_live(provider, "search_misspelled").unwrap();
        for v in f.request.query.values_mut() {
            if v == "darknet diariez" {
                "diariez".clone_into(v);
            }
        }
        f.mount(&server).await;
    }
    Mock::given(method("GET"))
        .and(path("/search/byterm"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"status": "true", "feeds": [], "count": 0})),
        )
        .mount(&server)
        .await;
    let d = discovery(&server, 2000, 8000);
    let r = d
        .engine
        .search(&req("darknet diariez"), CancellationToken::new())
        .await;
    assert_eq!(r.outcome, SearchOutcome::Results);
    assert!(
        r.providers
            .iter()
            .all(|p| p.status == ProviderCallStatus::Ok && p.candidates == 0),
        "{:?}",
        r.providers
    );
    let relaxed: Vec<&str> = r.relaxed.iter().map(|q| q.query.as_str()).collect();
    assert_eq!(relaxed, ["darknet", "diariez"]);
    assert!(
        r.relaxed
            .iter()
            .flat_map(|q| &q.providers)
            .all(|p| p.status == ProviderCallStatus::Ok),
        "{:?}",
        r.relaxed
    );
    let top = &r.results[0].candidate;
    assert_eq!(top.title, "Darknet Diaries");
    assert_eq!(top.itunes_id, Some(1_296_350_485));
}
