//! `reconcile --rebuild`: the database is an index, and this is what puts
//! it back when the index is lost but the archive is not.
//!
//! The property every test here is about: a rebuilt record says what the
//! sidecar said, and never that the bytes were checked. Only an explicit
//! verification may write `verified`, and a record that already carries a
//! checked finding is never overwritten by a document claiming otherwise.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;

use std::time::Duration;

use common::{Harness, synthetic_feed_with_media};
use uguisu_archive::layout;
use uguisu_archive::path::RelativePath;
use uguisu_core::archive::{ArchiveOrigin, VerificationState, VerifyDepth, reason};
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
    let ids: Vec<EpisodeId> = h
        .engine
        .episodes(podcast.id, None, 100)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect();
    wait_for("every artifact to have a sidecar", || async {
        let mut reader = h.engine.storage().reader().await.unwrap();
        let all = archive_files::list(&mut reader, &ArchiveFilter::default(), None, 100)
            .await
            .unwrap();
        all.len() == count && all.iter().all(|f| f.sidecar_written_at.is_some())
    })
    .await;
    ids
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

async fn forget_records(h: &Harness, episodes: &[EpisodeId]) {
    let mut w = h.engine.storage().writer().await.unwrap();
    for id in episodes {
        archive_files::delete_for_episode(&mut w, *id)
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_index_is_rebuilt_from_the_sidecars_but_never_as_verified() {
    let h = Harness::new().await;
    let episodes = archived(&h, 3).await;
    let before: Vec<_> = {
        let mut out = Vec::new();
        for id in &episodes {
            out.push(h.engine.archive_file(*id).await.unwrap().unwrap());
        }
        out
    };

    // The database loses every archive record. The files and their
    // sidecars are untouched, which is the whole premise.
    forget_records(&h, &episodes).await;
    for id in &episodes {
        assert!(h.engine.archive_file(*id).await.unwrap().is_none());
    }

    // A dry run is the default, and it writes nothing.
    let dry = h
        .engine
        .rebuild_archive(&RebuildOptions::default())
        .await
        .unwrap();
    assert!(!dry.applied);
    assert_eq!(dry.scanned, 3);
    assert_eq!(dry.rebuilt, 3);
    assert_eq!(dry.unchanged, 0);
    assert!(!dry.has_findings(), "{dry:?}");
    for id in &episodes {
        assert!(
            h.engine.archive_file(*id).await.unwrap().is_none(),
            "a dry run restores nothing"
        );
    }

    let applied = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.rebuilt, 3);
    assert!(!applied.has_findings(), "{applied:?}");

    for original in &before {
        let restored = h
            .engine
            .archive_file(original.episode_id)
            .await
            .unwrap()
            .expect("the record came back");
        assert_eq!(restored.relative_path, original.relative_path);
        assert_eq!(restored.hash_value, original.hash_value);
        assert_eq!(restored.size_bytes, original.size_bytes);
        assert_eq!(restored.source_hash_value, original.source_hash_value);
        assert_eq!(
            restored.origin,
            ArchiveOrigin::Rebuild,
            "the record remembers where it came from"
        );
        assert_eq!(
            restored.verification_state,
            VerificationState::Unchecked,
            "a sidecar is metadata, not evidence"
        );
        assert_eq!(
            restored.verification_reason.as_deref(),
            Some(reason::REBUILT)
        );
        assert!(restored.verified_at.is_none());
    }

    // Only reading the bytes may say `verified`.
    let summary = h
        .engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(summary.verified, 3);
    for id in &episodes {
        assert_eq!(
            h.engine
                .archive_file(*id)
                .await
                .unwrap()
                .unwrap()
                .verification_state,
            VerificationState::Verified
        );
    }
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rebuilding_twice_reports_everything_unchanged() {
    let h = Harness::new().await;
    let episodes = archived(&h, 2).await;
    forget_records(&h, &episodes).await;
    h.engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();

    let again = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert_eq!(again.scanned, 2);
    assert_eq!(again.unchanged, 2);
    assert_eq!(again.rebuilt, 0, "nothing was rewritten");
    assert!(!again.has_findings(), "{again:?}");
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checked_record_is_never_overwritten_by_a_document() {
    let h = Harness::new().await;
    let episodes = archived(&h, 1).await;
    h.engine
        .verify_episode(episodes[0], VerifyDepth::Full)
        .await
        .unwrap();
    let verified = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(verified.verification_state, VerificationState::Verified);

    // Someone edits the sidecar to claim different bytes. The record was
    // checked against the file; the document was not.
    let media = RelativePath::parse(&verified.relative_path).unwrap();
    let sidecar_path = h.media_dir().join(layout::sidecar_of(&media).as_str());
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sidecar_path).unwrap()).unwrap();
    document["archive"]["hash_value"] = serde_json::json!("0".repeat(64));
    std::fs::write(&sidecar_path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

    let report = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert_eq!(report.conflicts.count, 1, "{report:?}");
    assert_eq!(report.rebuilt, 0);
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.hash_value, verified.hash_value);
    assert_eq!(after.verification_state, VerificationState::Verified);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn documents_that_lead_nowhere_are_reported_and_never_invented_into_records() {
    let h = Harness::new().await;
    let episodes = archived(&h, 3).await;
    let files: Vec<_> = {
        let mut out = Vec::new();
        for id in &episodes {
            out.push(h.engine.archive_file(*id).await.unwrap().unwrap());
        }
        out
    };
    forget_records(&h, &episodes).await;
    let media_dir = h.media_dir();

    // (1) A document whose media file is gone.
    let orphaned = RelativePath::parse(&files[0].relative_path).unwrap();
    std::fs::remove_file(media_dir.join(orphaned.as_str())).unwrap();

    // (2) A document that does not parse.
    let broken = RelativePath::parse(&files[1].relative_path).unwrap();
    std::fs::write(
        media_dir.join(layout::sidecar_of(&broken).as_str()),
        b"{ this is not a sidecar",
    )
    .unwrap();

    // (3) A document naming an episode nothing in this library has, with
    // an identity key that matches nothing either.
    let unknown = RelativePath::parse(&files[2].relative_path).unwrap();
    let unknown_sidecar = media_dir.join(layout::sidecar_of(&unknown).as_str());
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&unknown_sidecar).unwrap()).unwrap();
    document["episode"]["id"] = serde_json::json!(EpisodeId::new().to_string());
    document["episode"]["identity_key"] = serde_json::json!("guid:nothing-like-this");
    std::fs::write(
        &unknown_sidecar,
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();

    let report = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert_eq!(report.rebuilt, 0, "{report:?}");
    assert_eq!(report.missing_media.count, 1, "{report:?}");
    assert_eq!(report.malformed.count, 1, "{report:?}");
    assert_eq!(report.unknown_episode.count, 1, "{report:?}");
    assert!(report.has_findings());

    for id in &episodes {
        assert!(
            h.engine.archive_file(*id).await.unwrap().is_none(),
            "nothing was invented"
        );
    }
    // Nothing was deleted either: the documents are all still there.
    assert!(
        media_dir
            .join(layout::sidecar_of(&orphaned).as_str())
            .is_file()
    );
    assert!(
        media_dir
            .join(layout::sidecar_of(&broken).as_str())
            .is_file()
    );
    assert!(unknown_sidecar.is_file());
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_finds_its_episode_again_after_the_identifiers_changed() {
    let h = Harness::new().await;
    let episodes = archived(&h, 1).await;
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    forget_records(&h, &episodes).await;

    // What a real recovery looks like: the library was rebuilt from the
    // feed, so the episode carries a fresh identifier and only its
    // identity key still points at the same thing.
    let media = RelativePath::parse(&file.relative_path).unwrap();
    let sidecar_path = h.media_dir().join(layout::sidecar_of(&media).as_str());
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sidecar_path).unwrap()).unwrap();
    document["episode"]["id"] = serde_json::json!(EpisodeId::new().to_string());
    document["podcast"]["id"] = serde_json::json!(uguisu_core::ids::PodcastId::new().to_string());
    assert!(
        document["podcast"]["feed_url"].is_string(),
        "the document records the feed URL, which is what survives"
    );
    std::fs::write(&sidecar_path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

    let report = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert_eq!(report.rebuilt, 1, "{report:?}");
    assert!(!report.has_findings(), "{report:?}");
    let restored = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(restored.relative_path, file.relative_path);
    assert_eq!(restored.hash_value, file.hash_value);
    h.engine.close().await;
}
