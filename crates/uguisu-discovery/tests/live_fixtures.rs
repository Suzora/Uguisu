//! Replays the fixtures recorded from the live providers, websites and feed
//! hosts on 2026-09-17 (`tests/fixtures/discovery/live`, `origin: recorded`).
//!
//! These tests never touch the network: every recording is served by
//! wiremock. They prove that the providers map the *real* response shapes
//! and that the resolver handles real autodiscovery markup, redirect chains
//! and feed documents the way the live run did.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use tokio_util::sync::CancellationToken;
use uguisu_core::config::{AppleConfig, GpodderNetConfig, PodcastIndexConfig};
use uguisu_core::secret::Secret;
use uguisu_discovery::provider::{DiscoveryProvider, ProviderContext, ProviderError, ProviderRef};
use uguisu_discovery::providers::apple::AppleProvider;
use uguisu_discovery::providers::gpoddernet::GpodderNetProvider;
use uguisu_discovery::providers::podcastindex::PodcastIndexProvider;
use uguisu_discovery::resolve::{ResolveError, Resolver, ResolverConfig, StepKind};
use uguisu_discovery::testing::{Fixture, FixtureOrigin};
use uguisu_discovery::{NormalizedQuery, ProviderId};
use uguisu_http::{HttpClient, NetworkPolicy, Profile, Url};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(profile: Profile) -> HttpClient {
    HttpClient::with_policy(
        profile,
        NetworkPolicy::strict().allow_private_hosts(["127.0.0.1"]),
    )
    .unwrap()
}

fn live(provider: &str, case: &str) -> Fixture {
    let f = Fixture::load_live(provider, case).unwrap();
    assert_eq!(f.origin, FixtureOrigin::Recorded, "{provider}/{case}");
    f
}

/// Body of a recorded website or feed as text.
fn web_body(host: &str, case: &str) -> (Fixture, String) {
    let f = live(&format!("web/{host}"), case);
    let body = f
        .response
        .body
        .as_str()
        .expect("web fixtures store the body as a string")
        .to_owned();
    (f, body)
}

