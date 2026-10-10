//! The archive through the engine: a finished download becomes a record,
//! verification tells the truth about what is on disk without changing it,
//! relocation moves files on request only, and a crash between the
//! completion transaction and the registration is repaired on the next
//! start.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::case_sensitive_file_extension_comparisons
)]

mod common;

use std::time::Duration;

use common::{Harness, synthetic_feed_with_media};
use uguisu_core::archive::{ArchivePolicy, PolicyMode, VerificationState, VerifyDepth, reason};
use uguisu_core::download::{DownloadState, Priority};
use uguisu_core::events::EventKind;
use uguisu_core::ids::EpisodeId;
use uguisu_core::model::ArchiveState;
use uguisu_http::CancellationToken;
use uguisu_storage::archive_files::{self, ArchiveFilter};
use uguisu_storage::downloads;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Serves a media-backed synthetic feed, adds it and downloads everything.
async fn archived_podcast(
    h: &Harness,
    count: usize,
) -> (uguisu_core::model::Podcast, Vec<EpisodeId>) {
    let body = synthetic_feed_with_media(count, h.media.base(), &["/normal/2048", "/range/4096"]);
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
        .downloads()
        .enqueue_podcast(podcast.id, Priority::Normal)
        .await
        .unwrap();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    let episodes = h
        .engine
        .episodes(podcast.id, None, 100)
        .await
        .unwrap()
        .episodes;
    let ids: Vec<EpisodeId> = episodes.iter().map(|e| e.id).collect();
    // The watcher archives after the download transaction commits, so give
    // it a moment to catch up rather than racing it.
    //
    // Waiting for the *record* is not enough: registration and verification
    // are two writes, and a caller that asserts `verified` after seeing the
    // row is racing the second one. That race is why this waits for the
    // state the tests actually assert — it showed up as a one-in-a-run
    // failure under load rather than as a reliable one.
    wait_for("every download to be archived and verified", || async {
        let mut reader = h.engine.storage().reader().await.unwrap();
        let filter = ArchiveFilter {
            state: Some(VerificationState::Verified),
            podcast_id: None,
            ..ArchiveFilter::default()
        };
        archive_files::list(&mut reader, &filter, None, 100)
            .await
            .unwrap()
            .len()
            == count
    })
    .await;
    (podcast, ids)
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

/// A hash finding about a file whose size and mtime did not change stays
/// until a full pass says otherwise: a light pass reads only those two.
#[tokio::test]
async fn light_pass_keeps_a_hash_finding() {
    let h = Harness::new().await;
    let (_podcast, episodes) = archived_podcast(&h, 1).await;
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let path = h.media_dir().join(&file.relative_path);
    let bytes = std::fs::read(&path).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let rewrite = |data: &[u8]| {
        std::fs::write(&path, data).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    };
    let (engine, episode) = (&h.engine, episodes[0]);
    let state = |depth| async move { engine.verify_episode(episode, depth).await.unwrap().state };

    let mut rotted = bytes.clone();
    *rotted.last_mut().unwrap() ^= 0xFF;
    rewrite(&rotted);
    assert_eq!(
        state(VerifyDepth::Light).await,
        VerificationState::Verified,
        "a light pass cannot see it"
    );
    assert_eq!(state(VerifyDepth::Full).await, VerificationState::Invalid);
    assert_eq!(
        state(VerifyDepth::Light).await,
        VerificationState::Invalid,
        "nor clear it"
    );

    let held = engine
        .verify_episode(episode, VerifyDepth::Light)
        .await
        .unwrap();
    assert_eq!(
        held.detail.as_deref(),
        Some("only a full pass clears this finding")
    );

    rewrite(&bytes);
    assert_eq!(state(VerifyDepth::Light).await, VerificationState::Invalid);
    assert_eq!(state(VerifyDepth::Full).await, VerificationState::Verified);
}

/// Only a change of state is announced: a file found intact again is not
/// news, while a finding and its repair both are.
#[tokio::test]
async fn unchanged_verification_is_not_announced() {
    let h = Harness::new().await;
    let (_podcast, episodes) = archived_podcast(&h, 1).await;
    let announced = |sub: &mut uguisu_engine::events::Subscription| {
        let mut kinds = Vec::new();
        while let Some(event) = sub.try_recv() {
            if matches!(
                event.kind,
                EventKind::ArchiveVerified { .. } | EventKind::ArchiveInvalid { .. }
            ) {
                kinds.push(event.kind.name());
            }
        }
        kinds
    };
    let mut sub = h.engine.subscribe();

    let summary = h
        .engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(summary.verified, 1);
    assert_eq!(announced(&mut sub), Vec::<&str>::new(), "still intact");

    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let path = h.media_dir().join(&file.relative_path);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
    h.engine
        .verify_episode(episodes[0], VerifyDepth::Light)
        .await
        .unwrap();
    std::fs::write(&path, &bytes).unwrap();
    h.engine
        .verify_episode(episodes[0], VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(
        announced(&mut sub),
        ["archive.invalid", "archive.verified"],
        "the finding and its repair"
    );
}

#[tokio::test]
async fn a_finished_download_becomes_a_record() {
    let h = Harness::new().await;
    let mut sub = h.engine.subscribe();
    let (podcast, episodes) = archived_podcast(&h, 3).await;

    let mut registered = 0;
    let mut verified = 0;
    while let Some(event) = sub.try_recv() {
        match event.kind {
            EventKind::ArchiveRegistered { size_bytes, .. } => {
                assert!(size_bytes > 0);
                registered += 1;
            }
            EventKind::ArchiveVerified { .. } => verified += 1,
            _ => {}
        }
    }
    assert_eq!(registered, 3, "one registration per episode");
    assert!(verified >= 1, "the default configuration verifies");

    for episode_id in &episodes {
        let file = h.engine.archive_file(*episode_id).await.unwrap().unwrap();
        assert_eq!(file.podcast_id, podcast.id);
        assert_eq!(file.hash_algo, "sha256");
        assert_eq!(file.hash_value.len(), 64);
        assert_eq!(file.verification_state, VerificationState::Verified);
        assert!(h.media_dir().join(&file.relative_path).is_file());
    }

    let counts = h.engine.archive_counts().await.unwrap();
    assert_eq!(counts.get(&VerificationState::Verified), Some(&3));

    // Registering again converges on the same record rather than a second.
    let first = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let again = h.engine.register_archive_file(episodes[0]).await.unwrap();
    assert_eq!(again.id, first.id);
    assert_eq!(again.registered_at, first.registered_at);
    let listed = h
        .engine
        .archive_list(&ArchiveFilter::default(), None, 100)
        .await
        .unwrap();
    assert_eq!(listed.files.len(), 3, "no duplicates");
    h.engine.close().await;
}

#[tokio::test]
async fn verification_reports_and_changes_nothing() {
    let h = Harness::new().await;
    let (_, episodes) = archived_podcast(&h, 3).await;
    let media = h.media_dir();

    let intact = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let tampered = h.engine.archive_file(episodes[1]).await.unwrap().unwrap();
    let deleted = h.engine.archive_file(episodes[2]).await.unwrap().unwrap();

    // Same length, different bytes: only a full pass can see it.
    let tampered_path = media.join(&tampered.relative_path);
    let original = std::fs::read(&tampered_path).unwrap();
    let mut edited = original.clone();
    edited[0] ^= 0xFF;
    std::fs::write(&tampered_path, &edited).unwrap();
    let edited_at = std::time::UNIX_EPOCH
        + Duration::from_secs(u64::try_from(tampered.mtime_unix.unwrap()).unwrap() + 3600);
    std::fs::File::options()
        .write(true)
        .open(&tampered_path)
        .unwrap()
        .set_modified(edited_at)
        .unwrap();
    std::fs::remove_file(media.join(&deleted.relative_path)).unwrap();

    // The size is unchanged, the mtime is not: a light pass cannot vouch
    // for it, and asking again does not make it trust the size.
    for _ in 0..2 {
        let light = h
            .engine
            .verify_episode(episodes[1], VerifyDepth::Light)
            .await
            .unwrap();
        assert_eq!(light.state, VerificationState::Unchecked);
        assert_eq!(light.reason, reason::MTIME_CHANGED);
    }

    let summary = h
        .engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(summary.checked, 3);
    assert_eq!(summary.verified, 1);
    assert_eq!(summary.invalid, 1);
    assert_eq!(summary.missing, 1);
    assert!(summary.has_problems());

    let after = h.engine.archive_file(episodes[1]).await.unwrap().unwrap();
    assert_eq!(after.verification_state, VerificationState::Invalid);
    assert_eq!(
        after.verification_reason.as_deref(),
        Some(reason::HASH_MISMATCH)
    );
    assert_eq!(
        after.hash_value, tampered.hash_value,
        "the recorded hash is the download's, not the tampered file's"
    );
    assert_eq!(
        std::fs::read(&tampered_path).unwrap(),
        edited,
        "verification must not rewrite the file"
    );
    let light = h
        .engine
        .verify_episode(episodes[1], VerifyDepth::Light)
        .await
        .unwrap();
    assert_eq!(
        light.state,
        VerificationState::Invalid,
        "a light pass cannot clear a hash mismatch"
    );

    let gone = h.engine.archive_file(episodes[2]).await.unwrap().unwrap();
    assert_eq!(gone.verification_state, VerificationState::Missing);
    assert_eq!(gone.hash_value, deleted.hash_value, "the record survives");

    let still_fine = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(still_fine.verification_state, VerificationState::Verified);
    assert_eq!(still_fine.hash_value, intact.hash_value);

    // The episode view agrees with the archive.
    let episodes_now = h
        .engine
        .episodes(still_fine.podcast_id, None, 100)
        .await
        .unwrap()
        .episodes;
    let state_of = |id: EpisodeId| {
        episodes_now
            .iter()
            .find(|e| e.id == id)
            .unwrap()
            .archive_state
    };
    assert_eq!(state_of(episodes[0]), ArchiveState::Archived);
    assert_eq!(state_of(episodes[1]), ArchiveState::Modified);
    assert_eq!(state_of(episodes[2]), ArchiveState::Missing);

    // A filtered scan only touches what it selects.
    let only_missing = h
        .engine
        .verify_all(
            &ArchiveFilter {
                state: Some(VerificationState::Missing),
                podcast_id: None,
                ..ArchiveFilter::default()
            },
            VerifyDepth::Light,
        )
        .await
        .unwrap();
    assert_eq!(only_missing.checked, 1);
    assert_eq!(only_missing.missing, 1);
    h.engine.close().await;
}

#[tokio::test]
async fn preview_creates_nothing_relocation_moves() {
    let h = Harness::new().await;
    let (_, episodes) = archived_podcast(&h, 2).await;
    let media = h.media_dir();
    let episode_id = episodes[0];

    let before = h.engine.archive_file(episode_id).await.unwrap().unwrap();
    let preview = h.engine.path_preview(episode_id).await.unwrap();
    assert_eq!(
        preview.current.as_deref(),
        Some(before.relative_path.as_str())
    );
    assert!(
        preview.resolved.starts_with("Synthetic Show/"),
        "the template renders a readable path: {}",
        preview.resolved
    );
    assert!(preview.resolved.ends_with(".mp3"), "{}", preview.resolved);
    // A new download already lands on the template path, so there is
    // nothing to move and the preview says so.
    assert_eq!(preview.resolved, before.relative_path);
    assert!(!preview.would_move);
    let dry = h.engine.relocate(episode_id, true).await.unwrap();
    assert!(!dry.moved);
    assert_eq!(dry.from, dry.to);

    // An artifact that predates the template — a file at the
    // identifier path, downloaded by a job that aimed there — is what
    // relocation is actually for.
    let legacy = format!("{}/{episode_id}.mp3", before.podcast_id);
    let bytes = std::fs::read(media.join(&before.relative_path)).unwrap();
    std::fs::create_dir_all(media.join(before.podcast_id.to_string())).unwrap();
    std::fs::rename(media.join(&before.relative_path), media.join(&legacy)).unwrap();
    {
        let now = time::OffsetDateTime::now_utc();
        let mut tx = h.engine.storage().begin().await.unwrap();
        assert!(
            archive_files::set_path(&mut tx, before.id, &legacy, None, now)
                .await
                .unwrap()
        );
        assert!(
            downloads::set_target_path(&mut tx, episode_id, &legacy, now)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
    }

    let preview = h.engine.path_preview(episode_id).await.unwrap();
    assert_eq!(preview.current.as_deref(), Some(legacy.as_str()));
    assert!(preview.would_move, "the template path is not where it lies");
    assert!(
        !media.join(&preview.resolved).exists(),
        "a preview creates nothing"
    );
    assert_eq!(
        h.engine
            .archive_file(episode_id)
            .await
            .unwrap()
            .unwrap()
            .relative_path,
        legacy,
        "a preview changes no record either"
    );

    let dry = h.engine.relocate(episode_id, true).await.unwrap();
    assert_eq!(dry.to, preview.resolved);
    assert!(!dry.moved);
    assert!(media.join(&legacy).is_file(), "a dry run moves nothing");
    assert!(!media.join(&preview.resolved).exists());

    let moved = h.engine.relocate(episode_id, false).await.unwrap();
    assert!(moved.moved);
    assert_eq!(moved.from, legacy);
    assert_eq!(moved.to, preview.resolved);
    assert!(!media.join(&legacy).exists(), "renamed, not copied");
    assert_eq!(
        std::fs::read(media.join(&preview.resolved)).unwrap(),
        bytes,
        "the bytes are untouched"
    );

    let after = h.engine.archive_file(episode_id).await.unwrap().unwrap();
    assert_eq!(after.relative_path, preview.resolved);
    assert_eq!(after.hash_value, before.hash_value);
    assert_eq!(before.verification_state, VerificationState::Verified);
    assert_eq!(after.verification_state, VerificationState::Verified);
    assert_eq!(
        after.verification_reason.as_deref(),
        Some(reason::RELOCATED)
    );
    assert_eq!(after.id, before.id);

    // The job followed the file, so a deep reconcile finds it.
    let job = h
        .engine
        .downloads()
        .job_for_episode(episode_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.target_path, moved.to);
    let reconciled = h.engine.downloads().reconcile(true).await.unwrap();
    assert_eq!(reconciled.missing_targets, 0, "{reconciled:?}");
    assert_eq!(
        h.engine
            .episode(episode_id)
            .await
            .unwrap()
            .episode
            .archive_state,
        ArchiveState::Archived
    );

    // It still verifies at the new path, and moving again is a no-op.
    let verified = h
        .engine
        .verify_episode(episode_id, VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(verified.state, VerificationState::Verified);
    let again = h.engine.relocate(episode_id, false).await.unwrap();
    assert!(!again.moved);
    assert_eq!(again.from, again.to);
    h.engine.close().await;
}

#[tokio::test]
async fn one_path_two_episodes_stay_distinct() {
    let h = Harness::new().await;
    // Two items with the same title and date render to one path.
    let body = String::from_utf8(synthetic_feed_with_media(
        2,
        h.media.base(),
        &["/normal/2048"],
    ))
    .unwrap()
    // Same title *and* the same date, which is what actually collides.
    .replace(
        "Episode 1: the one about &amp; things",
        "Episode 0: the one about &amp; things",
    )
    .replace("02 Jan 2020", "01 Jan 2020");
    Mock::given(method("GET"))
        .and(path("/media.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body.into_bytes())
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
        .downloads()
        .enqueue_podcast(podcast.id, Priority::Normal)
        .await
        .unwrap();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    wait_for("both downloads to be archived", || async {
        let mut reader = h.engine.storage().reader().await.unwrap();
        archive_files::list(&mut reader, &ArchiveFilter::default(), None, 10)
            .await
            .unwrap()
            .len()
            == 2
    })
    .await;

    let episodes: Vec<EpisodeId> = h
        .engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect();
    let first = h.engine.relocate(episodes[0], false).await.unwrap();
    let second = h.engine.relocate(episodes[1], false).await.unwrap();
    assert_ne!(first.to, second.to, "one path cannot hold two episodes");
    assert!(
        second.to.contains('[') || first.to.contains('['),
        "the newcomer gets the stable suffix: {} / {}",
        first.to,
        second.to
    );
    assert!(h.media_dir().join(&first.to).is_file());
    assert!(h.media_dir().join(&second.to).is_file());

    // The choice does not depend on the order things happened in.
    let preview = h.engine.path_preview(episodes[1]).await.unwrap();
    assert_eq!(preview.resolved, second.to);
    h.engine.close().await;
}

#[tokio::test]
async fn a_crash_before_registration_is_repaired() {
    let mut h = Harness::new().await;
    let (_, episodes) = archived_podcast(&h, 2).await;
    let media = h.media_dir();

    // Simulate the crash window: the download committed, the registration
    // did not. Removing the records leaves exactly that state.
    let paths: Vec<String> = {
        let mut tx = h.engine.storage().begin().await.unwrap();
        let mut paths = Vec::new();
        for id in &episodes {
            let file = archive_files::get_by_episode(&mut tx, *id)
                .await
                .unwrap()
                .unwrap();
            paths.push(file.relative_path);
            assert!(
                archive_files::delete_for_episode(&mut tx, *id)
                    .await
                    .unwrap()
            );
        }
        tx.commit().await.unwrap();
        paths
    };
    for p in &paths {
        assert!(media.join(p).is_file(), "the files are still there");
    }

    h.restart(None).await;

    for (episode_id, expected) in episodes.iter().zip(&paths) {
        let file = h.engine.archive_file(*episode_id).await.unwrap().unwrap();
        assert_eq!(&file.relative_path, expected, "registered where it lies");
        assert_eq!(file.hash_value.len(), 64);
        // Startup is shallow: it confirms the file is there and says no
        // more. A `stat` cannot claim the bytes are right, so the record
        // stays `unchecked` until something actually reads it.
        assert_eq!(file.verification_state, VerificationState::Unchecked);
    }

    // And asking properly does establish it.
    let summary = h
        .engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(summary.verified, 2);
    assert!(!summary.has_problems());

    // A finding survives a restart: a shallow pass must not clear it.
    let tampered = h.engine.archive_file(episodes[1]).await.unwrap().unwrap();
    let path = media.join(&tampered.relative_path);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
    let invalid = h
        .engine
        .verify_episode(episodes[1], VerifyDepth::Light)
        .await
        .unwrap();
    assert_eq!(invalid.state, VerificationState::Invalid);
    h.restart(None).await;
    assert_eq!(
        h.engine
            .archive_file(episodes[1])
            .await
            .unwrap()
            .unwrap()
            .verification_state,
        VerificationState::Invalid,
        "a startup stat must not overwrite a finding"
    );
    std::fs::write(&path, &bytes).unwrap();

    // A start looks at no file; the check a server starts finds the one
    // that is gone, and nothing is deleted.
    std::fs::remove_file(media.join(&paths[0])).unwrap();
    h.restart(None).await;
    let state = || async {
        h.engine
            .archive_file(episodes[0])
            .await
            .unwrap()
            .unwrap()
            .verification_state
    };
    assert_eq!(
        state().await,
        VerificationState::Verified,
        "open stats nothing"
    );
    h.engine.start_archive_check();
    wait_for("the check to find the missing file", || async {
        state().await == VerificationState::Missing
    })
    .await;
    assert!(
        h.engine.archive_file(episodes[0]).await.unwrap().is_some(),
        "a missing file never removes its record"
    );
    h.engine.close().await;
}

#[tokio::test]
async fn changed_enclosure_is_flagged() {
    let h = Harness::new().await;
    let (podcast, _) = archived_podcast(&h, 2).await;
    let all = || async {
        h.engine
            .archive_list(&ArchiveFilter::default(), None, 100)
            .await
            .unwrap()
            .files
    };
    let before = all().await;
    assert!(before.iter().all(|f| f.source_changed_at.is_none()));

    // The first item now points at another file; the second is unchanged.
    h.server.reset().await;
    Mock::given(method("GET"))
        .and(path("/media.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(synthetic_feed_with_media(
                    2,
                    h.media.base(),
                    &["/normal/3072", "/range/4096"],
                ))
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    let mut sub = h.engine.subscribe();
    h.engine
        .refresh_podcast(podcast.id, uguisu_engine::RefreshOptions::default())
        .await
        .unwrap();

    let mut changed = Vec::new();
    while let Some(event) = sub.try_recv() {
        if let EventKind::ArchiveSourceChanged { new_url, .. } = event.kind {
            changed.push(new_url);
        }
    }
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert!(changed[0].ends_with("/normal/3072"), "{changed:?}");

    let flagged = h
        .engine
        .archive_list(
            &ArchiveFilter {
                source_changed: true,
                ..ArchiveFilter::default()
            },
            None,
            100,
        )
        .await
        .unwrap()
        .files;
    assert_eq!(flagged.len(), 1);
    let was = before.iter().find(|f| f.id == flagged[0].id).unwrap();
    assert_eq!(flagged[0].hash_value, was.hash_value, "the file is kept");
    assert!(h.media_dir().join(&flagged[0].relative_path).is_file());
    let job = h
        .engine
        .downloads()
        .job_for_episode(flagged[0].episode_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        job.state,
        uguisu_core::download::DownloadState::Completed,
        "nothing is downloaded again"
    );
    h.engine.close().await;
}

#[tokio::test]
async fn a_missing_archive_file_is_reported() {
    let h = Harness::new().await;
    let podcast = h.add_fixture("minimal_rss.xml").await.podcast;
    let episode_id = h
        .engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes[0]
        .id;

    assert!(h.engine.archive_file(episode_id).await.unwrap().is_none());
    let err = h
        .engine
        .verify_episode(episode_id, VerifyDepth::Light)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "archive_not_found", "{err}");
    let err = h.engine.relocate(episode_id, true).await.unwrap_err();
    assert_eq!(err.kind(), "archive_not_found", "{err}");

    // A preview works without a record: that is what makes it useful
    // before anything has been downloaded.
    let preview = h.engine.path_preview(episode_id).await.unwrap();
    assert!(preview.current.is_none());
    assert!(!preview.would_move);
    assert!(!preview.resolved.is_empty());
    h.engine.close().await;
}

#[tokio::test]
async fn the_policy_is_off_then_queues() {
    let h = Harness::new().await;
    let body = synthetic_feed_with_media(4, h.media.base(), &["/normal/2048"]);
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

    // Adding a podcast discovers four episodes and queues none of them.
    let stats = h.engine.downloads().stats().await.unwrap();
    assert!(stats.by_state.is_empty(), "off by default: {stats:?}");
    assert_eq!(
        h.engine.effective_policy(podcast.id).await.unwrap().mode,
        PolicyMode::Manual
    );

    // Opting this one podcast in, on an installation that is opted out.
    h.engine
        .set_policy(&ArchivePolicy {
            podcast_id: podcast.id,
            mode: PolicyMode::Auto,
            max_backlog: Some(2),
            max_age_days: None,
            priority: Some(Priority::High),
            updated_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
    let resolved = h.engine.effective_policy(podcast.id).await.unwrap();
    assert_eq!(resolved.mode, PolicyMode::Auto);
    assert_eq!(resolved.max_backlog, 2);

    let episodes: Vec<EpisodeId> = h
        .engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect();
    let outcome = h.engine.apply_policy(podcast.id, &episodes).await.unwrap();
    assert_eq!(outcome.queued, 2, "the backlog limit holds: {outcome:?}");
    assert_eq!(
        outcome
            .reasons
            .get(uguisu_core::archive::policy_reason::BACKLOG_EXCEEDED),
        Some(&2)
    );

    // Running it again — as three more refreshes would — queues nothing
    // more: the backlog limit counts what is already outstanding, so a
    // repeated refresh cannot top the queue up past it.
    for _ in 0..3 {
        let again = h.engine.apply_policy(podcast.id, &episodes).await.unwrap();
        assert_eq!(again.queued, 0, "{again:?}");
    }
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
    assert_eq!(jobs.jobs.len(), 2, "one job per episode, ever");
    assert!(jobs.jobs.iter().all(|j| j.job.priority == Priority::High));

    // A manual command is never blocked by the policy.
    let manual = h
        .engine
        .downloads()
        .enqueue_podcast(podcast.id, Priority::Normal)
        .await
        .unwrap();
    assert_eq!(manual.created, 2, "the two the policy passed over");

    // Clearing the policy puts the podcast back on the global defaults.
    assert!(h.engine.clear_policy(podcast.id).await.unwrap());
    assert_eq!(
        h.engine.effective_policy(podcast.id).await.unwrap().mode,
        PolicyMode::Manual
    );
    h.engine.close().await;
}

#[tokio::test]
async fn three_refreshes_queue_each_episode_once() {
    let h = Harness::new().await;
    h.engine
        .set_setting("UGUISU_ARCHIVE_AUTO_DOWNLOAD", "true", Some("test"))
        .await
        .unwrap();
    let feed = |count| synthetic_feed_with_media(count, h.media.base(), &["/normal/2048"]);
    h.serve("/media.xml", &feed(2), None, None).await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/media.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast;

    // One new item, then one renamed under the same guid, then nothing new.
    let renamed = String::from_utf8(feed(3))
        .unwrap()
        .replace("Episode 1: the one", "Episode 1 (remastered): the one");
    for body in [feed(3), renamed.clone().into_bytes(), renamed.into_bytes()] {
        h.reset().await;
        h.serve("/media.xml", &body, None, None).await;
        let r = h.refresh(podcast.id, true).await;
        assert_eq!(
            r.outcome,
            uguisu_core::feed::RefreshOutcome::Fetched,
            "{r:?}"
        );
    }

    let jobs = h
        .engine
        .downloads()
        .list(&uguisu_download::JobFilter {
            podcast_id: Some(podcast.id),
            limit: 50,
            ..uguisu_download::JobFilter::default()
        })
        .await
        .unwrap()
        .jobs;
    let mut episodes: Vec<EpisodeId> = jobs.iter().map(|j| j.job.episode_id).collect();
    episodes.sort();
    episodes.dedup();
    assert_eq!((jobs.len(), episodes.len()), (3, 3), "{jobs:#?}");
    h.engine.close().await;
}

/// Whether names that differ only in case are one name in `dir`.
fn folds_case(dir: &std::path::Path) -> bool {
    std::fs::create_dir_all(dir).unwrap();
    let probe = dir.join("case-probe");
    std::fs::write(&probe, b"").unwrap();
    let folded = dir.join("CASE-PROBE").exists();
    std::fs::remove_file(probe).unwrap();
    folded
}

#[tokio::test]
async fn case_variant_titles_never_overwrite() {
    let h = Harness::new().await;
    // One date, two titles that differ only in case, two different files.
    let body = String::from_utf8(synthetic_feed_with_media(
        2,
        h.media.base(),
        &["/normal/2048", "/normal/3072"],
    ))
    .unwrap()
    .replace(
        "Episode 1: the one about &amp; things",
        "EPISODE 0: THE ONE ABOUT &amp; THINGS",
    )
    .replace("02 Jan 2020", "01 Jan 2020");
    Mock::given(method("GET"))
        .and(path("/media.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body.into_bytes())
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
        .downloads()
        .enqueue_podcast(podcast.id, Priority::Normal)
        .await
        .unwrap();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();

    let jobs = || async {
        let mut jobs = Vec::new();
        for episode in h
            .engine
            .episodes(podcast.id, None, 10)
            .await
            .unwrap()
            .episodes
        {
            jobs.push(
                h.engine
                    .downloads()
                    .job_for_episode(episode.id)
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        jobs
    };
    let first = jobs().await;
    let completed = |jobs: &[uguisu_core::download::DownloadJob]| {
        jobs.iter()
            .filter(|j| j.state == DownloadState::Completed)
            .count()
    };
    if folds_case(&h.media_dir()) {
        // NTFS and APFS: the two targets are one name. Neither download
        // replaces the other; the later one stops at the occupied name, and
        // a retry gives it another.
        assert_eq!(completed(&first), 1, "{first:#?}");
        let failed = first
            .iter()
            .find(|j| j.state == DownloadState::Failed)
            .unwrap();
        assert_eq!(
            failed.state_reason.as_deref(),
            Some("target_exists"),
            "{failed:#?}"
        );
        h.engine.downloads().retry(failed.id).await.unwrap();
        h.engine.downloads().wait_idle().await.unwrap();
    }
    let finished = jobs().await;
    assert_eq!(completed(&finished), 2, "{finished:#?}");
    for job in &finished {
        wait_for("the download to be archived", || async {
            h.engine
                .archive_file(job.episode_id)
                .await
                .unwrap()
                .is_some()
        })
        .await;
        let checked = h
            .engine
            .verify_episode(job.episode_id, VerifyDepth::Full)
            .await
            .unwrap();
        assert_eq!(checked.state, VerificationState::Verified, "{checked:?}");
    }
    h.engine.close().await;
}
