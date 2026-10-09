//! Refresh pipeline: import, change detection, identity rules, conditional
//! requests, fingerprints, failure safety and idempotency (brief §39).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Harness, fixture, live_feed, probe_fixture};
use time::OffsetDateTime;
use uguisu_core::config::FeedConfig;
use uguisu_core::events::EventKind;
use uguisu_core::feed::{FetchErrorKind, NotModifiedReason, RefreshOutcome};
use uguisu_core::ids::{PodcastId, SourceId};
use uguisu_core::model::{
    ArchiveState, FetchState, FetchStatus, IdentitySource, Podcast, PodcastSource, PodcastStatus,
};
use uguisu_engine::RefreshOptions;
use uguisu_storage::{changes, podcasts, sources};
use url::Url;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn kinds(events: &[uguisu_core::Event]) -> Vec<&'static str> {
    events.iter().map(uguisu_core::Event::name).collect()
}

fn drain(sub: &mut uguisu_engine::Subscription) -> Vec<uguisu_core::Event> {
    let mut out = Vec::new();
    while let Some(e) = sub.try_recv() {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn a_first_import_stores_and_emits() {
    let h = Harness::new().await;
    let mut sub = h.engine.subscribe();
    let added = h.add_fixture("episodes_v1.xml").await;
    let report = added.report.clone().unwrap();
    assert_eq!(report.outcome, RefreshOutcome::Fetched);
    assert_eq!(report.episodes.added, 3);
    assert_eq!(report.episodes.updated, 0);
    assert_eq!(report.episodes.unchanged, 0);
    assert_eq!(report.episodes.seen, 3);
    assert!(!report.http.conditional);
    assert!(
        report
            .podcast_changed_fields
            .contains(&"description_html".to_owned()),
        "{:?}",
        report.podcast_changed_fields
    );
    assert!(report.removal_suppressed.is_none());

    let page = h.engine.episodes(added.podcast.id, None, 10).await.unwrap();
    assert_eq!(page.episodes.len(), 3);
    assert_eq!(page.episodes[0].title, "Episode Three", "newest first");
    let one = page
        .episodes
        .iter()
        .find(|e| e.title == "Episode One")
        .unwrap();
    assert_eq!(one.identity.key, "guid:v-1");
    assert_eq!(one.identity.source, IdentitySource::Guid);
    assert_eq!(one.duration_secs, Some(600));
    assert_eq!(one.episode_number, Some(1));
    assert_eq!(one.enclosures.len(), 1);
    assert_eq!(one.enclosures[0].length_bytes, Some(1000));
    assert_eq!(one.archive_state, ArchiveState::Expected);

    let shown = h.engine.podcast(added.podcast.id).await.unwrap();
    assert_eq!(shown.podcast.title, "Versioned Show");
    assert_eq!(shown.podcast.author.as_deref(), Some("V Host"));
    assert_eq!(shown.episodes_total, 3);
    let src = shown.source.unwrap();
    assert_eq!(src.fetch.state, FetchState::Fetched);
    assert!(src.fetch.content_fingerprint.is_some());
    assert_eq!(src.fetch.consecutive_failures, 0);
    assert_eq!(shown.last_fetch.unwrap().id, report.fetch_id);

    let events = drain(&mut sub);
    let k = kinds(&events);
    assert_eq!(k[0], "podcast.added");
    assert_eq!(k[1], "podcast.feed.refresh.started");
    assert_eq!(k.iter().filter(|k| **k == "episode.discovered").count(), 3);
    assert_eq!(
        k.iter()
            .filter(|k| **k == "podcast.metadata.updated")
            .count(),
        1
    );
    assert_eq!(*k.last().unwrap(), "podcast.feed.refresh.completed");
    let discovered = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::EpisodeDiscovered { .. }))
        .unwrap();
    assert!(discovered.episode_id.is_some());
    if let EventKind::EpisodeDiscovered { enclosure_url, .. } = &discovered.kind {
        assert!(enclosure_url.is_some());
    }
}

