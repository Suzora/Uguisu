//! What a crash or a race must not be able to do to the archive.
//!
//! Every test here asserts the same two things in a different way: no
//! valid file is lost, and nothing is deleted to make the database and
//! the disk agree.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;

use std::time::Duration;

use common::{Harness, synthetic_feed_with_media};
use uguisu_core::archive::{VerificationState, VerifyDepth};
use uguisu_core::download::Priority;
use uguisu_core::ids::EpisodeId;
use uguisu_download::deps::{FailInjector, FailPoint};
use uguisu_engine::archive::ArchiveFilter;
use uguisu_http::CancellationToken;
use uguisu_storage::archive_files;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn add_media_podcast(h: &Harness, count: usize) -> uguisu_core::model::Podcast {
    let body = synthetic_feed_with_media(count, h.media.base(), &["/normal/4096", "/range/8192"]);
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

async fn wait_for<F, Fut>(what: &str, f: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !f().await {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn archived_count(h: &Harness) -> usize {
    let mut reader = h.engine.storage().reader().await.unwrap();
    archive_files::list(&mut reader, &ArchiveFilter::default(), None, 100)
        .await
        .unwrap()
        .len()
}

/// Downloads everything and waits for the watcher to record it.
async fn download_all(h: &Harness, podcast: uguisu_core::ids::PodcastId, count: usize) {
    h.engine
        .downloads()
        .enqueue_podcast(podcast, Priority::Normal)
        .await
        .unwrap();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    wait_for("the archive records", || async {
        archived_count(h).await == count
    })
    .await;
}

/// The download crash points still leave a consistent archive: a worker
/// that died silently is recovered on the next start, the retry finishes
/// the transfer, and the archive engine records the result.
#[tokio::test]
async fn a_crash_near_finalization_loses_nothing() {
    for point in [
        FailPoint::BeforeFinalizingWrite,
        FailPoint::AfterFinalizingWrite,
        FailPoint::AfterRename,
    ] {
        let mut h = Harness::new().await;
        let podcast = add_media_podcast(&h, 1).await;
        h.engine
            .downloads()
            .enqueue_podcast(podcast.id, Priority::Normal)
            .await
            .unwrap();

        // Die at the chosen point. The worker returns without touching the
        // database, so nothing is cleaned up — exactly as after a kill.
        let injector = FailInjector::armed(point);
        h.restart(Some(injector.clone())).await;
        h.engine.start_downloads();
        wait_for(&format!("{point:?}"), || {
            let injector = injector.clone();
            async move { injector.was_hit() }
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Restarting reconciles the queue; the retry finishes the job and
        // the watcher records it.
        h.restart(None).await;
        h.engine.start_downloads();
        h.engine.downloads().wait_idle().await.unwrap();
        wait_for(&format!("{point:?} to settle"), || async {
            archived_count(&h).await == 1
        })
        .await;

        let episode = h
            .engine
            .episodes(podcast.id, None, 10)
            .await
            .unwrap()
            .episodes[0]
            .id;
        let file = h.engine.archive_file(episode).await.unwrap().unwrap();
        let on_disk = h.media_dir().join(&file.relative_path);
        assert!(on_disk.is_file(), "{point:?}: {}", on_disk.display());

        // And the bytes are the ones the record claims.
        let verified = h
            .engine
            .verify_episode(episode, VerifyDepth::Full)
            .await
            .unwrap();
        assert_eq!(
            verified.state,
            VerificationState::Verified,
            "{point:?}: {verified:?}"
        );

        // Nothing was left over in the temporary directory either.
        let stray: Vec<_> = walk_parts(&h.media_dir());
        assert!(stray.is_empty(), "{point:?}: {stray:?}");
        h.engine.close().await;
    }
}

/// Every `.part` left under the media directory.
fn walk_parts(media_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn visit(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if path.extension().is_some_and(|e| e == "part") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(media_dir, &mut out);
    out
}

/// A crash between the completion transaction and the registration is the
/// window the design accepts; reconciliation has to close it, repeatedly
/// and without moving anything.
#[tokio::test]
async fn reconciliation_is_idempotent_and_never_relocates() {
    let mut h = Harness::new().await;
    let podcast = add_media_podcast(&h, 3).await;
    download_all(&h, podcast.id, 3).await;

    let before: Vec<(EpisodeId, String, String)> = {
        let mut reader = h.engine.storage().reader().await.unwrap();
        archive_files::list(&mut reader, &ArchiveFilter::default(), None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|f| (f.episode_id, f.relative_path, f.hash_value))
            .collect()
    };

    // The crash window: the downloads committed, the records did not.
    {
        let mut tx = h.engine.storage().begin().await.unwrap();
        for (episode, _, _) in &before {
            assert!(
                archive_files::delete_for_episode(&mut tx, *episode)
                    .await
                    .unwrap()
            );
        }
        tx.commit().await.unwrap();
    }

    h.restart(None).await;

    // Reconciliation registered each file where it lies, with the same
    // path and the same hash, and running it again changes nothing.
    for _ in 0..3 {
        let report = h.engine.reconcile_archive(true).await.unwrap();
        assert_eq!(report.registered, 0, "already registered: {report:?}");
        assert_eq!(report.missing, 0);
        assert_eq!(report.invalid, 0);
    }
    for (episode, path, hash) in &before {
        let file = h.engine.archive_file(*episode).await.unwrap().unwrap();
        assert_eq!(&file.relative_path, path, "reconciliation moved a file");
        assert_eq!(&file.hash_value, hash);
        assert!(h.media_dir().join(path).is_file());
    }
    h.engine.close().await;
}

/// A file that is gone stays gone: reported, never re-created, and its
/// record — hash included — kept so it can be recovered.
#[tokio::test]
async fn a_deleted_file_keeps_its_record() {
    let h = Harness::new().await;
    let podcast = add_media_podcast(&h, 2).await;
    download_all(&h, podcast.id, 2).await;

    let episodes: Vec<EpisodeId> = h
        .engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect();
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let on_disk = h.media_dir().join(&file.relative_path);
    std::fs::remove_file(&on_disk).unwrap();

    for depth in [
        VerifyDepth::Light,
        VerifyDepth::Full,
        VerifyDepth::Existence,
    ] {
        let summary = h
            .engine
            .verify_all(&ArchiveFilter::default(), depth)
            .await
            .unwrap();
        assert_eq!(summary.missing, 1, "{depth}: {summary:?}");
        let still = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
        assert_eq!(still.verification_state, VerificationState::Missing);
        assert_eq!(still.hash_value, file.hash_value, "the hash is kept");
        assert!(!on_disk.exists(), "verification must not re-create it");
    }

    // The other episode is untouched by any of that.
    let other = h.engine.archive_file(episodes[1]).await.unwrap().unwrap();
    assert!(h.media_dir().join(&other.relative_path).is_file());

    // Putting the file back makes it verify again, with the same record.
    let good = h.media_dir().join(&other.relative_path);
    std::fs::copy(&good, &on_disk).unwrap();
    let recovered = h
        .engine
        .verify_episode(episodes[0], VerifyDepth::Full)
        .await
        .unwrap();
    // Different bytes, so it is invalid rather than verified — but the
    // point is that the record was still there to compare against.
    assert_eq!(recovered.state, VerificationState::Invalid);
    assert_eq!(recovered.file.hash_value, file.hash_value);
    h.engine.close().await;
}

/// Concurrent work on one artifact converges instead of racing.
#[tokio::test]
async fn concurrent_registrations_and_verifications_converge() {
    let h = Harness::new().await;
    let podcast = add_media_podcast(&h, 1).await;
    download_all(&h, podcast.id, 1).await;
    let episode = h
        .engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes[0]
        .id;
    let first = h.engine.archive_file(episode).await.unwrap().unwrap();

    // Eight registrations at once: the unique `episode_id` means one row.
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let engine = h.engine.clone();
        tasks.push(tokio::spawn(async move {
            engine.register_archive_file(episode).await
        }));
    }
    for task in tasks {
        let file = task.await.unwrap().unwrap();
        assert_eq!(file.id, first.id, "the record keeps its identity");
        assert_eq!(file.registered_at, first.registered_at);
    }
    assert_eq!(archived_count(&h).await, 1, "no duplicate rows");

    // Eight verifications at once, at mixed depths.
    let mut tasks = Vec::new();
    for i in 0..8 {
        let engine = h.engine.clone();
        let depth = if i % 2 == 0 {
            VerifyDepth::Full
        } else {
            VerifyDepth::Light
        };
        tasks.push(tokio::spawn(async move {
            engine.verify_episode(episode, depth).await
        }));
    }
    for task in tasks {
        let verified = task.await.unwrap().unwrap();
        assert_eq!(verified.state, VerificationState::Verified);
    }

    // A verification racing a relocation: whichever order they land in,
    // the file is at exactly one of the two paths and the record agrees.
    let engine = h.engine.clone();
    let relocation = tokio::spawn(async move { engine.relocate(episode, false).await });
    let engine = h.engine.clone();
    let reading =
        tokio::spawn(async move { engine.verify_episode(episode, VerifyDepth::Full).await });
    let outcome = relocation.await.unwrap();
    let _ = reading.await.unwrap();

    let after = h.engine.archive_file(episode).await.unwrap().unwrap();
    assert!(h.media_dir().join(&after.relative_path).is_file());
    assert_eq!(after.hash_value, first.hash_value);
    if let Ok(m) = outcome
        && m.moved
    {
        assert!(
            !h.media_dir().join(&m.from).exists(),
            "a rename leaves nothing behind"
        );
    }
    // Whatever happened, the artifact still verifies.
    let verified = h
        .engine
        .verify_episode(episode, VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(verified.state, VerificationState::Verified);
    h.engine.close().await;
}

/// A manual command racing the policy still produces one job.
#[tokio::test]
async fn a_policy_race_creates_one_job() {
    let h = Harness::new().await;
    let podcast = add_media_podcast(&h, 3).await;
    h.engine
        .set_policy(&uguisu_core::archive::ArchivePolicy {
            podcast_id: podcast.id,
            mode: uguisu_core::archive::PolicyMode::Auto,
            max_backlog: Some(0),
            max_age_days: None,
            priority: None,
            updated_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
    let episodes: Vec<EpisodeId> = h
        .engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect();

    let engine = h.engine.clone();
    let ids = episodes.clone();
    let policy = tokio::spawn(async move { engine.apply_policy(podcast.id, &ids).await });
    let engine = h.engine.clone();
    let manual = tokio::spawn(async move {
        engine
            .downloads()
            .enqueue_podcast(podcast.id, Priority::High)
            .await
    });
    policy.await.unwrap().unwrap();
    manual.await.unwrap().unwrap();

    let jobs = h
        .engine
        .downloads()
        .list(&uguisu_download::JobFilter {
            podcast_id: Some(podcast.id),
            limit: 50,
            ..uguisu_download::JobFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(jobs.jobs.len(), 3, "one job per episode, not two");
    let mut seen: Vec<EpisodeId> = jobs.jobs.iter().map(|j| j.job.episode_id).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 3);
    h.engine.close().await;
}
