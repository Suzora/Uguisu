//! The download queue through the engine: manual enqueue, the media
//! client, files and archive states, events on the bus, startup
//! reconciliation after a silent death and a graceful close that parks
//! running jobs.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::case_sensitive_file_extension_comparisons
)]

mod common;

use std::time::Duration;

use common::{Harness, synthetic_feed_with_media};
use sha2::{Digest, Sha256};
use uguisu_core::download::{AttemptOutcome, DownloadState, Priority};
use uguisu_core::events::EventKind;
use uguisu_core::model::ArchiveState;
use uguisu_download::deps::{FailInjector, FailPoint};
use uguisu_download::testing::content_sha256;
use uguisu_download::{EnqueueOutcome, JobFilter};
use uguisu_http::CancellationToken;
use uguisu_storage::events;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Serves a media-backed synthetic feed and adds it.
async fn add_media_podcast(
    h: &Harness,
    count: usize,
    scenarios: &[&str],
) -> uguisu_core::model::Podcast {
    let body = synthetic_feed_with_media(count, h.media.base(), scenarios);
    Mock::given(method("GET"))
        .and(path("/media.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    h.engine
        .add_podcast(&h.url("/media.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast
}

fn sha256_of(path: &std::path::Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
}

async fn wait_for<F: Fn() -> bool>(what: &str, f: F) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn podcast_downloads_end_to_end() {
    let h = Harness::new().await;
    let mut sub = h.engine.subscribe();
    let podcast = add_media_podcast(
        &h,
        5,
        &[
            "/range/300000",
            "/normal/120000",
            "/slow/200000/400000",
            "/range/64",
        ],
    )
    .await;

    // Nothing is enqueued by discovery alone.
    let stats = h.engine.downloads().stats().await.unwrap();
    assert!(stats.by_state.is_empty(), "{stats:?}");
    assert!(!stats.workers_started);
    assert!(!h.media_dir().exists(), "media dir is created lazily");

    let summary = h
        .engine
        .downloads()
        .enqueue_podcast(podcast.id, Priority::Normal)
        .await
        .unwrap();
    assert_eq!(summary.created, 5);
    assert!(summary.skipped.is_empty());
    let again = h
        .engine
        .downloads()
        .enqueue_podcast(podcast.id, Priority::High)
        .await
        .unwrap();
    assert_eq!(again.existing, 5, "idempotent");

    h.engine.start_downloads();
    h.engine.start_downloads();
    assert!(h.engine.downloads().is_started());
    h.engine.downloads().wait_idle().await.unwrap();

    let page = h
        .engine
        .downloads()
        .list(&JobFilter {
            podcast_id: Some(podcast.id),
            limit: 50,
            ..JobFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(page.jobs.len(), 5);
    let media_dir = h.media_dir();
    let episodes = h
        .engine
        .episodes(podcast.id, None, 50)
        .await
        .unwrap()
        .episodes;
    for summary in &page.jobs {
        let job = &summary.job;
        assert_eq!(job.state, DownloadState::Completed, "{job:?}");
        let rel = job.target_path.as_str();
        // New downloads are written straight to the path the
        // archive template renders, not to the identifier layout.
        assert!(
            rel.starts_with("Synthetic Show/2020/"),
            "template destination: {rel}"
        );
        assert!(rel.ends_with(".mp3"), "{rel}");
        let target = media_dir.join(rel);
        assert!(target.is_file(), "{}", target.display());
        let episode = episodes.iter().find(|e| e.id == job.episode_id).unwrap();
        let url = &episode.primary_enclosure().unwrap().url;
        let size: u64 = url
            .path_segments()
            .unwrap()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(job.total_bytes, Some(size));
        assert_eq!(sha256_of(&target), content_sha256(size));
        assert_eq!(
            job.hash_value.as_deref(),
            Some(content_sha256(size).as_str())
        );
        assert_eq!(episode.archive_state, ArchiveState::Archived);
        assert_eq!(
            h.engine
                .downloads()
                .enqueue_episode(episode.id, Priority::Low)
                .await
                .unwrap(),
            EnqueueOutcome::AlreadyCompleted(job.clone())
        );
    }
    let tmp = media_dir.join(podcast.id.to_string()).join(".uguisu-tmp");
    let leftovers: Vec<_> = std::fs::read_dir(&tmp)
        .map(|d| d.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Events on the bus: per job queued → started → progress* → completed;
    // the store never holds a progress event.
    let mut live = Vec::new();
    while let Some(e) = sub.try_recv() {
        live.push(e);
    }
    for summary in &page.jobs {
        let job = &summary.job;
        let names: Vec<&str> = live
            .iter()
            .filter(|e| {
                let payload = serde_json::to_value(&e.kind).unwrap();
                payload["job_id"].as_str() == Some(job.id.to_string().as_str())
            })
            .map(uguisu_core::Event::name)
            .collect();
        assert_eq!(names.first(), Some(&"download.queued"), "{names:?}");
        assert_eq!(names.get(1), Some(&"download.started"), "{names:?}");
        assert_eq!(names.last(), Some(&"download.completed"), "{names:?}");
        assert!(
            names[2..names.len() - 1]
                .iter()
                .all(|n| *n == "download.progress"),
            "{names:?}"
        );
    }
    assert!(
        live.iter().any(|e| e.kind.is_transient()),
        "progress was published live"
    );
    let mut conn = h.engine.storage().reader().await.unwrap();
    let stored = events::list_after(&mut conn, None, 500).await.unwrap();
    assert!(!stored.iter().any(|e| e.kind.is_transient()));
    assert_eq!(
        stored
            .iter()
            .filter(|e| matches!(e.kind, EventKind::DownloadCompleted { .. }))
            .count(),
        5
    );
    drop(conn);
    h.engine.close().await;
}

#[tokio::test]
async fn open_reconciles_a_silently_dead_job() {
    let mut h = Harness::new().await;
    let podcast = add_media_podcast(&h, 1, &["/slow/2000000/4000000"]).await;
    let episode = h
        .engine
        .episodes(podcast.id, None, 1)
        .await
        .unwrap()
        .episodes[0]
        .clone();
    let job = match h
        .engine
        .downloads()
        .enqueue_episode(episode.id, Priority::High)
        .await
        .unwrap()
    {
        EnqueueOutcome::Created(j) => j,
        other => panic!("{other:?}"),
    };

    // A worker that dies after 1 MB without touching the database (the
    // route is slow enough for progress to have been persisted by then).
    let injector = FailInjector::armed(FailPoint::AfterBytes(1_000_000));
    h.restart(Some(injector.clone())).await;
    h.engine.start_downloads();
    wait_for("the fail point", || injector.was_hit()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stuck = h.engine.downloads().job(job.id).await.unwrap().job;
    assert_eq!(stuck.state, DownloadState::Downloading, "nobody cleaned up");
    let part = h.media_dir().join(&stuck.part_path);
    assert!(part.is_file());
    // The dead job is nobody's any more: closing parks nothing and the
    // row stays `downloading`, exactly as after a crash.
    h.restart(None).await;
    let report = h.engine.downloads().last_reconcile().unwrap();
    assert_eq!(report.recovered, 1, "{report:?}");
    let recovered = h.engine.downloads().job(job.id).await.unwrap();
    assert_eq!(recovered.job.state, DownloadState::Queued);
    assert_eq!(recovered.job.state_reason.as_deref(), Some("recovered"));
    assert_eq!(recovered.attempts.len(), 1);
    assert_eq!(
        recovered.attempts[0].outcome,
        Some(AttemptOutcome::Interrupted)
    );
    assert_eq!(episode_state(&h, episode.id).await, ArchiveState::Queued);

    h.media.reset();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    let done = h.engine.downloads().job(job.id).await.unwrap();
    assert_eq!(done.job.state, DownloadState::Completed);
    assert_eq!(done.attempts.len(), 2);
    let resumed = h.media.requests_for("/slow/2000000/4000000");
    assert_eq!(resumed.len(), 1);
    let from = resumed[0].range_start().unwrap();
    assert!(from > 0 && from <= 1_000_000, "resumed from {from}");
    assert_eq!(
        sha256_of(&h.media_dir().join(&done.job.target_path)),
        content_sha256(2_000_000)
    );
    assert_eq!(episode_state(&h, episode.id).await, ArchiveState::Archived);
    h.engine.close().await;
}

#[tokio::test]
async fn close_parks_jobs_reopen_resumes() {
    let mut h = Harness::new().await;
    let podcast = add_media_podcast(&h, 1, &["/slow/400000/200000"]).await;
    let episode = h
        .engine
        .episodes(podcast.id, None, 1)
        .await
        .unwrap()
        .episodes[0]
        .clone();
    let mut sub = h.engine.subscribe();
    h.engine
        .downloads()
        .enqueue_episode(episode.id, Priority::Normal)
        .await
        .unwrap();
    h.engine.start_downloads();
    loop {
        let e = sub.recv().await.unwrap();
        if matches!(e.kind, EventKind::DownloadProgress { bytes_downloaded, .. } if bytes_downloaded > 0)
        {
            break;
        }
    }
    h.restart(None).await;
    // Not completed: close did not wait for the transfer. Nothing recovered:
    // it did not abort the worker after the grace period either.
    assert_eq!(h.engine.downloads().last_reconcile().unwrap().recovered, 0);
    let job = h
        .engine
        .downloads()
        .job_for_episode(episode.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.state, DownloadState::Queued);
    assert_eq!(job.state_reason.as_deref(), Some("shutdown"));
    assert!(job.bytes_downloaded > 0);
    assert!(h.media_dir().join(&job.part_path).is_file());

    h.media.reset();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    let done = h.engine.downloads().job(job.id).await.unwrap().job;
    assert_eq!(done.state, DownloadState::Completed);
    let reqs = h.media.requests_for("/slow/400000/200000");
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].range_start(), Some(job.bytes_downloaded));
    assert_eq!(
        sha256_of(&h.media_dir().join(&done.target_path)),
        content_sha256(400_000)
    );
    h.engine.close().await;
}

async fn episode_state(h: &Harness, id: uguisu_core::ids::EpisodeId) -> ArchiveState {
    let mut conn = h.engine.storage().reader().await.unwrap();
    uguisu_storage::episodes::get(&mut conn, id)
        .await
        .unwrap()
        .unwrap()
        .archive_state
}