#[tokio::test]
async fn an_identical_refresh_is_not_modified() {
    let h = Harness::new().await;
    h.serve(
        "/feed.xml",
        &fixture("episodes_v1.xml"),
        Some("\"v1\""),
        Some("Mon, 01 Sep 2025 10:00:00 GMT"),
    )
    .await;
    let added = h
        .engine
        .add_podcast(&h.url("/feed.xml"), uguisu_http::CancellationToken::new())
        .await
        .unwrap();
    let id = added.podcast.id;
    let src = h.engine.podcast(id).await.unwrap().source.unwrap();
    assert_eq!(src.fetch.etag.as_deref(), Some("\"v1\""));
    assert_eq!(
        src.fetch.last_modified.as_deref(),
        Some("Mon, 01 Sep 2025 10:00:00 GMT")
    );

    // The server honours the validators.
    h.reset().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .and(header("if-none-match", "\"v1\""))
        .and(header_exists("if-modified-since"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&h.server)
        .await;
    let mut sub = h.engine.subscribe();
    let r = h.refresh(id, false).await;
    let reqs = h.server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0].headers.get("if-modified-since").unwrap(),
        "Mon, 01 Sep 2025 10:00:00 GMT"
    );
    assert_eq!(
        r.outcome,
        RefreshOutcome::NotModified {
            reason: NotModifiedReason::Http304
        }
    );
    assert!(r.http.conditional);
    assert_eq!(r.episodes.added, 0);
    let k = kinds(&drain(&mut sub));
    assert_eq!(
        k,
        vec!["podcast.feed.refresh.started", "podcast.feed.not_modified"]
    );
    let src = h.engine.podcast(id).await.unwrap().source.unwrap();
    assert_eq!(src.fetch.state, FetchState::NotModified);
    assert!(src.fetch.last_not_modified_at.is_some());
    assert_eq!(src.fetch.etag.as_deref(), Some("\"v1\""), "validators kept");

    // The server ignores validators but sends the same body: fingerprint hit.
    h.reset().await;
    h.serve("/feed.xml", &fixture("episodes_v1.xml"), None, None)
        .await;
    let r = h.refresh(id, false).await;
    assert_eq!(
        r.outcome,
        RefreshOutcome::NotModified {
            reason: NotModifiedReason::Fingerprint
        }
    );
    assert_eq!(
        h.engine
            .episodes(id, None, 10)
            .await
            .unwrap()
            .episodes
            .len(),
        3
    );

    // Forced: parsed again, nothing changed, no episode events.
    let r = h.refresh(id, true).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert!(!r.http.conditional);
    assert_eq!(r.episodes.unchanged, 3);
    assert_eq!(r.episodes.added + r.episodes.updated, 0);
    assert!(r.podcast_changed_fields.is_empty());
    let k = kinds(&drain(&mut sub));
    assert!(!k.iter().any(|k| k.starts_with("episode.")), "{k:?}");
    let mut conn = h.engine.storage().reader().await.unwrap();
    assert!(
        changes::list_for_podcast(&mut conn, id, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(h.engine.fetch_log(id, 10).await.unwrap().len(), 4);
}

#[tokio::test]
async fn a_changed_feed_adds_and_removes() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let before = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    let one_id = before.iter().find(|e| e.title == "Episode One").unwrap().id;
    let three_id = before
        .iter()
        .find(|e| e.title == "Episode Three")
        .unwrap()
        .id;
    let one_enclosure_id = before
        .iter()
        .find(|e| e.title == "Episode Two")
        .unwrap()
        .enclosures[0]
        .id;

    h.reset().await;
    h.serve_fixture("/feed.xml", "episodes_v2.xml").await;
    let mut sub = h.engine.subscribe();
    let r = h.refresh(id, false).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert_eq!(r.episodes.added, 1);
    assert_eq!(r.episodes.updated, 1);
    assert_eq!(r.episodes.unchanged, 1);
    assert_eq!(r.episodes.removed_detected, 0, "streak 1 is not removal");
    assert_eq!(
        r.podcast_changed_fields,
        vec!["description_html", "description_text", "artwork_url"]
    );

    let after = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    assert_eq!(after.len(), 4);
    let one = after.iter().find(|e| e.id == one_id).unwrap();
    assert_eq!(one.title, "Episode One (remastered)");
    assert_eq!(one.enclosures[0].length_bytes, Some(1100));
    assert_eq!(one.extras.chapters.len(), 1);
    let three = after.iter().find(|e| e.id == three_id).unwrap();
    assert_eq!(three.missing_streak, 1);
    assert!(three.removed_from_feed_at.is_none());
    let two = after.iter().find(|e| e.title == "Episode Two").unwrap();
    assert_eq!(
        two.enclosures[0].id, one_enclosure_id,
        "enclosure ids are stable"
    );
    assert_eq!(two.missing_streak, 0);

    let mut conn = h.engine.storage().reader().await.unwrap();
    let log = changes::list_for_episode(&mut conn, one_id).await.unwrap();
    let fields: Vec<&str> = log.iter().map(|c| c.field.as_str()).collect();
    for f in ["title", "description_html", "enclosures", "extras"] {
        assert!(fields.contains(&f), "{fields:?} lacks {f}");
    }
    assert_eq!(
        log.iter()
            .find(|c| c.field == "title")
            .unwrap()
            .old_value
            .as_deref(),
        Some("Episode One")
    );
    let events = drain(&mut sub);
    let updated = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::EpisodeUpdated { .. }))
        .unwrap();
    assert_eq!(updated.episode_id, Some(one_id));
    assert_eq!(
        kinds(&events)
            .iter()
            .filter(|k| **k == "episode.discovered")
            .count(),
        1
    );
    assert!(!kinds(&events).contains(&"episode.removal_detected"));

    // Second complete fetch without Three: streak 2 → removal detected.
    let r = h.refresh(id, true).await;
    assert_eq!(r.episodes.removed_detected, 1);
    assert_eq!(r.episodes.unchanged, 3);
    let three = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    let three = three.iter().find(|e| e.id == three_id).unwrap();
    assert_eq!(three.missing_streak, 2);
    assert!(three.removed_from_feed_at.is_some());
    let events = drain(&mut sub);
    let removed = events
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                EventKind::EpisodeRemovalDetected { missing_streak: 2 }
            )
        })
        .unwrap();
    assert_eq!(removed.episode_id, Some(three_id));
    let detail = h.engine.podcast(id).await.unwrap();
    assert_eq!((detail.episodes_total, detail.episodes_present), (4, 3));

    // It comes back: streak reset, removal cleared, id kept.
    h.reset().await;
    h.serve_fixture("/feed.xml", "episodes_v1.xml").await;
    let r = h.refresh(id, false).await;
    assert_eq!(r.episodes.updated, 1, "One reverts");
    let back = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    let three = back.iter().find(|e| e.id == three_id).unwrap();
    assert_eq!(three.missing_streak, 0);
    assert!(three.removed_from_feed_at.is_none());
}

