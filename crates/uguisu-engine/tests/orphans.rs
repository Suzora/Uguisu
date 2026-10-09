//! `archive orphans`: what nothing owns is reported and never touched
//! (ADR 0051).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Harness, synthetic_feed_with_media, tree_digest};
use uguisu_archive::layout;
use uguisu_core::download::Priority;
use uguisu_core::ids::{EpisodeId, JobId};
use uguisu_http::CancellationToken;
use uguisu_storage::archive_files::{self, ArchiveFilter};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Adds a podcast whose episodes the scenario media server answers.
async fn media_podcast(h: &Harness, count: usize) -> Vec<EpisodeId> {
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
        .iter()
        .map(|e| e.id)
        .collect()
}

/// One archived episode with its sidecar; returns its media path.
async fn archived(h: &Harness) -> String {
    let episodes = media_podcast(h, 1).await;
    h.engine
        .downloads()
        .enqueue_episode(episodes[0], Priority::Normal)
        .await
        .unwrap();
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let mut reader = h.engine.storage().reader().await.unwrap();
        let all = archive_files::list(&mut reader, &ArchiveFilter::default(), None, 10)
            .await
            .unwrap();
        if let Some(file) = all.first()
            && file.sidecar_written_at.is_some()
        {
            return file.relative_path.clone();
        }
        assert!(std::time::Instant::now() < deadline, "no archived file");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn plant(root: &Path, relative: &str) {
    let full = root.join(relative);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, b"left behind").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_archive_reports_nothing() {
    let h = Harness::new().await;
    archived(&h).await;
    let report = h.engine.orphans().await.unwrap();
    assert!(!report.has_findings(), "{report:?}");
    assert!(
        report.scanned >= 2,
        "the media file and its sidecar: {report:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orphan_report_deletes_nothing() {
    let h = Harness::new().await;
    let media = archived(&h).await;
    let root = h.media_dir();
    let tmp_name = |stem: &str, n: u32| format!("{stem}.4242.{n}.tmp");
    let leftovers = [
        format!(
            "{}/tmp/01J0000000000000000000000A.import",
            layout::CONTROL_DIR
        ),
        format!(
            "{}/artwork/p/{}",
            layout::CONTROL_DIR,
            tmp_name("ab12.jpg", 0)
        ),
        format!(
            "{}/manifests/p/{}",
            layout::CONTROL_DIR,
            tmp_name("manifest.sha256", 1)
        ),
        tmp_name(&format!("{media}.json"), 2),
    ];
    for path in &leftovers {
        plant(&root, path);
    }
    let part = format!("Foreign/.uguisu-tmp/{}.part", JobId::new());
    plant(&root, &part);
    plant(&root, "Foreign/song.mp3");
    plant(&root, "Foreign/gone.mp3.json");
    let before = tree_digest(&root);

    let report = h.engine.orphans().await.unwrap();
    assert_eq!(tree_digest(&root), before, "the report changed the tree");
    let mut expected = leftovers.to_vec();
    expected.sort();
    let mut found = report.leftovers.sample.clone();
    found.sort();
    assert_eq!(found, expected, "{report:?}");
    assert_eq!(report.orphan_parts.sample, [part]);
    assert_eq!(report.unknown_media.sample, ["Foreign/song.mp3"]);
    assert_eq!(report.stray_sidecars.sample, ["Foreign/gone.mp3.json"]);
    assert!(report.unreadable.is_empty(), "{report:?}");
}

#[tokio::test]
async fn absent_media_root_is_clean() {
    let h = Harness::new().await;
    assert!(!h.media_dir().exists(), "nothing downloaded yet");
    let report = h.engine.orphans().await.unwrap();
    assert!(!report.has_findings(), "{report:?}");
}

#[tokio::test]
async fn user_files_are_not_leftovers() {
    let h = Harness::new().await;
    let root = h.media_dir();
    for name in ["notes.json", "notes.tmp", "draft.12.tmp", "cover.jpg"] {
        plant(&root, &format!("Show/{name}"));
    }
    plant(
        &root,
        &format!("{}/manifests/p/manifest.sha256", layout::CONTROL_DIR),
    );
    let report = h.engine.orphans().await.unwrap();
    assert!(!report.has_findings(), "{report:?}");
}

#[tokio::test]
async fn held_scratch_is_not_reported() {
    let h = Harness::new().await;
    let relative = format!(
        "{}/tmp/01J0000000000000000000000B.tagtmp",
        layout::CONTROL_DIR
    );
    plant(&h.media_dir(), &relative);
    let held = layout::Scratch::hold(&h.media_dir().join(&relative));
    let report = h.engine.orphans().await.unwrap();
    assert!(
        report.leftovers.is_empty(),
        "a write in progress: {report:?}"
    );
    drop(held);
    let report = h.engine.orphans().await.unwrap();
    assert_eq!(report.leftovers.sample, [relative]);
}

#[tokio::test]
async fn a_jobs_part_is_not_orphaned() {
    let h = Harness::new().await;
    let episodes = media_podcast(&h, 1).await;
    let job = h
        .engine
        .downloads()
        .enqueue_episode(episodes[0], Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    let owned = format!("Show/.uguisu-tmp/{job}.part");
    let orphan = format!("Show/.uguisu-tmp/{}.part", JobId::new());
    plant(&h.media_dir(), &owned);
    plant(&h.media_dir(), &orphan);
    let report = h.engine.orphans().await.unwrap();
    assert_eq!(report.orphan_parts.sample, [orphan]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_jobs_part_is_reported() {
    // A crash between the move's link and its unlink leaves the `.part` name
    // of a job that completed.
    let h = Harness::new().await;
    archived(&h).await;
    let mut reader = h.engine.storage().reader().await.unwrap();
    let job = uguisu_storage::downloads::jobs_in_states(
        &mut reader,
        &[uguisu_core::download::DownloadState::Completed],
    )
    .await
    .unwrap()
    .remove(0);
    drop(reader);
    let leftover = format!("Show/.uguisu-tmp/{}.part", job.id);
    plant(&h.media_dir(), &leftover);
    let report = h.engine.orphans().await.unwrap();
    assert_eq!(report.orphan_parts.sample, std::slice::from_ref(&leftover));
    assert!(
        h.media_dir().join(&leftover).is_file(),
        "reported, never removed"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moved_media_leaves_both_reported() {
    let h = Harness::new().await;
    let media = archived(&h).await;
    let root = h.media_dir();
    std::fs::create_dir_all(root.join("Moved")).unwrap();
    std::fs::rename(root.join(&media), root.join("Moved/episode.mp3")).unwrap();
    let report = h.engine.orphans().await.unwrap();
    assert_eq!(report.unknown_media.sample, ["Moved/episode.mp3"]);
    assert_eq!(report.stray_sidecars.sample, [format!("{media}.json")]);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_is_reported_unreadable() {
    let h = Harness::new().await;
    let root = h.media_dir();
    plant(&root, "Show/real.mp3");
    std::os::unix::fs::symlink(root.join("Show/real.mp3"), root.join("Show/link.mp3")).unwrap();
    let report = h.engine.orphans().await.unwrap();
    assert_eq!(report.unreadable.sample, ["Show/link.mp3"]);
    assert_eq!(report.unknown_media.sample, ["Show/real.mp3"]);
}
