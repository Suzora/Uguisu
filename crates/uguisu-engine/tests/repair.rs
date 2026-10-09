//! Repairing a missing archived file: the exact bytes from a folder, or a
//! new download of the episode (ADR 0060).
//!
//! Both defend the rules an import does: the source is only read, nothing
//! is overwritten, and a file that is present is never replaced.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Harness, synthetic_feed_with_media, tree_digest};
use uguisu_core::UguisuError;
use uguisu_core::archive::{
    ArchiveErrorKind, ArchiveFile, ArchiveOrigin, VerificationState, VerifyDepth,
};
use uguisu_core::config::{ArchiveConfig, FeedConfig};
use uguisu_core::download::Priority;
use uguisu_core::ids::EpisodeId;
use uguisu_core::model::Episode;
use uguisu_engine::Engine;
use uguisu_engine::import::ImportOptions;
use uguisu_engine::restore::{RestoreAction, RestoreOptions};
use uguisu_http::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// A podcast with `count` downloadable episodes of different bytes, nothing
/// downloaded.
async fn library(h: &Harness, count: usize) -> Vec<Episode> {
    let body = synthetic_feed_with_media(count, h.media.base(), &["/normal/2048", "/normal/3072"]);
    Mock::given(method("GET"))
        .and(path("/media.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/media.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast;
    h.engine
        .episodes(podcast.id, None, 100)
        .await
        .unwrap()
        .episodes
}

/// Downloads every episode and waits until each is archived and verified.
async fn archived(h: &Harness, episodes: &[Episode]) -> Vec<ArchiveFile> {
    for episode in episodes {
        h.engine
            .downloads()
            .enqueue_episode(episode.id, Priority::Normal)
            .await
            .unwrap();
    }
    settled(h, episodes).await
}

/// Runs the queue dry and waits until every episode's file is verified.
async fn settled(h: &Harness, episodes: &[Episode]) -> Vec<ArchiveFile> {
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let mut out = Vec::new();
        for episode in episodes {
            if let Some(file) = h.engine.archive_file(episode.id).await.unwrap()
                && file.verification_state == VerificationState::Verified
            {
                out.push(file);
            }
        }
        if out.len() == episodes.len() {
            return out;
        }
        assert!(std::time::Instant::now() < deadline, "never archived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Moves an archived file out of the media root and lets a check notice.
async fn lose(h: &Harness, file: &ArchiveFile, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::rename(h.media_dir().join(&file.relative_path), to).unwrap();
    let checked = h
        .engine
        .verify_episode(file.episode_id, VerifyDepth::Light)
        .await
        .unwrap();
    assert_eq!(checked.state, VerificationState::Missing);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_copies_exact_bytes_only() {
    let h = Harness::new().await;
    let episodes = library(&h, 2).await;
    let files = archived(&h, &episodes).await;
    let outside = tempfile::tempdir().unwrap();
    let backup = outside.path().join("backup");
    lose(&h, &files[0], &backup.join("kept/one.mp3")).await;
    // The second file's bytes are gone; a file of the same size with other
    // bytes sits in the folder and must not be taken for it.
    std::fs::rename(
        h.media_dir().join(&files[1].relative_path),
        outside.path().join("gone.mp3"),
    )
    .unwrap();
    h.engine
        .verify_episode(files[1].episode_id, VerifyDepth::Light)
        .await
        .unwrap();
    std::fs::write(
        backup.join("decoy.mp3"),
        vec![0u8; usize::try_from(files[1].size_bytes).unwrap()],
    )
    .unwrap();
    let before = tree_digest(&backup);

    let plan = h
        .engine
        .restore(&backup, &RestoreOptions::default())
        .await
        .unwrap();
    assert!(!plan.applied);
    let action = |id: EpisodeId, report: &uguisu_engine::restore::RestoreReport| {
        report
            .items
            .iter()
            .find(|i| i.episode_id == id)
            .unwrap()
            .action
    };
    assert_eq!(action(files[0].episode_id, &plan), RestoreAction::Restore);
    assert_eq!(action(files[1].episode_id, &plan), RestoreAction::NotFound);
    assert!(
        !h.media_dir().join(&files[0].relative_path).exists(),
        "a plan writes nothing"
    );

    let done = h
        .engine
        .restore(
            &backup,
            &RestoreOptions {
                apply: true,
                ..RestoreOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(action(files[0].episode_id, &done), RestoreAction::Restore);
    let restored = h
        .engine
        .archive_file(files[0].episode_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.verification_state, VerificationState::Verified);
    assert_eq!(restored.hash_value, files[0].hash_value);
    let missing = h
        .engine
        .archive_file(files[1].episode_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(missing.verification_state, VerificationState::Missing);
    assert_eq!(tree_digest(&backup), before, "the folder is only read");
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_never_overwrites_a_file() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let file = archived(&h, &episodes).await.remove(0);
    let outside = tempfile::tempdir().unwrap();
    let backup = outside.path().join("backup");
    lose(&h, &file, &backup.join("one.mp3")).await;
    // Something else appears at the record's path after the check.
    let target = h.media_dir().join(&file.relative_path);
    std::fs::write(&target, b"somebody else's file").unwrap();

    let done = h
        .engine
        .restore(
            &backup,
            &RestoreOptions {
                apply: true,
                ..RestoreOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(done.items[0].action, RestoreAction::Taken, "{done:?}");
    assert_eq!(std::fs::read(&target).unwrap(), b"somebody else's file");
    assert!(backup.join("one.mp3").is_file(), "the source is kept");
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_refuses_the_media_root() {
    let h = Harness::new().await;
    std::fs::create_dir_all(h.media_dir()).unwrap();
    let refused = h
        .engine
        .restore(&h.media_dir(), &RestoreOptions::default())
        .await;
    assert!(
        matches!(
            refused,
            Err(UguisuError::Archive {
                kind: ArchiveErrorKind::ImportSourceInvalid,
                ..
            })
        ),
        "{refused:?}"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redownload_refuses_a_present_file() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    archived(&h, &episodes).await;
    let refused = h
        .engine
        .downloads()
        .redownload(episodes[0].id, Priority::Normal)
        .await;
    assert!(
        matches!(refused, Err(UguisuError::Conflict(_))),
        "{refused:?}"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redownload_replaces_a_missing_file() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let file = archived(&h, &episodes).await.remove(0);
    let outside = tempfile::tempdir().unwrap();
    lose(&h, &file, &outside.path().join("elsewhere.mp3")).await;

    let outcome = h
        .engine
        .downloads()
        .redownload(episodes[0].id, Priority::Normal)
        .await
        .unwrap();
    assert!(outcome.queued(), "{outcome:?}");
    let again = settled(&h, &episodes).await.remove(0);
    assert_eq!(again.id, file.id, "the record's identity is kept");
    assert_eq!(again.hash_value, file.hash_value);
    assert!(h.media_dir().join(&again.relative_path).is_file());
    assert!(
        outside.path().join("elsewhere.mp3").is_file(),
        "nothing removes the moved file"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redownload_gives_an_import_a_job() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    // A record made by an import, which no download job stands behind.
    let outside = tempfile::tempdir().unwrap();
    let source = outside.path().join("podgrab");
    let show = source.join("Synthetic Show");
    std::fs::create_dir_all(&show).unwrap();
    let episode = &episodes[0];
    let date = episode.published_at.unwrap().date();
    let title = episode.title.replace([':', '/'], "-");
    std::fs::write(
        show.join(format!("{date} - {title}.mp3")),
        b"ID3\x04imported bytes",
    )
    .unwrap();
    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.counts.imported, 1, "{:?}", report.items);
    let imported = h.engine.archive_file(episode.id).await.unwrap().unwrap();
    assert_eq!(imported.origin, ArchiveOrigin::Import);
    lose(&h, &imported, &outside.path().join("moved.mp3")).await;

    let outcome = h
        .engine
        .downloads()
        .redownload(episode.id, Priority::Normal)
        .await
        .unwrap();
    assert!(outcome.queued(), "{outcome:?}");
    let file = settled(&h, &episodes).await.remove(0);
    assert_eq!(file.id, imported.id, "the record's identity is kept");
    assert_eq!(file.origin, ArchiveOrigin::Download);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paused_redownload_can_resume() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let file = archived(&h, &episodes).await.remove(0);
    let outside = tempfile::tempdir().unwrap();
    lose(&h, &file, &outside.path().join("elsewhere.mp3")).await;
    let job = h
        .engine
        .downloads()
        .redownload(episodes[0].id, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;

    h.engine.downloads().pause(job).await.unwrap();
    let resumed = h.engine.downloads().resume(job).await;
    assert!(resumed.is_ok(), "the record's file is gone: {resumed:?}");
    settled(&h, &episodes).await;
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redownload_keeps_a_leftover_part() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let file = archived(&h, &episodes).await.remove(0);
    let job = h
        .engine
        .downloads()
        .job_for_episode(file.episode_id)
        .await
        .unwrap()
        .unwrap();
    // A finalization that could not take the `.part` name back leaves a
    // second link to the archived bytes; then the archived name goes.
    let part = h.media_dir().join(&job.part_path);
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::hard_link(h.media_dir().join(&file.relative_path), &part).unwrap();
    let outside = tempfile::tempdir().unwrap();
    lose(&h, &file, &outside.path().join("elsewhere.mp3")).await;
    let before = std::fs::read(&part).unwrap();

    let refused = h
        .engine
        .downloads()
        .redownload(file.episode_id, Priority::Normal)
        .await;
    assert!(
        matches!(refused, Err(UguisuError::Conflict(_))),
        "{refused:?}"
    );
    assert_eq!(std::fs::read(&part).unwrap(), before, "the .part is kept");
    h.engine.close().await;
}

/// Reopens the engine on the same data directory with another template.
async fn use_template(h: &mut Harness, template: &str) {
    h.engine.close().await;
    let base = common::config(h.dir.path().to_path_buf(), FeedConfig::default());
    let archive = ArchiveConfig {
        template: template.to_owned(),
        ..base.archive.clone()
    };
    let placeholder = Engine::open(common::config(
        tempfile::tempdir().unwrap().keep(),
        FeedConfig::default(),
    ))
    .await
    .unwrap();
    drop(std::mem::replace(&mut h.engine, placeholder));
    h.engine = Engine::open(base.with_archive(archive)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_avoids_another_records_path() {
    let mut h = Harness::new().await;
    let episodes = library(&h, 2).await;
    let (wanted, imported) = (&episodes[0], &episodes[1]);
    let outside = tempfile::tempdir().unwrap();
    let show = outside.path().join("podgrab").join("Synthetic Show");
    std::fs::create_dir_all(&show).unwrap();
    let date = imported.published_at.unwrap().date();
    let title = imported.title.replace([':', '/'], "-");
    std::fs::write(
        show.join(format!("{date} - {title}.mp3")),
        b"ID3\x04imported bytes",
    )
    .unwrap();
    h.engine
        .import_apply(&outside.path().join("podgrab"), &ImportOptions::default())
        .await
        .unwrap();
    let record = h.engine.archive_file(imported.id).await.unwrap().unwrap();
    // A template under which the other episode renders the imported path,
    // which no download job targets.
    let literal = record.relative_path.replace(".mp3", ".{extension}");
    use_template(&mut h, &literal).await;

    h.engine
        .downloads()
        .enqueue_episode(wanted.id, Priority::Normal)
        .await
        .unwrap();
    let file = settled(&h, std::slice::from_ref(wanted)).await.remove(0);
    assert_ne!(file.relative_path, record.relative_path);
    assert_eq!(
        std::fs::read(h.media_dir().join(&record.relative_path)).unwrap(),
        b"ID3\x04imported bytes"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn returned_file_keeps_its_record() {
    let mut h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let file = archived(&h, &episodes).await.remove(0);
    let outside = tempfile::tempdir().unwrap();
    let kept = outside.path().join("kept.mp3");
    lose(&h, &file, &kept).await;
    use_template(&mut h, "{podcast.title}/{episode.title}.{extension}").await;
    h.engine
        .downloads()
        .redownload(file.episode_id, Priority::Normal)
        .await
        .unwrap();
    // The original comes back before the new download is registered.
    std::fs::copy(&kept, h.media_dir().join(&file.relative_path)).unwrap();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();

    let refused = h.engine.register_archive_file(file.episode_id).await;
    assert!(
        matches!(
            refused,
            Err(UguisuError::Archive {
                kind: ArchiveErrorKind::PathCollision,
                ..
            })
        ),
        "{refused:?}"
    );
    let record = h
        .engine
        .archive_file(file.episode_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.relative_path, file.relative_path);
    let job = h
        .engine
        .downloads()
        .job_for_episode(file.episode_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(job.target_path, file.relative_path);
    assert!(
        h.media_dir().join(&job.target_path).is_file(),
        "the new download stays"
    );
    h.engine.close().await;
}