#[tokio::test]
async fn guid_change_follows_adr_0014_both_branches() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let before = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    let one_id = before.iter().find(|e| e.title == "Episode One").unwrap().id;
    let two_id = before.iter().find(|e| e.title == "Episode Two").unwrap().id;

    h.reset().await;
    h.serve_fixture("/feed.xml", "guid_changed.xml").await;
    let mut sub = h.engine.subscribe();
    let r = h.refresh(id, false).await;
    assert_eq!(r.episodes.ambiguous, 1, "{:?}", r.warnings);
    assert_eq!(r.episodes.added, 1);
    assert_eq!(r.episodes.updated, 1);
    assert_eq!(r.episodes.unchanged, 1);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("matched stored episode")),
        "{:?}",
        r.warnings
    );

    let after = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    assert_eq!(after.len(), 4);
    // Branch 1: both GUIDs present and different → candidate duplicate, never merged.
    let candidate = after
        .iter()
        .find(|e| e.duplicate_of_episode_id == Some(one_id))
        .expect("candidate duplicate of One");
    assert_eq!(candidate.archive_state, ArchiveState::Skipped);
    assert_eq!(
        candidate.skip_reason.as_deref(),
        Some(format!("duplicate of {one_id}").as_str())
    );
    assert!(
        candidate
            .duplicate_reasons
            .contains(&"same_enclosure_url".to_owned())
    );
    assert!(
        candidate
            .duplicate_reasons
            .contains(&"same_title".to_owned())
    );
    let one = after.iter().find(|e| e.id == one_id).unwrap();
    assert_eq!(one.identity.key, "guid:v-1", "stored episode untouched");
    assert_eq!(
        one.missing_streak, 1,
        "the old One is absent from this fetch"
    );
    // Branch 2: GUID vanished, same enclosure → same episode, key kept.
    let two = after.iter().find(|e| e.id == two_id).unwrap();
    assert_eq!(two.identity.key, "guid:v-2");
    assert_eq!(two.identity.guid_key, None);
    assert!(two.identity.enclosure_key.is_some());
    assert!(two.identity.reason.contains("lost its guid"));
    let mut conn = h.engine.storage().reader().await.unwrap();
    let log = changes::list_for_episode(&mut conn, two_id).await.unwrap();
    assert!(log.iter().any(|c| c.field == "identity_signals"), "{log:?}");

    let events = drain(&mut sub);
    let amb = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::EpisodeIdentityAmbiguous { .. }))
        .unwrap();
    assert_eq!(amb.episode_id, Some(candidate.id));
    if let EventKind::EpisodeIdentityAmbiguous { duplicate_of, .. } = &amb.kind {
        assert_eq!(*duplicate_of, one_id);
    }
    // Re-importing the same feed is stable: nothing new, nothing ambiguous.
    let r = h.refresh(id, true).await;
    assert_eq!((r.episodes.added, r.episodes.ambiguous), (0, 0));
    assert_eq!(r.episodes.unchanged, 3);
}

