//! Doing everything twice, and picking up after a stop.
//!
//! Every Phase-6 operation is something a user, a script or a restart can
//! run again, so every one of them has to be safe to repeat: a second run
//! must reach the same state and, where the state is a file, must not
//! move its hash. These tests run each operation twice and compare, and
//! then put the archive into the state a crash would have left and check
//! that the next start settles it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;

use std::time::Duration;

use common::{Harness, synthetic_feed_with_media};
use uguisu_archive::layout;
use uguisu_archive::path::RelativePath;
use uguisu_core::archive::{VerificationState, VerifyDepth};
use uguisu_core::download::Priority;
use uguisu_core::ids::EpisodeId;
use uguisu_engine::rebuild::RebuildOptions;
use uguisu_http::CancellationToken;
use uguisu_storage::archive_files::{self, ArchiveFilter};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn archived(h: &Harness, count: usize) -> Vec<EpisodeId> {
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
    wait_for("every artifact and its sidecar", || async {
        let mut reader = h.engine.storage().reader().await.unwrap();
        let all = archive_files::list(&mut reader, &ArchiveFilter::default(), None, 100)
            .await
            .unwrap();
        all.len() == count && all.iter().all(|f| f.sidecar_written_at.is_some())
    })
    .await;
    h.engine
        .episodes(podcast.id, None, 100)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect()
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

/// Every file under the media directory, with its hash.
fn snapshot(root: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    use sha2::{Digest, Sha256};
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    hex::encode(Sha256::digest(&bytes)),
                );
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_operation_is_safe_to_run_twice() {
    let h = Harness::new().await;
    let episodes = archived(&h, 3).await;
    h.engine.write_stale_manifests().await.unwrap();

    // The state after one of everything.
    let first_files: Vec<_> = {
        let mut out = Vec::new();
        for id in &episodes {
            out.push(h.engine.archive_file(*id).await.unwrap().unwrap());
        }
        out
    };
    let first_disk = snapshot(&h.media_dir());

    // ...and after a second of everything.
    for id in &episodes {
        h.engine.write_sidecar(*id).await.unwrap();
    }
    h.engine.write_stale_manifests().await.unwrap();
    h.engine.write_pending_sidecars(100).await.unwrap();
    let rebuild = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert_eq!(
        rebuild.unchanged, 3,
        "a rebuild over an archive that is already right changes nothing: {rebuild:?}"
    );
    assert_eq!(rebuild.rebuilt, 0);
    h.engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Full)
        .await
        .unwrap();

    for (before, id) in first_files.iter().zip(episodes.iter()) {
        let after = h.engine.archive_file(*id).await.unwrap().unwrap();
        assert_eq!(after.id, before.id, "the record kept its identity");
        assert_eq!(after.relative_path, before.relative_path);
        assert_eq!(after.hash_value, before.hash_value);
        assert_eq!(after.size_bytes, before.size_bytes);
        assert_eq!(after.source_hash_value, before.source_hash_value);
        assert_eq!(after.origin, before.origin);
    }

    // The media and the manifest are byte-identical; only the sidecars
    // carry a new timestamp, which is the one thing that is supposed to
    // move.
    let second_disk = snapshot(&h.media_dir());
    assert_eq!(
        first_disk.keys().collect::<Vec<_>>(),
        second_disk.keys().collect::<Vec<_>>(),
        "no file appeared or disappeared"
    );
    for (path, hash) in &first_disk {
        if std::path::Path::new(path)
            .extension()
            .is_some_and(|e| e == "json")
        {
            continue;
        }
        assert_eq!(
            second_disk.get(path),
            Some(hash),
            "{path} changed on a repeat run"
        );
    }
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_finishes_what_was_left_undone() {
    let mut h = Harness::new().await;
    let episodes = archived(&h, 2).await;

    // The state a stop between the registration and the sidecar leaves:
    // a record that says it has none, and no document on disk.
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let sidecar_path = h
        .media_dir()
        .join(layout::sidecar_of(&RelativePath::parse(&file.relative_path).unwrap()).as_str());
    std::fs::remove_file(&sidecar_path).unwrap();
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        let mut cleared = file.clone();
        cleared.sidecar_written_at = None;
        archive_files::upsert(&mut w, &cleared).await.unwrap();
    }
    // ...and a manifest that was never written at all.
    assert!(h.engine.manifest_status().await.unwrap()[0].stale);

    h.restart(None).await;

    // Startup does not do this work - it must stay fast - so the state is
    // still as it was, and it is the first idle flush that settles it.
    assert!(!sidecar_path.is_file(), "startup writes no sidecars");
    h.engine.flush_archive_metadata().await;
    assert!(
        sidecar_path.is_file(),
        "the flush wrote the missing sidecar"
    );
    assert!(
        !h.engine.manifest_status().await.unwrap()[0].stale,
        "and the manifest with it"
    );
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert!(after.sidecar_written_at.is_some());
    assert_eq!(
        after.hash_value, file.hash_value,
        "and nothing about the artifact changed"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leftovers_from_an_interrupted_run_are_kept_and_never_cleaned_up() {
    // Phase 6 deletes nothing - not the user's files, and not its own
    // scratch. A leftover is something to report, because a scratch file
    // that was silently removed is a copy of someone's media that nobody
    // gets to look at.
    let mut h = Harness::new().await;
    let episodes = archived(&h, 1).await;

    let tmp = h.media_dir().join(".uguisu").join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let leftovers = [
        tmp.join("01J0000000000000000000000A.import"),
        tmp.join("01J0000000000000000000000B.tagtmp"),
    ];
    for path in &leftovers {
        std::fs::write(path, b"half a copy").unwrap();
    }
    // A sidecar for an episode nobody has, and a manifest nobody wrote.
    let stray = h.media_dir().join("Unknown Show").join("mystery.mp3.json");
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, b"{\"schema\":1}").unwrap();

    h.restart(None).await;
    h.engine.flush_archive_metadata().await;
    let report = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();

    for path in &leftovers {
        assert!(
            path.is_file(),
            "{} was removed; nothing in Phase 6 may delete",
            path.display()
        );
    }
    assert!(
        stray.is_file(),
        "an unreadable document is reported, not removed"
    );
    assert!(
        report.malformed.count >= 1,
        "...and it is reported: {report:?}"
    );
    // The real artifact is untouched by any of it.
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert!(h.media_dir().join(&file.relative_path).is_file());
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_converge_instead_of_corrupting() {
    let h = Harness::new().await;
    let episodes = archived(&h, 3).await;
    let podcast = h
        .engine
        .archive_file(episodes[0])
        .await
        .unwrap()
        .unwrap()
        .podcast_id;

    // Four writers for one manifest and four for one sidecar, at once.
    // The file is written through a rename either way, so a reader can
    // only ever see one whole document.
    let mut tasks: Vec<tokio::task::JoinHandle<Result<(), uguisu_core::UguisuError>>> = Vec::new();
    for _ in 0..4 {
        let engine = h.engine.clone();
        tasks.push(tokio::spawn(async move {
            engine.write_manifest(podcast).await.map(|_| ())
        }));
        let engine = h.engine.clone();
        let id = episodes[0];
        tasks.push(tokio::spawn(async move {
            engine.write_sidecar(id).await.map(|_| ())
        }));
    }
    for task in tasks {
        task.await.unwrap().expect("no writer failed");
    }

    let status = h.engine.manifest_status().await.unwrap();
    assert_eq!(status.len(), 1);
    let on_disk = h.media_dir().join(&status[0].relative_path);
    let text = std::fs::read_to_string(&on_disk).unwrap();
    assert_eq!(
        uguisu_archive::manifest::parse(&text).unwrap().len(),
        3,
        "the manifest is one whole document, not two interleaved"
    );
    let sidecar = h.engine.read_sidecar(episodes[0]).await.unwrap().unwrap();
    assert_eq!(sidecar.episode.id, episodes[0]);

    // And no scratch file survived the race: eight writers, eight
    // renames, nothing left over.
    let leftovers: Vec<String> = std::fs::read_dir(on_disk.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e == "tmp")
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rebuilt_archive_needs_a_real_check_before_it_claims_anything() {
    // The whole Phase-6 promise in one test: lose the index, rebuild it
    // from disk, and what comes back is honest about not having been
    // checked - until something checks it.
    let h = Harness::new().await;
    let episodes = archived(&h, 2).await;
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        for id in &episodes {
            archive_files::delete_for_episode(&mut w, *id)
                .await
                .unwrap();
        }
    }

    h.engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    for id in &episodes {
        let file = h.engine.archive_file(*id).await.unwrap().unwrap();
        assert_eq!(file.verification_state, VerificationState::Unchecked);
    }

    // One file was quietly edited while the database was gone. The
    // rebuild could not know - and the check does.
    let tampered = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    std::fs::write(
        h.media_dir().join(&tampered.relative_path),
        b"not the bytes the sidecar describes",
    )
    .unwrap();

    let summary = h
        .engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(summary.verified, 1, "{summary:?}");
    assert_eq!(summary.invalid, 1, "{summary:?}");
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.verification_state, VerificationState::Invalid);
    assert!(
        h.media_dir().join(&after.relative_path).is_file(),
        "and the file it complained about is still there"
    );
    h.engine.close().await;
}
