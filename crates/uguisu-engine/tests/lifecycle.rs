//! Archiving and removing a podcast (ADR 0055): an archived podcast is
//! never refreshed until it is resumed, and a removed one leaves every file
//! where it was, enough to rebuild its records from.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Harness, synthetic_feed_with_media, tree_digest};
use uguisu_core::UguisuError;
use uguisu_core::download::Priority;
use uguisu_core::events::EventKind;
use uguisu_core::ids::PodcastId;
use uguisu_core::model::{FetchState, PodcastStatus};
use uguisu_engine::rebuild::RebuildOptions;
use uguisu_http::CancellationToken;
use uguisu_storage::archive_files::{self, ArchiveFilter};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn serve_media_feed(h: &Harness, count: usize) {
    let body = synthetic_feed_with_media(count, h.media.base(), &["/normal/2048"]);
    Mock::given(method("GET"))
        .and(path("/media.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
}

async fn add(h: &Harness) -> PodcastId {
    h.engine
        .add_podcast(&h.url("/media.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast
        .id
}

async fn files(h: &Harness) -> usize {
    let mut reader = h.engine.storage().reader().await.unwrap();
    archive_files::list(&mut reader, &ArchiveFilter::default(), None, 100)
        .await
        .unwrap()
        .len()
}

/// A podcast with `count` episodes downloaded, archived and described.
async fn archived(h: &Harness, count: usize) -> PodcastId {
    serve_media_feed(h, count).await;
    let id = add(h).await;
    h.engine
        .downloads()
        .enqueue_podcast(id, Priority::Normal)
        .await
        .unwrap();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let mut reader = h.engine.storage().reader().await.unwrap();
        let all = archive_files::list(&mut reader, &ArchiveFilter::default(), None, 100)
            .await
            .unwrap();
        if all.len() == count && all.iter().all(|f| f.sidecar_written_at.is_some()) {
            return id;
        }
        assert!(std::time::Instant::now() < deadline, "not archived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn archived_podcast_is_never_refreshed() {
    let h = Harness::new().await;
    let id = h.add_fixture("episodes_v1.xml").await.podcast.id;

    assert_eq!(
        h.engine.archive_podcast(id).await.unwrap(),
        PodcastStatus::Archived
    );
    let detail = h.engine.podcast(id).await.unwrap();
    assert_eq!(detail.podcast.status, PodcastStatus::Archived);
    assert_eq!(detail.source.unwrap().fetch.state, FetchState::Disabled);
    assert_eq!(detail.episodes_total, 3, "nothing is taken away");

    let err = h
        .engine
        .refresh_podcast(id, uguisu_engine::RefreshOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::Conflict(_)), "{err}");
    let all = h
        .engine
        .refresh_all(false, 2, CancellationToken::new())
        .await
        .unwrap();
    assert!(all.iter().all(|e| e.podcast_id != id), "{all:?}");
    assert!(
        matches!(
            h.engine.pause_podcast(id).await,
            Err(UguisuError::Conflict(_))
        ),
        "an archived podcast is resumed, not paused"
    );

    assert_eq!(
        h.engine.resume_podcast(id).await.unwrap(),
        PodcastStatus::Active
    );
    let detail = h.engine.podcast(id).await.unwrap();
    assert_eq!(detail.source.unwrap().fetch.state, FetchState::NeverFetched);
    h.refresh(id, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removal_keeps_every_file() {
    let h = Harness::new().await;
    let id = archived(&h, 2).await;
    // The manifest is written once the bus has been quiet for a while; a
    // digest taken before that would see it appear.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while h
        .engine
        .manifest_status()
        .await
        .unwrap()
        .iter()
        .any(|m| m.stale)
    {
        assert!(std::time::Instant::now() < deadline, "manifest not written");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let before = tree_digest(&h.media_dir());
    let mut sub = h.engine.subscribe();

    let removed = h.engine.remove_podcast(id).await.unwrap();
    assert_eq!((removed.episodes, removed.files), (2, 2));
    assert_eq!(tree_digest(&h.media_dir()), before, "a file changed");
    assert!(matches!(
        h.engine.podcast(id).await,
        Err(UguisuError::NotFound { .. })
    ));
    assert_eq!(files(&h).await, 0);
    let event = std::iter::from_fn(|| sub.try_recv())
        .find(|e| matches!(e.kind, EventKind::PodcastRemoved { .. }))
        .expect("podcast.removed");
    assert_eq!(event.podcast_id, Some(id));

    let orphans = h.engine.orphans().await.unwrap();
    assert_eq!(orphans.unknown_media.count, 2, "{orphans:?}");
    assert!(orphans.stray_sidecars.is_empty(), "{orphans:?}");

    // Adding the feed again and rebuilding restores the records.
    let again = add(&h).await;
    assert_ne!(again, id);
    let rebuilt = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert_eq!(rebuilt.rebuilt, 2, "{rebuilt:?}");
    assert_eq!(files(&h).await, 2);
    assert!(h.engine.orphans().await.unwrap().unknown_media.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn running_download_refuses_removal() {
    let h = Harness::new().await;
    let id = archived(&h, 1).await;
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        sqlx::query("UPDATE download_jobs SET state = 'downloading' WHERE podcast_id = ?1")
            .bind(id.to_string())
            .execute(&mut *w)
            .await
            .unwrap();
    }
    let err = h.engine.remove_podcast(id).await.unwrap_err();
    assert!(matches!(err, UguisuError::Conflict(_)), "{err}");
    assert!(h.engine.podcast(id).await.is_ok());
}