#[tokio::test]
async fn candidate_keeps_reasons_on_update() {
    let h = Harness::new().await;
    let id = h.add_fixture("episodes_v1.xml").await.podcast.id;
    h.reset().await;
    h.serve_fixture("/feed.xml", "guid_changed.xml").await;
    assert_eq!(h.refresh(id, false).await.episodes.ambiguous, 1);

    let changed = String::from_utf8(fixture("guid_changed.xml"))
        .unwrap()
        .replace("First description", "First description, corrected");
    h.reset().await;
    h.serve("/feed.xml", changed.as_bytes(), None, None).await;
    let r = h.refresh(id, true).await;
    assert_eq!((r.episodes.added, r.episodes.updated), (0, 1), "{r:?}");
    let episodes = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    let candidate = episodes
        .iter()
        .find(|e| e.duplicate_of_episode_id.is_some())
        .expect("still a candidate");
    assert_eq!(
        candidate.description_text.as_deref(),
        Some("First description, corrected")
    );
    assert!(
        candidate
            .duplicate_reasons
            .contains(&"same_enclosure_url".to_owned()),
        "{:?}",
        candidate.duplicate_reasons
    );
}

#[tokio::test]
async fn malformed_missing_and_duplicate_items_are_handled() {
    let h = Harness::new().await;
    let added = h.add_fixture("malformed_item.xml").await;
    let r = added.report.clone().unwrap();
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert_eq!(r.episodes.malformed, 1);
    assert_eq!(r.episodes.added, 2);
    assert!(
        r.removal_suppressed
            .as_deref()
            .unwrap()
            .contains("malformed")
    );
    let eps = h
        .engine
        .episodes(added.podcast.id, None, 10)
        .await
        .unwrap()
        .episodes;
    assert_eq!(eps.len(), 3, "the malformed item is kept as a placeholder");
    let bad = eps.iter().find(|e| e.malformed).unwrap();
    assert_eq!(bad.identity.key, "guid:bad");
    assert_eq!(bad.archive_state, ArchiveState::Skipped);
    assert!(bad.malformed_reason.is_some());

    let h = Harness::new().await;
    let added = h.add_fixture("missing_guid.xml").await;
    let r = added.report.clone().unwrap();
    assert_eq!(r.episodes.added, 3);
    let eps = h
        .engine
        .episodes(added.podcast.id, None, 10)
        .await
        .unwrap()
        .episodes;
    let first = eps.iter().find(|e| e.title == "First").unwrap();
    assert_eq!(first.identity.source, IdentitySource::EnclosureUrl);
    assert_eq!(first.identity.key, "url:cdn.example/first.mp3");
    let third = eps
        .iter()
        .find(|e| e.title == "Third without enclosure")
        .unwrap();
    assert_eq!(third.identity.source, IdentitySource::Fingerprint);
    assert!(third.enclosures.is_empty());
    let r = h.refresh(added.podcast.id, true).await;
    assert_eq!(r.episodes.unchanged, 3);

    let h = Harness::new().await;
    let added = h.add_fixture("duplicate_episodes.xml").await;
    let r = added.report.clone().unwrap();
    assert_eq!(r.episodes.seen, 3);
    assert_eq!(r.episodes.added, 2, "identical items collapse");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("identical to an earlier item"))
    );
    assert_eq!(
        h.engine
            .episodes(added.podcast.id, None, 10)
            .await
            .unwrap()
            .episodes
            .len(),
        2
    );
}