/// Serves `body` at `path` with the recorded content type.
async fn serve(server: &MockServer, p: &str, fixture: &Fixture, body: &str) {
    let ct = fixture
        .response
        .headers
        .get("content-type")
        .map_or("text/html", String::as_str);
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(
            ResponseTemplate::new(fixture.response.status)
                .insert_header("content-type", ct)
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

fn resolver() -> Resolver {
    Resolver::new(
        client(Profile::Feed),
        None,
        ResolverConfig {
            https_upgrade: false,
            ..ResolverConfig::default()
        },
    )
}

// Apple

fn apple(server: &MockServer) -> AppleProvider {
    AppleProvider::new(
        client(Profile::Discovery),
        &AppleConfig {
            enabled: true,
            country: "US".into(),
            lang: None,
            base_url: server.uri(),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn apple_live_search_maps_real_fields() {
    let server = MockServer::start().await;
    live("apple", "search_darknet").mount(&server).await;
    let resp = apple(&server)
        .search(
            &NormalizedQuery::parse("Darknet Diaries"),
            &ProviderContext::default(),
        )
        .await
        .unwrap();
    // Apple sends max-age=86400 on searches; the registry caps it at the configured TTL.
    assert_eq!(
        resp.cache_max_age,
        Some(std::time::Duration::from_secs(86_400))
    );
    let c = &resp.value;
    assert_eq!(c.len(), 3, "live: Darknet Diaries, en español, Deutsch");
    let top = &c[0];
    assert_eq!(top.title, "Darknet Diaries");
    assert_eq!(top.author.as_deref(), Some("Jack Rhysider"));
    assert_eq!(
        top.feed_url.as_ref().map(Url::as_str),
        Some("https://podcast.darknetdiaries.com/"),
        "live feed URL is the PRX host, not megaphone"
    );
    assert_eq!(top.itunes_id, Some(1_296_350_485));
    assert_eq!(top.episode_count, Some(198));
    assert_eq!(top.explicit, Some(false));
    assert_eq!(
        top.categories,
        vec!["Technology"],
        "'Podcasts' genre dropped"
    );
    assert!(top.artwork.as_ref().unwrap().as_str().contains("600x600bb"));
    assert!(top.last_published.is_some());
    assert_eq!(
        top.language, None,
        "Apple's `country` (USA) must not be mapped to a language"
    );
    assert_eq!(
        top.identities[0].url.as_ref().map(Url::as_str),
        Some("https://podcasts.apple.com/us/podcast/darknet-diaries/id1296350485?uo=4")
    );
    assert_eq!(c[2].author.as_deref(), Some("heise online"));
}

#[tokio::test]
async fn apple_live_search_is_not_fuzzy() {
    let server = MockServer::start().await;
    live("apple", "search_misspelled").mount(&server).await;
    live("apple", "search_partial").mount(&server).await;
    let p = apple(&server);
    let ctx = ProviderContext::default();
    let misspelled = p
        .search(&NormalizedQuery::parse("darknet diariez"), &ctx)
        .await
        .unwrap();
    assert!(
        misspelled.value.is_empty(),
        "Apple returns nothing for a typo; fuzzy ranking cannot help without candidates"
    );
    let partial = p
        .search(&NormalizedQuery::parse("darknet"), &ctx)
        .await
        .unwrap();
    assert!(partial.value.len() >= 10);
    assert!(
        partial
            .value
            .iter()
            .any(|c| c.title == "Darknet Diaries" && c.itunes_id == Some(1_296_350_485))
    );
}

#[tokio::test]
async fn apple_live_lookup_known_and_unknown_ids() {
    let server = MockServer::start().await;
    live("apple", "lookup_id").mount(&server).await;
    live("apple", "lookup_unknown").mount(&server).await;
    let p = apple(&server);
    let ctx = ProviderContext::default();
    let found = p
        .lookup(&ProviderRef::ItunesId(1_296_350_485), &ctx)
        .await
        .unwrap();
    assert_eq!(found.value.unwrap().title, "Darknet Diaries");
    let missing = p.lookup(&ProviderRef::ItunesId(1), &ctx).await.unwrap();
    assert!(
        missing.value.is_none(),
        "Apple answers 200 with resultCount 0 for unknown ids"
    );
}

#[tokio::test]
async fn apple_live_german_storefront() {
    let server = MockServer::start().await;
    live("apple", "search_german").mount(&server).await;
    let resp = apple(&server)
        .search(
            &NormalizedQuery::parse("Gemischtes Hack"),
            &ProviderContext {
                country: Some("DE".into()),
                limit: 10,
                ..ProviderContext::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(resp.value[0].title, "Gemischtes Hack");
    assert_eq!(
        resp.value[0].feed_url.as_ref().map(Url::as_str),
        Some("https://feeds.megaphone.fm/GLT8390938385")
    );
    let req = &server.received_requests().await.unwrap()[0];
    assert!(req.url.query().unwrap().contains("country=DE"));
}

// gpodder.net

fn gpodder(server: &MockServer) -> GpodderNetProvider {
    GpodderNetProvider::new(
        client(Profile::Discovery),
        &GpodderNetConfig {
            enabled: true,
            base_url: server.uri(),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn gpoddernet_live_lists_feed_variants() {
    let server = MockServer::start().await;
    live("gpoddernet", "search_darknet").mount(&server).await;
    let resp = gpodder(&server)
        .search(
            &NormalizedQuery::parse("Darknet Diaries"),
            &ProviderContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.cache_max_age,
        Some(std::time::Duration::from_secs(3600))
    );
    let c = &resp.value;
    assert_eq!(c.len(), 4, "four feed variants of the same show");
    assert!(
        c.iter()
            .all(|x| x.author.as_deref() == Some("Jack Rhysider"))
    );
    assert!(
        c.iter()
            .all(|x| x.website.as_ref().map(Url::as_str) == Some("https://darknetdiaries.com/"))
    );
    assert!(
        c.iter()
            .all(|x| x.popularity[0].raw == 0.0 && x.popularity[0].normalized == 0.0),
        "live gpodder.net reports 0 subscribers for every result"
    );
    let feeds: Vec<&str> = c
        .iter()
        .map(|x| x.feed_url.as_ref().unwrap().as_str())
        .collect();
    assert!(feeds.contains(&"https://feeds.megaphone.fm/darknetdiaries"));
    assert!(feeds.contains(&"https://darknetdiaries.com/feedfree.xml"));
    assert!(
        c[0].artwork
            .as_ref()
            .unwrap()
            .as_str()
            .starts_with("http://gpodder.net/logo/256/"),
        "scaled logos are served over plain http"
    );
}

#[tokio::test]
async fn gpoddernet_live_lookup_and_html_404() {
    let server = MockServer::start().await;
    live("gpoddernet", "podcast_by_url").mount(&server).await;
    live("gpoddernet", "podcast_by_url_404")
        .mount(&server)
        .await;
    live("gpoddernet", "search_misspelled").mount(&server).await;
    let p = gpodder(&server);
    let ctx = ProviderContext::default();
    let found = p
        .lookup(
            &ProviderRef::FeedUrl(Url::parse("https://feeds.megaphone.fm/darknetdiaries").unwrap()),
            &ctx,
        )
        .await
        .unwrap();
    let found = found.value.unwrap();
    assert_eq!(found.title, "Darknet Diaries");
    assert_eq!(
        found.identities[0].url.as_ref().map(Url::as_str),
        Some("http://gpodder.net/podcast/darknet-diaries-2")
    );
    let missing = p
        .lookup(
            &ProviderRef::FeedUrl(Url::parse("https://nowhere.invalid/feed.xml").unwrap()),
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        missing.value.is_none(),
        "the live 404 is an HTML page, not JSON; it must map to `None`"
    );
    let typo = p
        .search(&NormalizedQuery::parse("darknet diariez"), &ctx)
        .await
        .unwrap();
    assert!(
        typo.value.is_empty(),
        "gpodder.net search is not fuzzy either"
    );
}

// Podcast Index

#[tokio::test]
async fn podcastindex_live_401_is_plain_text() {
    let server = MockServer::start().await;
    live("podcastindex", "error_401_invalid_key")
        .mount(&server)
        .await;
    live("podcastindex", "byfeedurl_401").mount(&server).await;
    let p = PodcastIndexProvider::new(
        client(Profile::Discovery),
        &PodcastIndexConfig {
            enabled: true,
            key: Some(Secret::new("INVALIDKEY".to_owned())),
            secret: Some(Secret::new("INVALIDSECRET".to_owned())),
            base_url: format!("{}/api/1.0", server.uri()),
        },
    )
    .unwrap();
    let ctx = ProviderContext::default();
    let err = p
        .search(&NormalizedQuery::parse("Darknet Diaries"), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::AuthRejected), "{err:?}");
    let err = p
        .lookup(
            &ProviderRef::FeedUrl(Url::parse("https://feeds.megaphone.fm/darknetdiaries").unwrap()),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::AuthRejected), "{err:?}");
    // The live 401 carries `no-cache`; such answers must never be cached.
    let f = live("podcastindex", "error_401_invalid_key");
    assert_eq!(
        f.response.headers.get("cache-control").map(String::as_str),
        Some("no-cache, must-revalidate")
    );
    assert!(
        f.response.body.is_string(),
        "body is text despite application/json"
    );
}

// resolver

#[tokio::test]
async fn resolver_replays_darknetdiaries_autodiscovery_and_redirect_chain() {
    // darknetdiaries.com → <link rel=alternate> feeds.megaphone.fm/darknetdiaries
    // → 301 → podcast.darknetdiaries.com (recorded chain, replayed on one host).
    let server = MockServer::start().await;
    let (page, html) = web_body("darknetdiaries_com", "darknetdiaries_home");
    let html = html.replace(
        "https://feeds.megaphone.fm/darknetdiaries",
        &format!("{}/darknetdiaries", server.uri()),
    );
    serve(&server, "/", &page, &html).await;
    Mock::given(method("GET"))
        .and(path("/darknetdiaries"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("location", format!("{}/podcast", server.uri())),
        )
        .mount(&server)
        .await;
    let (feed, xml) = web_body("rss_libsyn_com", "libsyn_feed");
    serve(&server, "/podcast", &feed, &xml).await;

    let r = resolver()
        .resolve(&format!("{}/", server.uri()), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.feed_url.as_str(), format!("{}/podcast", server.uri()));
    assert_eq!(
        r.title.as_deref(),
        Some("Dan Carlin's Hardcore History: Addendum")
    );
    assert!(r.items_with_media > 0);
    assert!(
        r.provenance
            .iter()
            .any(|s| s.kind == StepKind::Autodiscovery && s.ok)
    );
    assert!(
        r.provenance
            .iter()
            .any(|s| s.kind == StepKind::Fetch && s.detail.contains("after 1 redirect")),
        "{:?}",
        r.provenance
    );
}

#[tokio::test]
async fn resolver_replays_relative_and_absolute_link_tags() {
    let server = MockServer::start().await;
    let (feed, xml) = web_body("rss_buzzsprout_com", "buzzsprout_feed");

    // atp.fm: <link href="/rss"> (relative)
    let (page, html) = web_body("atp_fm", "atp_home");
    serve(&server, "/atp", &page, &html).await;
    serve(&server, "/rss", &feed, &xml).await;
    let r = resolver()
        .resolve(&format!("{}/atp", server.uri()), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.feed_url.as_str(), format!("{}/rss", server.uri()));

    // twit.tv: absolute link to feeds.twit.tv/sn.xml
    let (page, html) = web_body("twit_tv", "twit_security_now");
    let html = html.replace(
        "https://feeds.twit.tv/sn.xml",
        &format!("{}/sn.xml", server.uri()),
    );
    serve(&server, "/shows/security-now", &page, &html).await;
    serve(&server, "/sn.xml", &feed, &xml).await;
    let r = resolver()
        .resolve(
            &format!("{}/shows/security-now", server.uri()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(r.feed_url.as_str(), format!("{}/sn.xml", server.uri()));
    assert_eq!(r.title.as_deref(), Some("How to Start a Podcast"));

    // buzzsprout show page: oembed link must be ignored, rss link used
    let (page, html) = web_body("www_buzzsprout_com", "buzzsprout_show_page");
    let html = html.replace(
        "https://feeds.buzzsprout.com/1.rss",
        &format!("{}/1.rss", server.uri()),
    );
    serve(&server, "/1", &page, &html).await;
    serve(&server, "/1.rss", &feed, &xml).await;
    let r = resolver()
        .resolve(&format!("{}/1", server.uri()), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.feed_url.as_str(), format!("{}/1.rss", server.uri()));
}

#[tokio::test]
async fn resolver_replays_chaosradio_multiple_feed_links() {
    let server = MockServer::start().await;
    let (page, html) = web_body("chaosradio_de", "chaosradio_home");
    let html = html.replace("https://chaosradio.de/", &format!("{}/", server.uri()));
    serve(&server, "/", &page, &html).await;
    let (feed, xml) = web_body("feeds_feedburner_com", "feedburner_feed");
    for p in ["/feed/mp3", "/feed/m4a", "/feed/opus"] {
        serve(&server, p, &feed, &xml).await;
    }
    let r = resolver()
        .resolve(&format!("{}/", server.uri()), CancellationToken::new())
        .await
        .unwrap();
    let chosen = r.feed_url.path();
    assert!(
        ["/feed/mp3", "/feed/m4a", "/feed/opus"].contains(&chosen),
        "one of the three announced feeds, got {chosen}"
    );
    assert_eq!(r.title.as_deref(), Some("Dan Carlin's Hardcore History"));
    let fetches = r
        .provenance
        .iter()
        .filter(|s| s.kind == StepKind::Fetch)
        .count();
    assert_eq!(fetches, 2, "page + first validated feed, no extra requests");
}

#[tokio::test]
async fn resolver_replays_a_site_without_feeds() {
    // gemischteshack.de: no <link rel=alternate>, every path answers HTML 200.
    let server = MockServer::start().await;
    let (page, html) = web_body("www_gemischteshack_de", "gemischteshack_home");
    let ct = page.response.headers.get("content-type").cloned().unwrap();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", ct.as_str())
                .set_body_string(html),
        )
        .mount(&server)
        .await;
    let failure = resolver()
        .resolve(&format!("{}/", server.uri()), CancellationToken::new())
        .await
        .unwrap_err();
    match failure.error {
        ResolveError::NoFeedLinkFound { tried, .. } => {
            assert_eq!(tried.len(), 7, "well-known paths tried once each");
        }
        other => panic!("unexpected {other:?}"),
    }
    let fetches = failure
        .provenance
        .iter()
        .filter(|s| s.kind == StepKind::Fetch)
        .count();
    assert!(fetches <= 8, "request budget respected: {fetches}");
}

#[tokio::test]
async fn a_self_reference_is_no_move() {
    // Real feeds (TWiT, Megaphone, Simplecast, Transistor) repeat their own
    // URL in itunes:new-feed-url; the live run must not report `moved_to`.
    let server = MockServer::start().await;
    let (feed, xml) = web_body("feeds_twit_tv", "twit_feed");
    let own = format!("{}/sn.xml", server.uri());
    assert!(
        xml.contains("<itunes:new-feed-url>https://feeds.twit.tv/sn.xml</itunes:new-feed-url>"),
        "fixture carries the self-referencing tag"
    );
    let xml = xml.replace("https://feeds.twit.tv/sn.xml", &own);
    serve(&server, "/sn.xml", &feed, &xml).await;
    let r = resolver()
        .resolve(&own, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.feed_url.as_str(), own);
    assert_eq!(r.title.as_deref(), Some("Security Now (Audio)"));
    assert_eq!(r.moved_to, None);
    assert!(r.warnings.iter().all(|w| !w.contains("new location")));
}

#[test]
fn recorded_feeds_probe_as_podcasts() {
    for (host, case, title) in [
        ("anchor_fm", "anchor_feed", "Placeholder Radio"),
        (
            "feeds_feedburner_com",
            "feedburner_feed",
            "Dan Carlin's Hardcore History",
        ),
        (
            "rss_buzzsprout_com",
            "buzzsprout_feed",
            "How to Start a Podcast",
        ),
        (
            "feeds_buzzsprout_com",
            "buzzsprout_feed_redirect",
            "How to Start a Podcast",
        ),
        (
            "rss_libsyn_com",
            "libsyn_feed",
            "Dan Carlin's Hardcore History: Addendum",
        ),
    ] {
        let (f, xml) = web_body(host, case);
        let probe = uguisu_feed::probe(xml.as_bytes()).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert!(probe.looks_like_podcast(), "{case}");
        assert_eq!(probe.title.as_deref(), Some(title), "{case}");
        assert!(probe.items_with_enclosure > 0, "{case}");
        assert!(
            f.response
                .headers
                .get("content-type")
                .is_some_and(|ct| ct.contains("xml")),
            "{case}: {:?}",
            f.response.headers
        );
    }
    let (f, _) = web_body("feeds_buzzsprout_com", "buzzsprout_feed_redirect");
    assert!(
        f.notes
            .iter()
            .any(|n| n.contains("followed 1 redirect(s) to https://rss.buzzsprout.com/1.rss"))
    );
}

#[test]
fn every_live_fixture_is_recorded_and_sanitized() {
    let mut count = 0;
    for entry in walkdir(&Fixture::live_root()) {
        let f = Fixture::load_path(&entry).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(f.origin, FixtureOrigin::Recorded, "{}", entry.display());
        assert!(f.source.starts_with("https://"), "{}", entry.display());
        assert!(f.recorded_at.starts_with("2026-"), "{}", entry.display());
        count += 1;
    }
    assert!(count >= 30, "expected the Part B recordings, found {count}");
    let provider_ids: Vec<ProviderId> = [
        ProviderId::APPLE,
        ProviderId::GPODDER_NET,
        ProviderId::PODCAST_INDEX,
    ]
    .into();
    for id in provider_ids {
        assert!(Fixture::live_root().join(id.to_string()).is_dir(), "{id}");
    }
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else if p.extension().is_some_and(|e| e == "json") {
            out.push(p);
        }
    }
    out.sort();
    out
}
