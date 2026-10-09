//! Shared helpers for the discovery suites: a discovery stack pointed at
//! mock providers, and the answers those providers give. Tests never touch
//! the network.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use uguisu_core::config::DiscoveryConfig;
use uguisu_discovery::{Discovery, SearchRequest, assemble};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A discovery stack whose three providers answer at the given base URLs.
pub fn discovery_at(
    apple: &str,
    podcast_index: &str,
    gpodder: &str,
    soft_ms: u64,
    hard_ms: u64,
) -> Discovery {
    let cfg = DiscoveryConfig::from_lookup(|k| match k {
        "UGUISU_DISCOVERY_APPLE_BASE_URL" => Some(apple.to_owned()),
        "UGUISU_DISCOVERY_PODCASTINDEX_BASE_URL" => Some(podcast_index.to_owned()),
        "UGUISU_DISCOVERY_GPODDERNET_BASE_URL" => Some(gpodder.to_owned()),
        "UGUISU_PODCASTINDEX_KEY" => Some("k".into()),
        "UGUISU_PODCASTINDEX_SECRET" => Some("s".into()),
        "UGUISU_DISCOVERY_GPODDERNET_ENABLED" => Some("true".into()),
        "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS" => Some("127.0.0.1".into()),
        "UGUISU_DISCOVERY_SOFT_DEADLINE_MS" => Some(soft_ms.to_string()),
        "UGUISU_DISCOVERY_HARD_DEADLINE_MS" | "UGUISU_DISCOVERY_PROVIDER_TIMEOUT_MS" => {
            Some(hard_ms.to_string())
        }
        _ => None,
    })
    .unwrap();
    assemble(cfg).unwrap()
}

/// A discovery stack whose three providers all answer at `server`.
pub fn discovery(server: &MockServer, soft_ms: u64, hard_ms: u64) -> Discovery {
    let uri = server.uri();
    discovery_at(&uri, &uri, &uri, soft_ms, hard_ms)
}

pub fn req(q: &str) -> SearchRequest {
    SearchRequest {
        query: q.into(),
        ..SearchRequest::default()
    }
}

async fn answer(server: &MockServer, at: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Apple's search answers with these `results` entries, whatever the term.
pub async fn apple_answers(server: &MockServer, results: serde_json::Value) {
    let count = results.as_array().map_or(0, Vec::len);
    answer(
        server,
        "/search",
        serde_json::json!({ "resultCount": count, "results": results }),
    )
    .await;
}

/// Podcast Index's term search answers with these `feeds`.
pub async fn podcast_index_answers(server: &MockServer, feeds: serde_json::Value) {
    let count = feeds.as_array().map_or(0, Vec::len);
    answer(
        server,
        "/search/byterm",
        serde_json::json!({ "status": "true", "feeds": feeds, "count": count }),
    )
    .await;
}

/// gpodder.net's search answers with these podcasts.
pub async fn gpodder_answers(server: &MockServer, podcasts: serde_json::Value) {
    answer(server, "/search.json", podcasts).await;
}

/// A minimal podcast feed titled `title`, served at `at`; returns its URL.
pub async fn serve_feed(server: &MockServer, at: &str, title: &str) -> String {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title>{title}</title>
    <link>https://example.test/</link>
    <description>{title}, the feed.</description>
    <item>
      <title>Episode 1</title>
      <guid isPermaLink="false">ep-1</guid>
      <pubDate>Tue, 02 Sep 2025 10:00:00 +0000</pubDate>
      <enclosure url="https://media.example.test/ep1.mp3" length="12345" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#
    );
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(xml),
        )
        .mount(server)
        .await;
    format!("{}{at}", server.uri())
}
