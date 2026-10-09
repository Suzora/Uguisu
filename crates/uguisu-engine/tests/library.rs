//! Library service: adding is idempotent, conflicts are refused, the
//! data directory is single-process, and reads page correctly.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use uguisu_core::UguisuError;
use uguisu_core::config::FeedConfig;
use uguisu_core::events::EventKind;
use uguisu_core::ids::PodcastId;
use uguisu_core::model::FetchState;
use uguisu_engine::Engine;
use uguisu_http::CancellationToken;

#[tokio::test]
async fn add_is_idempotent_and_emits_once() {
    let h = Harness::new().await;
    h.serve_fixture("/feed.xml", "minimal_rss.xml").await;
    let mut sub = h.engine.subscribe();

    let first = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(first.source.podcast_id, first.podcast.id);
    assert!(first.source.is_current);
    assert_eq!(first.source.fetch.state, FetchState::NeverFetched);
    assert_eq!(
        first.podcast.sort_title,
        uguisu_core::model::sort_title(&first.podcast.title)
    );

    let event = sub
        .try_recv()
        .expect("podcast.added published after commit");
    assert_eq!(event.podcast_id, Some(first.podcast.id));
    assert!(
        matches!(event.kind, EventKind::PodcastAdded { source_id, .. } if source_id == first.source.id)
    );
    assert!(first.report.is_some(), "a new podcast is refreshed at once");
    while sub.try_recv().is_some() {} // the first refresh's events

    let second = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap();
    assert!(!second.created);
    assert!(
        second.report.is_none(),
        "an existing podcast is not refreshed"
    );
    assert_eq!(second.podcast.id, first.podcast.id);
    assert_eq!(second.source.id, first.source.id);
    let stray = sub.try_recv();
    assert!(
        stray.is_none(),
        "no second podcast.added, no refresh: {:?}",
        stray.map(|e| e.name())
    );

    let list = h.engine.list_podcasts().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].podcast.id, first.podcast.id);
    assert_eq!(list[0].episodes_total, 1);
    assert_eq!(
        list[0].last_fetch.as_ref().map(|f| f.id),
        first.report.as_ref().map(|r| r.fetch_id)
    );

    let shown = h.engine.podcast(first.podcast.id).await.unwrap();
    assert_eq!(shown.source.unwrap().id, first.source.id);
    let (by_id, latest_fetch) = h.engine.source(first.source.id).await.unwrap();
    assert_eq!(by_id.id, first.source.id);
    assert!(latest_fetch.is_some());
    assert_eq!(by_id.fetch.state, FetchState::Fetched);
}

#[tokio::test]
async fn one_guid_under_two_urls_conflicts() {
    let h = Harness::new().await;
    h.serve_fixture("/a.xml", "podcasting20.xml").await;
    h.serve_fixture("/b.xml", "podcasting20.xml").await;
    let first = h
        .engine
        .add_podcast(&h.url("/a.xml"), CancellationToken::new())
        .await
        .unwrap();
    assert!(first.podcast.podcast_guid.is_some());
    let err = h
        .engine
        .add_podcast(&h.url("/b.xml"), CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        UguisuError::Conflict(msg) => {
            assert!(msg.contains(&first.podcast.id.to_string()), "{msg}");
            assert!(msg.contains("/a.xml"), "{msg}");
        }
        other => panic!("expected conflict, got {other:?}"),
    }
    assert_eq!(h.engine.list_podcasts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn bad_input_is_reported_without_writing() {
    let h = Harness::new().await;
    let err = h
        .engine
        .add_podcast("   ", CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::Invalid(_)));
    let err = h
        .engine
        .add_podcast(&h.url("/missing.xml"), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            UguisuError::Network { .. } | UguisuError::Unresolvable { .. }
        ),
        "{err:?}"
    );
    let err = h
        .engine
        .add_podcast("http://10.0.0.1/feed.xml", CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::BlockedByPolicy(_)), "{err:?}");
    assert!(h.engine.list_podcasts().await.unwrap().is_empty());
}

#[tokio::test]
async fn one_process_holds_the_data_directory() {
    let mut h = Harness::new().await;
    let err = Engine::open(common::config(
        h.dir.path().to_path_buf(),
        FeedConfig::default(),
    ))
    .await
    .unwrap_err();
    assert!(matches!(err, UguisuError::Locked(_)), "{err:?}");
    assert!(h.dir.path().join("uguisu.lock").exists());
    assert!(h.dir.path().join("uguisu.db").exists());

    // Dropping the handle releases the lock; the data survives.
    h.serve_fixture("/feed.xml", "minimal_rss.xml").await;
    let added = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap();
    h.engine.close().await;
    let placeholder = Engine::open(
        common::config(h.dir.path().to_path_buf(), FeedConfig::default()).with_data(
            uguisu_core::config::DataConfig {
                data_dir: Some(tempfile::tempdir().unwrap().keep()),
                media_dir: None,
            },
        ),
    )
    .await
    .unwrap();
    h.engine = placeholder;
    h.reopen().await;
    let shown = h.engine.podcast(added.podcast.id).await.unwrap();
    assert_eq!(shown.podcast.title, added.podcast.title);
}

#[tokio::test]
async fn unknown_ids_and_cursors_refused() {
    let h = Harness::new().await;
    let missing = PodcastId::new();
    assert!(matches!(
        h.engine.podcast(missing).await.unwrap_err(),
        UguisuError::NotFound { entity, .. } if entity == "podcast"
    ));
    assert!(matches!(
        h.engine.episodes(missing, None, 10).await.unwrap_err(),
        UguisuError::NotFound { .. }
    ));
    h.serve_fixture("/feed.xml", "minimal_rss.xml").await;
    let added = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap();
    // A page of nothing is refused rather than quietly turned into a page of
    // one: a caller that asked for zero rows has a bug (ADR 0040).
    assert!(matches!(
        h.engine
            .episodes(added.podcast.id, None, 0)
            .await
            .unwrap_err(),
        UguisuError::Invalid(_)
    ));
    // One episode, asked for one at a time: there is no second page, so there
    // is no cursor - the feed has exactly one item.
    let page = h.engine.episodes(added.podcast.id, None, 1).await.unwrap();
    assert_eq!(page.episodes.len(), 1);
    assert!(
        page.next_after.is_none(),
        "a full page with nothing after it must not offer a cursor"
    );
    let err = h
        .engine
        .episodes(
            added.podcast.id,
            Some(uguisu_core::ids::EpisodeId::new()),
            10,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::Invalid(_)), "{err:?}");
}
