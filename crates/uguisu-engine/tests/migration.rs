//! Feed URL migration: verified moves switch the source atomically,
//! unverified announcements are only reported, and redirects to another
//! show never merge into this podcast.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, fixture, live_feed};
use uguisu_core::UguisuError;
use uguisu_core::events::EventKind;
use uguisu_core::feed::{FeedUrlStatus, FetchErrorKind, RefreshOutcome};
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_core::model::{FetchState, ReplacementReason};
use uguisu_engine::migration::{FeedMove, MoveOptions};
use uguisu_http::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// `episodes_v1.xml` announcing `new_url` as its new home.
fn v1_announcing(new_url: &str) -> Vec<u8> {
    let s = String::from_utf8(fixture("episodes_v1.xml")).unwrap();
    s.replacen(
        "<itunes:author>V Host</itunes:author>",
        &format!("<itunes:author>V Host</itunes:author><itunes:new-feed-url>{new_url}</itunes:new-feed-url>"),
        1,
    )
    .into_bytes()
}

#[tokio::test]
async fn a_verified_new_url_keeps_episodes() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let old_source = added.source.id;
    let before = h.engine.episodes(id, None, 10).await.unwrap().episodes;

    // The old URL now announces the new one; the new one serves v2.
    let new_url = h.url("/new.xml");
    h.reset().await;
    h.serve("/feed.xml", &v1_announcing(&new_url), None, None)
        .await;
    h.serve(
        "/new.xml",
        &fixture("episodes_v2.xml"),
        Some("\"n1\""),
        None,
    )
    .await;
    let mut sub = h.engine.subscribe();
    let r = h.refresh(id, true).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    match &r.feed_url {
        FeedUrlStatus::Changed { from, to, via } => {
            assert_eq!(from.as_str(), h.url("/feed.xml"));
            assert_eq!(to.as_str(), new_url);
            assert_eq!(*via, ReplacementReason::NewFeedUrl);
        }
        other => panic!("{other:?}"),
    }
    assert!(r.warnings.iter().any(|w| w.contains("feed url changed")));

    let detail = h.engine.podcast(id).await.unwrap();
    let current = detail.source.unwrap();
    assert_ne!(current.id, old_source);
    assert_eq!(current.feed_url.as_str(), new_url);
    assert_eq!(current.fetch.state, FetchState::Fetched);
    assert!(
        current.fetch.etag.is_none() && current.fetch.content_fingerprint.is_none(),
        "the verification fetch did not sync items, so nothing is marked seen"
    );
    let history = h.engine.sources(id).await.unwrap();
    assert_eq!(history.len(), 2);
    let old = history.iter().find(|s| s.id == old_source).unwrap();
    assert!(!old.is_current);
    assert_eq!(old.replaced_by_source_id, Some(current.id));
    assert_eq!(old.replacement_reason, Some(ReplacementReason::NewFeedUrl));

    // Episodes keep their ids (the v1 items were synced from the old URL).
    let after = h.engine.episodes(id, None, 10).await.unwrap().episodes;
    for e in &before {
        assert!(after.iter().any(|a| a.id == e.id), "{} kept", e.title);
    }
    let events = drain(&mut sub);
    let changed = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::FeedUrlChanged { .. }))
        .expect("feed.url.changed");
    if let EventKind::FeedUrlChanged {
        from_source_id,
        to_source_id,
        ..
    } = &changed.kind
    {
        assert_eq!(*from_source_id, old_source);
        assert_eq!(*to_source_id, current.id);
    }

    // The next refresh fetches the new URL in full and syncs v2 from there.
    h.reset().await;
    Mock::given(method("GET"))
        .and(path("/new.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fixture("episodes_v2.xml"))
                .insert_header("etag", "\"n1\""),
        )
        .expect(2)
        .mount(&h.server)
        .await;
    let r = h.refresh(id, false).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert_eq!(r.source_id, current.id);
    assert!(!r.http.conditional);
    assert_eq!(r.feed_url, FeedUrlStatus::Unchanged);
    assert_eq!(r.episodes.added, 1, "Episode Four arrives from the new URL");
    assert_eq!(r.episodes.updated, 1, "Episode One (remastered)");
    // From here on the new source is refreshed conditionally.
    let r = h.refresh(id, false).await;
    assert!(r.http.conditional);
    assert!(matches!(r.outcome, RefreshOutcome::NotModified { .. }));
}