/// Points an `itunes:new-feed-url` at `served`: verifying the announcement
/// would otherwise fetch the real host, and tests never use the network.
fn announce_self(body: Vec<u8>, served: &str) -> Vec<u8> {
    const OPEN: &[u8] = b"<itunes:new-feed-url>";
    let Some(start) = body.windows(OPEN.len()).position(|w| w == OPEN) else {
        return body;
    };
    let start = start + OPEN.len();
    let end = start + body[start..].iter().position(|&b| b == b'<').unwrap();
    [&body[..start], served.as_bytes(), &body[end..]].concat()
}

#[tokio::test]
async fn atom_rdf_probe_and_live_feeds_ingest() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("minimal_atom", fixture("minimal_atom.xml")),
        ("rss_rdf", probe_fixture("rss_rdf.xml")),
        (
            "atom_with_enclosures",
            probe_fixture("atom_with_enclosures.xml"),
        ),
        (
            "rss_alternate_enclosure_only",
            probe_fixture("rss_alternate_enclosure_only.xml"),
        ),
        (
            "rss_entities_and_cdata",
            probe_fixture("rss_entities_and_cdata.xml"),
        ),
        (
            "rss_itunes_podcast",
            probe_fixture("rss_itunes_podcast.xml"),
        ),
        ("rss_latin1", probe_fixture("rss_latin1.xml")),
        ("podcasting20", fixture("podcasting20.xml")),
        ("dates", fixture("dates.xml")),
        ("enclosures", fixture("enclosures.xml")),
        ("durations", fixture("durations.xml")),
        ("unicode_heavy", fixture("unicode_heavy.xml")),
        ("very_long_title", fixture("very_long_title.xml")),
        ("html_cdata_empty", fixture("html_cdata_empty.xml")),
        ("odd_prefixes", fixture("odd_prefixes.xml")),
        ("new_feed_url", fixture("new_feed_url.xml")),
        ("anchor", live_feed("anchor_fm", "anchor_feed")),
        (
            "buzzsprout",
            live_feed("rss_buzzsprout_com", "buzzsprout_feed"),
        ),
        (
            "feedburner",
            live_feed("feeds_feedburner_com", "feedburner_feed"),
        ),
        ("twit", live_feed("feeds_twit_tv", "twit_feed")),
        ("libsyn", live_feed("rss_libsyn_com", "libsyn_feed")),
    ];
    let mut podcasts_seen = 0;
    for (name, body) in cases {
        // One engine per feed: some fixtures share a podcast:guid on purpose.
        let h = Harness::new().await;
        let p = format!("/{name}.xml");
        let body = announce_self(body, &h.url(&p));
        h.serve(&p, &body, None, None).await;
        let added = h
            .engine
            .add_podcast(&h.url(&p), uguisu_http::CancellationToken::new())
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let r = added.report.unwrap();
        assert_eq!(
            r.outcome,
            RefreshOutcome::Fetched,
            "{name}: {:?}",
            r.warnings
        );
        assert!(r.episodes.added >= 1, "{name}");
        // Idempotent: a forced second pass changes nothing.
        let again = h.refresh(added.podcast.id, true).await;
        assert_eq!(again.outcome, RefreshOutcome::Fetched, "{name}");
        assert_eq!(again.episodes.added, 0, "{name}: {:?}", again.warnings);
        assert_eq!(again.episodes.updated, 0, "{name}: {:?}", again.warnings);
        assert_eq!(again.episodes.unchanged, r.episodes.added, "{name}");
        assert!(
            again.podcast_changed_fields.is_empty(),
            "{name}: {:?}",
            again.podcast_changed_fields
        );
        let eps = h
            .engine
            .episodes(added.podcast.id, None, 500)
            .await
            .unwrap();
        assert_eq!(
            eps.episodes.len(),
            usize::try_from(r.episodes.added).unwrap(),
            "{name}"
        );
        podcasts_seen += 1;
    }
    assert_eq!(podcasts_seen, 21);
}

