//! Discovery scenarios the release checklist names that the recorded
//! fixtures do not show: an obscure show, similar names, providers that
//! disagree, and a provider that cannot be reached. The table of all eight
//! and where each is tested is in `tests/fixtures/discovery/README.md`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{
    apple_answers, discovery, discovery_at, gpodder_answers, podcast_index_answers, req, serve_feed,
};
use tokio_util::sync::CancellationToken;
use uguisu_discovery::{ProviderCallStatus, ProviderId, SearchOutcome};
use wiremock::MockServer;

#[tokio::test]
async fn obscure_show_ranks_and_resolves() {
    let server = MockServer::start().await;
    let feed = serve_feed(&server, "/kleinstadtfunk.xml", "Kleinstadtfunk").await;
    // Only one directory knows the show; another offers a popular show
    // that merely shares a word.
    apple_answers(&server, serde_json::json!([])).await;
    podcast_index_answers(
        &server,
        serde_json::json!([{
            "id": 7001,
            "title": "Kleinstadtfunk",
            "url": feed,
            "author": "Verein Kleinstadtfunk",
            "episodeCount": 4,
            "dead": 0,
            "lastHttpStatus": 200
        }]),
    )
    .await;
    gpodder_answers(
        &server,
        serde_json::json!([{
            "url": "https://funk-classics.test/rss",
            "title": "Funk Classics",
            "author": "Big Radio",
            "subscribers": 50000,
            "website": "https://funk-classics.test/"
        }]),
    )
    .await;
    let d = discovery(&server, 2000, 8000);

    let r = d
        .engine
        .search(&req("kleinstadtfunk"), CancellationToken::new())
        .await;
    assert_eq!(r.outcome, SearchOutcome::Results);
    let top = &r.results[0];
    assert_eq!(top.candidate.title, "Kleinstadtfunk", "{:#?}", r.results);
    assert_eq!(top.candidate.providers(), vec![ProviderId::PODCAST_INDEX]);
    let resolved = d
        .resolver
        .resolve_candidate(&top.candidate, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resolved.title.as_deref(), Some("Kleinstadtfunk"));
}

#[tokio::test]
async fn similar_names_stay_apart() {
    let server = MockServer::start().await;
    // Two different shows with one name: other authors, feeds and sites.
    apple_answers(
        &server,
        serde_json::json!([{
            "wrapperType": "track",
            "kind": "podcast",
            "collectionId": 1_200_361_736_u64,
            "collectionName": "The Daily",
            "artistName": "The New York Times",
            "feedUrl": "https://feeds.daily-news.test/the-daily",
            "collectionViewUrl": "https://podcasts.apple.test/id1200361736"
        }]),
    )
    .await;
    podcast_index_answers(
        &server,
        serde_json::json!([{
            "id": 8001,
            "title": "The Daily",
            "url": "https://hope-church.test/daily.xml",
            "link": "https://hope-church.test/",
            "author": "Hope Church Devotions",
            "itunesId": 1_500_000_777_u64,
            "dead": 0
        }]),
    )
    .await;
    gpodder_answers(&server, serde_json::json!([])).await;
    let d = discovery(&server, 2000, 8000);

    let r = d
        .engine
        .search(&req("the daily"), CancellationToken::new())
        .await;
    assert_eq!(r.results.len(), 2, "{:#?}", r.results);
    for result in &r.results {
        assert_eq!(
            result.candidate.providers().len(),
            1,
            "{:#?}",
            result.candidate
        );
        assert!(
            result
                .ambiguities
                .iter()
                .any(|a| a.other_title == "The Daily"),
            "each names the other as possibly the same: {:#?}",
            result.ambiguities
        );
    }
}

#[tokio::test]
async fn disagreeing_providers_keep_provenance() {
    let server = MockServer::start().await;
    let moved = serve_feed(&server, "/signal-path.xml", "Signal Path").await;
    // One show by its iTunes id: Apple still has the old feed address and
    // its own artwork, Podcast Index the address the feed moved to.
    apple_answers(
        &server,
        serde_json::json!([{
            "wrapperType": "track",
            "kind": "podcast",
            "collectionId": 4242,
            "collectionName": "Signal Path",
            "artistName": "Ana Ruiz",
            "feedUrl": "https://old-host.test/signal-path.xml",
            "artworkUrl600": "https://img.apple.test/4242/600x600bb.jpg",
            "collectionViewUrl": "https://podcasts.apple.test/id4242"
        }]),
    )
    .await;
    podcast_index_answers(
        &server,
        serde_json::json!([{
            "id": 9001,
            "title": "Signal Path",
            "url": moved,
            "author": "Ana Ruiz",
            "image": "https://img.pi.test/9001.jpg",
            "itunesId": 4242,
            "dead": 0
        }]),
    )
    .await;
    gpodder_answers(&server, serde_json::json!([])).await;
    let d = discovery(&server, 2000, 8000);

    let r = d
        .engine
        .search(&req("signal path"), CancellationToken::new())
        .await;
    assert_eq!(
        r.results.len(),
        1,
        "merged by the iTunes id: {:#?}",
        r.results
    );
    let merged = &r.results[0].candidate;
    assert_eq!(merged.providers().len(), 2);
    assert_eq!(
        merged.feed_url.as_ref().map(url::Url::as_str),
        Some(moved.as_str())
    );
    assert_eq!(
        merged.provenance.get("feed_url"),
        Some(&ProviderId::PODCAST_INDEX)
    );
    assert_eq!(
        merged.artwork.as_ref().map(url::Url::as_str),
        Some("https://img.apple.test/4242/600x600bb.jpg")
    );
    assert_eq!(merged.provenance.get("artwork"), Some(&ProviderId::APPLE));
    let resolved = d
        .resolver
        .resolve_candidate(merged, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resolved.title.as_deref(), Some("Signal Path"));
}

#[tokio::test]
async fn unreachable_provider_is_survivable() {
    let server = MockServer::start().await;
    podcast_index_answers(
        &server,
        serde_json::json!([{
            "id": 1001,
            "title": "Darknet Diaries",
            "url": "https://feeds.darknet.test/rss",
            "author": "Jack Rhysider",
            "dead": 0
        }]),
    )
    .await;
    gpodder_answers(&server, serde_json::json!([])).await;
    // A loopback port nothing listens on: the connection is refused, it
    // does not hang.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let uri = server.uri();
    let d = discovery_at(&closed, &uri, &uri, 2000, 8000);

    let r = d
        .engine
        .search(&req("darknet diaries"), CancellationToken::new())
        .await;
    assert_eq!(r.outcome, SearchOutcome::Results);
    assert!(r.complete);
    let apple = r
        .providers
        .iter()
        .find(|p| p.provider == ProviderId::APPLE)
        .unwrap();
    assert_eq!(apple.status, ProviderCallStatus::Failed, "{apple:?}");
    assert_eq!(r.results[0].candidate.title, "Darknet Diaries");
    assert_eq!(
        r.results[0].candidate.providers(),
        vec![ProviderId::PODCAST_INDEX]
    );
}