#[tokio::test]
async fn another_shows_feed_is_not_adopted() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let new_url = h.url("/other.xml");
    h.reset().await;
    h.serve("/feed.xml", &v1_announcing(&new_url), None, None)
        .await;
    h.serve_fixture("/other.xml", "podcasting20.xml").await;
    let mut sub = h.engine.subscribe();
    let r = h.refresh(id, true).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    match &r.feed_url {
        FeedUrlStatus::ChangeDetected { announced, reason } => {
            assert_eq!(announced.as_str(), new_url);
            assert!(reason.contains("title differs"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    let current = h.engine.podcast(id).await.unwrap().source.unwrap();
    assert_eq!(current.id, added.source.id, "source unchanged");
    assert_eq!(h.engine.sources(id).await.unwrap().len(), 1);
    let events = drain(&mut sub);
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::FeedUrlChangeDetected { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::FeedUrlChanged { .. }))
    );
    assert_eq!(
        r.episodes.unchanged, 3,
        "the other show's items were not merged"
    );

    // The announced feed answering 404 is reported the same way.
    h.reset().await;
    h.serve("/feed.xml", &v1_announcing(&new_url), None, None)
        .await;
    let r = h.refresh(id, true).await;
    match &r.feed_url {
        FeedUrlStatus::ChangeDetected { reason, .. } => {
            assert!(reason.contains("not_found"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_permanent_redirect_migrates_the_source() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    h.reset().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/moved.xml"))
        .mount(&h.server)
        .await;
    h.serve(
        "/moved.xml",
        &fixture("episodes_v2.xml"),
        Some("\"m1\""),
        None,
    )
    .await;
    let r = h.refresh(id, true).await;
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert_eq!(r.episodes.added, 1);
    match &r.feed_url {
        FeedUrlStatus::Changed { to, via, .. } => {
            assert_eq!(to.as_str(), h.url("/moved.xml"));
            assert_eq!(*via, ReplacementReason::Redirect);
        }
        other => panic!("{other:?}"),
    }
    let current = h.engine.podcast(id).await.unwrap().source.unwrap();
    assert_eq!(current.feed_url.as_str(), h.url("/moved.xml"));
    assert_eq!(current.fetch.etag.as_deref(), Some("\"m1\""));
    assert_eq!(current.fetch.state, FetchState::Fetched);
    let history = h.engine.sources(id).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history
            .iter()
            .find(|s| !s.is_current)
            .unwrap()
            .replacement_reason,
        Some(ReplacementReason::Redirect)
    );

    // Follow-up refresh goes straight to the new URL and is a 304/fingerprint hit.
    h.reset().await;
    h.serve(
        "/moved.xml",
        &fixture("episodes_v2.xml"),
        Some("\"m1\""),
        None,
    )
    .await;
    let r = h.refresh(id, false).await;
    assert!(
        matches!(r.outcome, RefreshOutcome::NotModified { .. }),
        "{:?}",
        r.outcome
    );
    assert_eq!(r.source_id, current.id);
}

#[tokio::test]
async fn a_redirect_to_another_show_fails() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let snapshot = h.engine.episodes(id, None, 10).await.unwrap();
    for status in [301u16, 302] {
        h.reset().await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(ResponseTemplate::new(status).insert_header("location", "/other.xml"))
            .mount(&h.server)
            .await;
        h.serve_fixture("/other.xml", "podcasting20.xml").await;
        let r = h.refresh(id, true).await;
        match &r.outcome {
            RefreshOutcome::Failed { kind, detail } => {
                assert_eq!(*kind, FetchErrorKind::InvalidPodcastFeed, "{status}");
                assert!(detail.contains("different podcast"), "{status}: {detail}");
            }
            other => panic!("{status}: {other:?}"),
        }
        assert_eq!(h.engine.episodes(id, None, 10).await.unwrap(), snapshot);
        assert_eq!(h.engine.sources(id).await.unwrap().len(), 1);
        assert_eq!(
            h.engine.podcast(id).await.unwrap().podcast.title,
            "Versioned Show"
        );
    }
}

#[tokio::test]
async fn a_recorded_redirect_chain_lands_final() {
    // feeds.buzzsprout.com/1.rss answered 301 to rss.buzzsprout.com/1.rss
    // when recorded; both hosts are replayed by one mock server.
    let h = Harness::new().await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1.rss"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(live_feed(
                    "feeds_buzzsprout_com",
                    "buzzsprout_feed_redirect",
                ))
                .insert_header("content-type", "text/xml; charset=utf-8")
                .insert_header("etag", "W/\"6018bf577866160af8f91c81db72938e\""),
        )
        .mount(&other)
        .await;
    Mock::given(method("GET"))
        .and(path("/1.rss"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", format!("{}/1.rss", other.uri())),
        )
        .mount(&h.server)
        .await;
    let added = h
        .engine
        .add_podcast(&h.url("/1.rss"), uguisu_http::CancellationToken::new())
        .await
        .unwrap();
    let r = added.report.unwrap();
    assert_eq!(r.outcome, RefreshOutcome::Fetched, "{:?}", r.warnings);
    let current = h
        .engine
        .podcast(added.podcast.id)
        .await
        .unwrap()
        .source
        .unwrap();
    assert_eq!(current.feed_url.as_str(), format!("{}/1.rss", other.uri()));
    assert!(r.episodes.added > 0);
    // Whether the resolver or the migration performed the move, a second
    // refresh is not modified and hits the final URL only.
    let r = h.refresh(added.podcast.id, false).await;
    assert!(
        matches!(r.outcome, RefreshOutcome::NotModified { .. }),
        "{:?}",
        r.outcome
    );
}

fn drain(sub: &mut uguisu_engine::Subscription) -> Vec<uguisu_core::Event> {
    let mut out = Vec::new();
    while let Some(e) = sub.try_recv() {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn unverified_announcement_is_kept_as_source() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let other = h.url("/other.xml");
    h.reset().await;
    h.serve("/feed.xml", &v1_announcing(&other), None, None)
        .await;
    h.serve_fixture("/other.xml", "podcasting20.xml").await;

    h.refresh(id, true).await;
    let first = h.engine.podcast(id).await.unwrap().announced.unwrap();
    assert_eq!(first.feed_url.as_str(), other);
    assert!(!first.is_current && first.replaced_by_source_id.is_none());
    assert_eq!(first.fetch.state, FetchState::Failed);
    assert_eq!(
        first.fetch.last_error_kind,
        Some(FetchErrorKind::InvalidPodcastFeed)
    );
    let detail = first.fetch.last_error_detail.as_deref().unwrap();
    assert!(detail.contains("title differs"), "{detail}");
    assert_eq!(first.fetch.last_http_status, Some(200));

    // Checked again, the same row counts on.
    h.refresh(id, true).await;
    let second = h.engine.podcast(id).await.unwrap().announced.unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.discovered_at, first.discovered_at);
    assert_eq!(second.fetch.consecutive_failures, 2);

    // An unchanged feed is not checked again and keeps it.
    let r = h.refresh(id, false).await;
    assert!(
        matches!(r.outcome, RefreshOutcome::NotModified { .. }),
        "{r:?}"
    );
    let kept = h.engine.podcast(id).await.unwrap().announced.unwrap();
    assert_eq!(kept.fetch.consecutive_failures, 2);

    // Another URL replaces it.
    let third = h.url("/third.xml");
    h.reset().await;
    h.serve("/feed.xml", &v1_announcing(&third), None, None)
        .await;
    h.refresh(id, true).await;
    let replaced = h.engine.podcast(id).await.unwrap().announced.unwrap();
    assert_ne!(replaced.id, first.id);
    assert_eq!(replaced.feed_url.as_str(), third);
    assert_eq!(
        replaced.fetch.last_error_kind,
        Some(FetchErrorKind::NotFound)
    );
    assert_eq!(replaced.fetch.consecutive_failures, 1);

    // A feed that stops announcing drops it.
    h.reset().await;
    h.serve_fixture("/feed.xml", "episodes_v1.xml").await;
    h.refresh(id, true).await;
    let detail = h.engine.podcast(id).await.unwrap();
    assert!(detail.announced.is_none());
    assert_eq!(detail.source.unwrap().id, added.source.id);
    assert_eq!(h.engine.sources(id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn verified_move_drops_announcement() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let new_url = h.url("/new.xml");
    h.reset().await;
    h.serve("/feed.xml", &v1_announcing(&new_url), None, None)
        .await;
    h.refresh(id, true).await;
    assert!(h.engine.podcast(id).await.unwrap().announced.is_some());

    h.serve_fixture("/new.xml", "episodes_v2.xml").await;
    let r = h.refresh(id, true).await;
    assert!(matches!(r.feed_url, FeedUrlStatus::Changed { .. }), "{r:?}");
    let detail = h.engine.podcast(id).await.unwrap();
    assert!(detail.announced.is_none());
    assert_eq!(detail.source.unwrap().feed_url.as_str(), new_url);
    assert_eq!(h.engine.sources(id).await.unwrap().len(), 2);
}

async fn move_feed(
    h: &Harness,
    id: PodcastId,
    url: &str,
    dry_run: bool,
    force: bool,
) -> Result<FeedMove, UguisuError> {
    h.engine
        .move_feed(
            id,
            url,
            MoveOptions { dry_run, force },
            CancellationToken::new(),
        )
        .await
}

async fn episode_ids(h: &Harness, id: PodcastId) -> Vec<EpisodeId> {
    let mut ids: Vec<_> = h
        .engine
        .episodes(id, None, 100)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn unverified_move_needs_force() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let before = episode_ids(&h, id).await;
    // The new host renamed the show: the check fails on the title.
    let new_url = h.url("/renamed.xml");
    let renamed = String::from_utf8(fixture("episodes_v1.xml"))
        .unwrap()
        .replacen(
            "<title>Versioned Show</title>",
            "<title>Versioned Show Reloaded</title>",
            1,
        );
    h.reset().await;
    h.serve("/feed.xml", &v1_announcing(&new_url), None, None)
        .await;
    h.serve("/renamed.xml", renamed.as_bytes(), None, None)
        .await;
    h.refresh(id, true).await;
    assert!(h.engine.podcast(id).await.unwrap().announced.is_some());

    let asked = move_feed(&h, id, &new_url, false, false).await.unwrap();
    assert!(!asked.moved && !asked.verified, "{asked:?}");
    assert!(asked.check.contains("title differs"), "{}", asked.check);
    assert!(asked.report.is_none());
    let detail = h.engine.podcast(id).await.unwrap();
    assert_eq!(detail.source.unwrap().id, added.source.id);
    assert!(detail.announced.is_some(), "an unforced move keeps it");

    let mut sub = h.engine.subscribe();
    let forced = move_feed(&h, id, &new_url, false, true).await.unwrap();
    assert!(forced.moved && !forced.verified, "{forced:?}");
    assert_eq!(forced.to.as_str(), new_url);
    let report = forced.report.unwrap();
    assert_eq!(report.outcome, RefreshOutcome::Fetched);
    assert_eq!(report.episodes.added, 0, "{report:?}");

    let detail = h.engine.podcast(id).await.unwrap();
    let current = detail.source.unwrap();
    assert_eq!(current.feed_url.as_str(), new_url);
    assert!(detail.announced.is_none());
    assert_eq!(detail.podcast.title, "Versioned Show Reloaded");
    let (old, _) = h.engine.source(added.source.id).await.unwrap();
    assert_eq!(old.replacement_reason, Some(ReplacementReason::Manual));
    assert_eq!(old.replaced_by_source_id, Some(current.id));
    assert_eq!(episode_ids(&h, id).await, before, "episodes keep their ids");
    let events = drain(&mut sub);
    assert!(events.iter().any(|e| matches!(
        e.kind,
        EventKind::FeedUrlChanged {
            via: ReplacementReason::Manual,
            ..
        }
    )));
}

#[tokio::test]
async fn verified_move_keeps_episodes() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    let before = episode_ids(&h, id).await;
    let new_url = h.url("/v2.xml");
    h.serve_fixture("/v2.xml", "episodes_v2.xml").await;

    let looked = move_feed(&h, id, &new_url, true, false).await.unwrap();
    assert!(looked.verified && !looked.moved, "{looked:?}");
    assert_eq!(
        h.engine.podcast(id).await.unwrap().source.unwrap().id,
        added.source.id,
        "a dry run moves nothing"
    );

    let moved = move_feed(&h, id, &new_url, false, false).await.unwrap();
    assert!(moved.moved && moved.verified, "{moved:?}");
    assert_eq!(moved.report.unwrap().episodes.added, 1);
    let current = h.engine.podcast(id).await.unwrap().source.unwrap();
    assert_eq!(current.feed_url.as_str(), new_url);
    let after = episode_ids(&h, id).await;
    assert!(
        before.iter().all(|e| after.contains(e)),
        "{before:?} {after:?}"
    );
    assert_eq!(h.engine.sources(id).await.unwrap().len(), 2);

    let again = move_feed(&h, id, &new_url, false, false).await.unwrap();
    assert!(!again.moved, "{again:?}");
    assert_eq!(h.engine.sources(id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn move_refuses_another_podcast() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    h.serve_fixture("/p20.xml", "podcasting20.xml").await;
    h.serve_fixture("/copy.xml", "podcasting20.xml").await;
    h.engine
        .add_podcast(&h.url("/p20.xml"), CancellationToken::new())
        .await
        .unwrap();

    for url in [h.url("/p20.xml"), h.url("/copy.xml")] {
        let err = move_feed(&h, id, &url, false, true).await.unwrap_err();
        assert!(matches!(err, UguisuError::Conflict(_)), "{url}: {err}");
    }
    let current = h.engine.podcast(id).await.unwrap().source.unwrap();
    assert_eq!(current.id, added.source.id);
}

#[tokio::test]
async fn unusable_feed_moves_nothing() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    h.serve("/page.xml", b"<html><body>a page</body></html>", None, None)
        .await;

    let err = move_feed(&h, id, "ftp://example.test/feed.xml", false, true)
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::Invalid(_)), "{err}");
    let err = move_feed(&h, id, &h.url("/missing.xml"), false, true)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            UguisuError::Network {
                kind: FetchErrorKind::NotFound,
                ..
            }
        ),
        "{err}"
    );
    let err = move_feed(&h, id, &h.url("/page.xml"), false, true)
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::Feed { .. }), "{err}");
    let same = move_feed(&h, id, &h.url("/feed.xml"), false, true)
        .await
        .unwrap();
    assert!(!same.moved, "{same:?}");
    assert_eq!(h.engine.sources(id).await.unwrap().len(), 1);
}