#[tokio::test]
async fn unverifiable_new_feed_url_is_reported_only() {
    let h = Harness::new().await;
    let added = h.add_fixture("new_feed_url.xml").await;
    let r = added.report.unwrap();
    match &r.feed_url {
        uguisu_core::feed::FeedUrlStatus::ChangeDetected { announced, reason } => {
            assert_eq!(announced.host_str(), Some("new.example"));
            assert!(reason.contains("dns_error"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    let log = h.engine.fetch_log(added.podcast.id, 1).await.unwrap();
    assert!(log[0].url_change_detected);
    assert_eq!(h.engine.sources(added.podcast.id).await.unwrap().len(), 1);
}

async fn insert_podcast_with_url(h: &Harness, url: &str) -> (PodcastId, SourceId) {
    let now = OffsetDateTime::now_utc();
    let podcast = Podcast {
        id: PodcastId::new(),
        title: "Direct".into(),
        sort_title: "direct".into(),
        subtitle: None,
        author: None,
        publisher: None,
        owner_name: None,
        owner_email: None,
        description_html: None,
        description_text: None,
        website: None,
        artwork_url: None,
        language: None,
        categories: vec![],
        explicit: None,
        copyright: None,
        podcast_guid: None,
        feed_kind: uguisu_core::model::FeedKind::Rss2,
        status: PodcastStatus::Active,
        refresh_interval_secs: None,
        next_refresh_at: None,
        last_refresh_at: None,
        last_error: None,
        directory_name: None,
        metadata_hash: String::new(),
        created_at: now,
        updated_at: now,
    };
    let source = PodcastSource {
        id: SourceId::new(),
        podcast_id: podcast.id,
        feed_url: Url::parse(url).unwrap(),
        canonical_url: None,
        website_url: None,
        provider: "test".into(),
        provider_ref: None,
        discovered_at: now,
        verified_at: None,
        is_current: true,
        replaced_by_source_id: None,
        replacement_reason: None,
        fetch: FetchStatus::default(),
        created_at: now,
        updated_at: now,
    };
    let mut tx = h.engine.storage().begin().await.unwrap();
    podcasts::insert(&mut tx, &podcast).await.unwrap();
    sources::insert(&mut tx, &source).await.unwrap();
    tx.commit().await.unwrap();
    (podcast.id, source.id)
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one table of failure kinds
async fn every_failure_kind_leaves_state_intact() {
    let h = Harness::with_feed_config(FeedConfig {
        refresh_timeout: Duration::from_secs(3),
        error_after_failures: 3,
        ..FeedConfig::default()
    })
    .await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let source_id = added.source.id;
    let snapshot = h.engine.episodes(id, None, 10).await.unwrap();
    let title = h.engine.podcast(id).await.unwrap().podcast.title;

    let cases: Vec<(&str, ResponseTemplate, FetchErrorKind)> = vec![
        (
            "500",
            ResponseTemplate::new(500),
            FetchErrorKind::HttpServerError,
        ),
        ("404", ResponseTemplate::new(404), FetchErrorKind::NotFound),
        ("410", ResponseTemplate::new(410), FetchErrorKind::NotFound),
        (
            "429",
            ResponseTemplate::new(429),
            FetchErrorKind::RateLimited,
        ),
        (
            "401",
            ResponseTemplate::new(401),
            FetchErrorKind::Unauthorized,
        ),
        ("403", ResponseTemplate::new(403), FetchErrorKind::Forbidden),
        (
            "418",
            ResponseTemplate::new(418),
            FetchErrorKind::HttpClientError,
        ),
        (
            "html",
            ResponseTemplate::new(200)
                .set_body_string("<!DOCTYPE html><html><body>nope</body></html>")
                .insert_header("content-type", "text/html"),
            FetchErrorKind::InvalidContentType,
        ),
        (
            "json",
            ResponseTemplate::new(200).set_body_string("{\"a\":1}"),
            FetchErrorKind::InvalidContentType,
        ),
        (
            "malformed",
            ResponseTemplate::new(200).set_body_bytes(probe_fixture("malformed.xml")),
            FetchErrorKind::MalformedXml,
        ),
        (
            "no_enclosures",
            ResponseTemplate::new(200).set_body_bytes(probe_fixture("rss_no_enclosures.xml")),
            FetchErrorKind::InvalidPodcastFeed,
        ),
        (
            "empty",
            ResponseTemplate::new(200).set_body_string(""),
            FetchErrorKind::InvalidPodcastFeed,
        ),
        (
            "unknown_root",
            ResponseTemplate::new(200).set_body_string("<?xml version=\"1.0\"?><opml/>"),
            FetchErrorKind::UnsupportedFeed,
        ),
        (
            "slow",
            ResponseTemplate::new(200)
                .set_body_bytes(fixture("episodes_v1.xml"))
                .set_delay(Duration::from_secs(4)),
            FetchErrorKind::Timeout,
        ),
    ];
    let mut failures = 0;
    for (name, template, expected) in cases {
        h.reset().await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(template)
            .mount(&h.server)
            .await;
        let r = h.refresh(id, false).await;
        failures += 1;
        match &r.outcome {
            RefreshOutcome::Failed { kind, detail } => {
                assert_eq!(*kind, expected, "{name}: {detail}");
            }
            other => panic!("{name}: expected failure, got {other:?}"),
        }
        let (src, last) = h.engine.source(source_id).await.unwrap();
        assert_eq!(src.fetch.state, FetchState::Failed, "{name}");
        assert_eq!(src.fetch.last_error_kind, Some(expected), "{name}");
        assert_eq!(src.fetch.consecutive_failures, failures, "{name}");
        assert!(
            src.fetch.content_fingerprint.is_some(),
            "{name}: fingerprint kept"
        );
        assert_eq!(last.unwrap().id, r.fetch_id, "{name}");
        let after = h.engine.episodes(id, None, 10).await.unwrap();
        assert_eq!(after, snapshot, "{name}: episodes untouched");
        let p = h.engine.podcast(id).await.unwrap().podcast;
        assert_eq!(p.title, title, "{name}");
        assert!(
            p.last_error
                .as_deref()
                .unwrap()
                .starts_with(expected.as_str()),
            "{name}"
        );
        if failures >= 3 {
            assert_eq!(p.status, PodcastStatus::Error, "{name}");
        } else {
            assert_eq!(p.status, PodcastStatus::Active, "{name}");
        }
    }

    // Too large: the body cap is the parser cap.
    h.reset().await;
    h.serve_fixture("/feed.xml", "episodes_v1.xml").await;
    let small = Harness::with_feed_config(FeedConfig {
        limits: uguisu_core::config::FeedLimits {
            max_bytes: 200,
            ..uguisu_core::config::FeedLimits::default()
        },
        ..FeedConfig::default()
    })
    .await;
    small.serve_fixture("/feed.xml", "episodes_v1.xml").await;
    let (pid, _) = insert_podcast_with_url(&small, &small.url("/feed.xml")).await;
    let r = small.refresh(pid, false).await;
    assert!(
        matches!(
            r.outcome,
            RefreshOutcome::Failed {
                kind: FetchErrorKind::TooLarge,
                ..
            }
        ),
        "{:?}",
        r.outcome
    );

    // Blocked by policy and DNS failures.
    let (pid, _) = insert_podcast_with_url(&h, "http://10.0.0.1/feed.xml").await;
    let r = h.refresh(pid, false).await;
    assert!(
        matches!(
            r.outcome,
            RefreshOutcome::Failed {
                kind: FetchErrorKind::BlockedByPolicy,
                ..
            }
        ),
        "{:?}",
        r.outcome
    );
    let (pid, _) = insert_podcast_with_url(&h, "http://feed.invalid/feed.xml").await;
    let r = h.refresh(pid, false).await;
    assert!(
        matches!(
            r.outcome,
            RefreshOutcome::Failed {
                kind: FetchErrorKind::DnsError,
                ..
            }
        ),
        "{:?}",
        r.outcome
    );

    // Cancellation before the fetch starts.
    let cancel = uguisu_http::CancellationToken::new();
    cancel.cancel();
    let r = h
        .engine
        .refresh_podcast(
            id,
            RefreshOptions {
                force: true,
                cancel: Some(cancel),
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(
            r.outcome,
            RefreshOutcome::Failed {
                kind: FetchErrorKind::Cancelled,
                ..
            }
        ),
        "{:?}",
        r.outcome
    );

    // Recovery: one good fetch clears the error state. The body is the one
    // whose fingerprint survived the failures, so this is a fingerprint hit.
    let r = h.refresh(id, false).await;
    assert_eq!(
        r.outcome,
        RefreshOutcome::NotModified {
            reason: NotModifiedReason::Fingerprint
        },
        "{:?}",
        r.warnings
    );
    let detail = h.engine.podcast(id).await.unwrap();
    assert_eq!(detail.podcast.status, PodcastStatus::Active);
    assert!(detail.podcast.last_error.is_none());
    let src = detail.source.unwrap();
    assert_eq!(src.fetch.consecutive_failures, 0);
    assert_eq!(src.fetch.state, FetchState::NotModified);
    assert!(src.fetch.last_error_at.is_some(), "history kept");
    let r = h.refresh(id, true).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert_eq!(r.episodes.unchanged, 3);
}

#[tokio::test]
async fn refresh_of_unknown_podcast_is_not_found() {
    let h = Harness::new().await;
    let err = h
        .engine
        .refresh_podcast(PodcastId::new(), RefreshOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, uguisu_core::UguisuError::NotFound { .. }));
}

#[tokio::test]
async fn refresh_survives_a_temporary_move() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/moved.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("episodes_v1.xml")))
        .mount(&other)
        .await;
    h.reset().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/moved.xml", other.uri())),
        )
        .mount(&h.server)
        .await;
    let r = h.refresh(added.podcast.id, true).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert_eq!(r.http.redirects, 1);
    assert_eq!(r.feed_url, uguisu_core::feed::FeedUrlStatus::Unchanged);
    assert_eq!(
        h.engine
            .podcast(added.podcast.id)
            .await
            .unwrap()
            .source
            .unwrap()
            .feed_url,
        added.source.feed_url
    );
}

#[tokio::test]
async fn markup_title_updates_without_duplicates() {
    let h = Harness::new().await;
    let body = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Markup</title>
<item><title><![CDATA[<br>]]></title><guid>ep-1</guid><pubDate>Mon, 01 Sep 2025 12:00:00 +0000</pubDate><enclosure url="https://cdn.example/one.mp3" type="audio/mpeg" length="10"/></item>
<item><title><![CDATA[<b></b>]]></title><pubDate>Tue, 02 Sep 2025 12:00:00 +0000</pubDate><enclosure url="https://cdn.example/two.mp3" type="audio/mpeg" length="20"/></item>
<item><title><![CDATA[<i></i>]]></title><pubDate>Wed, 03 Sep 2025 12:00:00 +0000</pubDate></item>
</channel></rss>"#;
    Mock::given(method("GET"))
        .and(path("/markup.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(body),
        )
        .mount(&h.server)
        .await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/markup.xml"), uguisu_http::CancellationToken::new())
        .await
        .unwrap()
        .podcast;
    let listed = |h: &Harness| {
        let engine = h.engine.clone();
        async move {
            let mut eps = engine
                .episodes(podcast.id, None, 10)
                .await
                .unwrap()
                .episodes;
            eps.sort_by_key(|e| e.id);
            eps
        }
    };
    let first = listed(&h).await;
    let mut titles: Vec<&str> = first.iter().map(|e| e.title.as_str()).collect();
    titles.sort_unstable();
    assert_eq!(titles, ["Untitled episode", "one.mp3", "two.mp3"]);
    // The key a build before the fix stored for the item with neither a GUID
    // nor an enclosure; a different fingerprint input would add a row below.
    let untitled = first
        .iter()
        .find(|e| e.title == "Untitled episode")
        .unwrap();
    assert_eq!(
        untitled.identity.key,
        format!(
            "fp:{}",
            uguisu_feed::identity::fingerprint(
                "",
                Some(time::macros::datetime!(2025-09-03 12:00 UTC)),
                None
            )
        )
    );

    // The rows a build before the fix stored: the same identities, empty titles.
    {
        let stale: Vec<_> = first
            .iter()
            .cloned()
            .map(|mut e| {
                e.title = String::new();
                e.content_hash = "stale".to_owned();
                e
            })
            .collect();
        let mut tx = h.engine.storage().begin().await.unwrap();
        uguisu_storage::episodes::upsert_all(&mut tx, &stale)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let r = h.refresh(podcast.id, true).await;
    assert_eq!(r.episodes.added, 0, "{r:?}");
    let after = listed(&h).await;
    assert_eq!(
        after
            .iter()
            .map(|e| (e.id, &e.identity.key))
            .collect::<Vec<_>>(),
        first
            .iter()
            .map(|e| (e.id, &e.identity.key))
            .collect::<Vec<_>>(),
        "no episode is replaced or duplicated"
    );
    assert!(after.iter().all(|e| !e.title.is_empty()), "{after:?}");
}
