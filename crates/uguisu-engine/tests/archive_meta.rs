//! Sidecars and manifests through the engine: a finished download
//! describes itself on disk, the manifest is written when the work stops
//! rather than per episode, and everything here is derived data that a
//! second run reproduces exactly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;

use std::time::Duration;

use common::{Harness, synthetic_feed_with_media};
use uguisu_archive::layout;
use uguisu_archive::path::RelativePath;
use uguisu_core::archive::{ArchiveOrigin, Sidecar, TagState, VerificationState};
use uguisu_core::download::Priority;
use uguisu_core::ids::EpisodeId;
use uguisu_core::model::{ChaptersRef, TranscriptRef};
use uguisu_http::CancellationToken;
use uguisu_storage::archive_files::{self, ArchiveFilter};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn archived_podcast(
    h: &Harness,
    count: usize,
) -> (uguisu_core::model::Podcast, Vec<EpisodeId>) {
    let body = synthetic_feed_with_media(count, h.media.base(), &["/normal/2048", "/range/4096"]);
    archived_feed(h, body, count).await
}

async fn archived_feed(
    h: &Harness,
    body: Vec<u8>,
    count: usize,
) -> (uguisu_core::model::Podcast, Vec<EpisodeId>) {
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
    wait_for("every download to be archived", || async {
        let mut reader = h.engine.storage().reader().await.unwrap();
        archive_files::list(&mut reader, &ArchiveFilter::default(), None, 100)
            .await
            .unwrap()
            .len()
            == count
    })
    .await;
    (podcast, episodes.iter().map(|e| e.id).collect())
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finished_download_writes_a_sidecar_beside_its_media() {
    let h = Harness::new().await;
    let (podcast, episodes) = archived_podcast(&h, 3).await;

    for episode_id in &episodes {
        wait_for("the sidecar", || async {
            h.engine
                .archive_file(*episode_id)
                .await
                .unwrap()
                .is_some_and(|f| f.sidecar_written_at.is_some())
        })
        .await;

        let file = h.engine.archive_file(*episode_id).await.unwrap().unwrap();
        let media = RelativePath::parse(&file.relative_path).unwrap();
        let on_disk = h.media_dir().join(layout::sidecar_of(&media).as_str());
        assert!(on_disk.is_file(), "{} has no sidecar", file.relative_path);

        let sidecar = h.engine.read_sidecar(*episode_id).await.unwrap().unwrap();
        assert_eq!(sidecar.schema, 1);
        assert!(sidecar.generator.starts_with("uguisu/"));
        assert_eq!(sidecar.podcast.id, podcast.id);
        assert_eq!(sidecar.podcast.title, podcast.title);
        assert_eq!(sidecar.episode.id, *episode_id);
        assert_eq!(sidecar.archive.relative_path, file.relative_path);
        assert_eq!(sidecar.archive.hash_value, file.hash_value);
        assert_eq!(sidecar.archive.origin, ArchiveOrigin::Download);
        assert_eq!(sidecar.archive.tag_state, TagState::Untagged);
        assert!(!sidecar.episode.identity_key.is_empty());
        // Provenance: for a fresh download the two agree, and the sidecar
        // carries both so a rebuild can tell them apart later.
        let source = sidecar
            .source
            .clone()
            .expect("a download records where it came from");
        assert_eq!(source.hash_value, file.hash_value);
        assert_eq!(source.size_bytes, file.size_bytes);
        assert!(source.origin_detail.is_some());

        // Writing the same record again says the same thing: only the
        // moment of writing moves.
        h.engine.write_sidecar(*episode_id).await.unwrap().unwrap();
        let again = h.engine.read_sidecar(*episode_id).await.unwrap().unwrap();
        assert_eq!(
            Sidecar {
                written_at: sidecar.written_at,
                ..again
            },
            sidecar,
            "a rewrite of an unchanged record changes nothing but its timestamp"
        );
    }
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_manifest_is_written_once_for_the_whole_batch() {
    let h = Harness::new().await;
    let (podcast, episodes) = archived_podcast(&h, 3).await;

    // Three registrations, and the manifest is stale exactly once - it is
    // not rewritten per episode.
    let status = h.engine.manifest_status().await.unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].podcast_id, podcast.id);
    assert!(status[0].stale);
    assert_eq!(status[0].entries, 0, "nothing has been written yet");

    let written = h.engine.write_stale_manifests().await.unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].entries, 3);
    assert!(written[0].cleared);

    let on_disk = h.media_dir().join(&written[0].path);
    assert!(on_disk.is_file());
    let text = std::fs::read_to_string(&on_disk).unwrap();
    assert!(text.starts_with("# uguisu manifest"));
    let entries = uguisu_archive::manifest::parse(&text).unwrap();
    assert_eq!(entries.len(), 3);
    for episode_id in &episodes {
        let file = h.engine.archive_file(*episode_id).await.unwrap().unwrap();
        let entry = entries
            .iter()
            .find(|e| e.relative_path == file.relative_path)
            .expect("every artifact is listed");
        assert_eq!(entry.hash_value, file.hash_value);
    }
    assert!(
        !text.contains(".json"),
        "the manifest lists media, not the documents describing it"
    );

    // Nothing changed, so nothing is stale and a second flush is a no-op.
    assert!(!h.engine.manifest_status().await.unwrap()[0].stale);
    assert!(h.engine.write_stale_manifests().await.unwrap().is_empty());

    // Writing it again from the same index produces the same listing.
    let again = h.engine.write_manifest(podcast.id).await.unwrap();
    assert_eq!(again.entries, 3);
    assert_eq!(
        uguisu_archive::manifest::parse(&std::fs::read_to_string(&on_disk).unwrap()).unwrap(),
        entries
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verifying_a_manifest_reports_what_moved_without_changing_anything() {
    let h = Harness::new().await;
    let (podcast, episodes) = archived_podcast(&h, 3).await;
    h.engine.write_stale_manifests().await.unwrap();

    let check = h.engine.verify_manifest(podcast.id, true).await.unwrap();
    assert!(check.diff.is_clean(), "{:?}", check.diff);
    assert_eq!(check.diff.unchanged, 3);
    assert!(!check.stale);

    // Change one file behind Uguisu's back and delete another.
    let changed = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let missing = h.engine.archive_file(episodes[1]).await.unwrap().unwrap();
    std::fs::write(
        h.media_dir().join(&changed.relative_path),
        b"something else entirely",
    )
    .unwrap();
    std::fs::remove_file(h.media_dir().join(&missing.relative_path)).unwrap();

    let check = h.engine.verify_manifest(podcast.id, true).await.unwrap();
    assert_eq!(
        check.diff.changed.sample,
        vec![changed.relative_path.clone()]
    );
    assert_eq!(
        check.diff.missing.sample,
        vec![missing.relative_path.clone()]
    );
    assert_eq!(check.diff.unchanged, 1);
    assert!(check.rehashed);

    // Reading the manifest changed no record: verification is
    // `archive verify`'s job, and this is a report.
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.hash_value, changed.hash_value);
    assert_eq!(after.verification_state, changed.verification_state);

    // Without re-reading the files, the same check compares the manifest
    // against the index instead - which is cheap and answers a different
    // question: has the file fallen behind?
    let cheap = h.engine.verify_manifest(podcast.id, false).await.unwrap();
    assert!(
        cheap.diff.is_clean(),
        "the index still agrees: {:?}",
        cheap.diff
    );
    assert!(!cheap.rehashed);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sidecar_keeps_the_feeds_references() {
    let h = Harness::new().await;
    let feed = String::from_utf8(synthetic_feed_with_media(1, h.media.base(), &["/normal/2048"]))
        .unwrap()
        .replace(
            "</item>",
            r#"<itunes:image href="https://cdn.synthetic.example/art/0.jpg"/>
<podcast:chapters url="https://cdn.synthetic.example/chapters/0.json" type="application/json+chapters"/>
<podcast:transcript url="https://cdn.synthetic.example/transcripts/0.vtt" type="text/vtt" language="de" rel="captions"/>
</item>"#,
        );
    let (_, episodes) = archived_feed(&h, feed.into_bytes(), 1).await;
    wait_for("the sidecar", || async {
        h.engine
            .archive_file(episodes[0])
            .await
            .unwrap()
            .is_some_and(|f| f.sidecar_written_at.is_some())
    })
    .await;

    let episode = h
        .engine
        .read_sidecar(episodes[0])
        .await
        .unwrap()
        .unwrap()
        .episode;
    assert_eq!(
        episode.artwork_url.map(String::from).as_deref(),
        Some("https://cdn.synthetic.example/art/0.jpg")
    );
    assert_eq!(
        episode.chapters,
        [ChaptersRef {
            url: "https://cdn.synthetic.example/chapters/0.json".to_owned(),
            mime_type: Some("application/json+chapters".to_owned()),
        }]
    );
    assert_eq!(
        episode.transcripts,
        [TranscriptRef {
            url: "https://cdn.synthetic.example/transcripts/0.vtt".to_owned(),
            mime_type: Some("text/vtt".to_owned()),
            language: Some("de".to_owned()),
            rel: Some("captions".to_owned()),
        }]
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relocation_takes_the_sidecar_with_it_and_marks_the_manifest_stale() {
    let h = Harness::new().await;
    let (_, episodes) = archived_podcast(&h, 1).await;
    wait_for("the sidecar", || async {
        h.engine
            .archive_file(episodes[0])
            .await
            .unwrap()
            .is_some_and(|f| f.sidecar_written_at.is_some())
    })
    .await;
    h.engine.write_stale_manifests().await.unwrap();
    assert!(!h.engine.manifest_status().await.unwrap()[0].stale);

    // Put the artifact somewhere the template would not have put it, as a
    // older archive or a changed template leaves it.
    let before = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let media = h.media_dir();
    let legacy = "legacy/old-name.mp3";
    std::fs::create_dir_all(media.join("legacy")).unwrap();
    std::fs::rename(media.join(&before.relative_path), media.join(legacy)).unwrap();
    let old_sidecar = media
        .join(layout::sidecar_of(&RelativePath::parse(&before.relative_path).unwrap()).as_str());
    assert!(old_sidecar.is_file());
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::set_path(
            &mut w,
            before.id,
            legacy,
            None,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    }

    let moved = h.engine.relocate(episodes[0], false).await.unwrap();
    assert!(moved.moved);
    assert_eq!(moved.from, legacy);

    // The sidecar is rendered again beside the media at its new path.
    let new_sidecar =
        media.join(layout::sidecar_of(&RelativePath::parse(&moved.to).unwrap()).as_str());
    assert!(new_sidecar.is_file(), "the sidecar followed the media");
    let document = h.engine.read_sidecar(episodes[0]).await.unwrap().unwrap();
    assert_eq!(document.archive.relative_path, moved.to);
    assert_eq!(document.archive.hash_value, before.hash_value);

    // The document left at the old path stays there: nothing deletes
    // nothing, not even its own leftovers, and a rebuild reports it.
    assert!(
        old_sidecar.is_file(),
        "a superseded sidecar is reported, never removed"
    );
    assert!(
        h.engine.manifest_status().await.unwrap()[0].stale,
        "a moved file means the manifest is behind the index"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clean_close_leaves_every_manifest_current() {
    let mut h = Harness::new().await;
    archived_podcast(&h, 2).await;
    assert!(h.engine.manifest_status().await.unwrap()[0].stale);

    // No explicit flush anywhere: closing is what writes them, which is
    // what makes the guarantee worth stating.
    h.restart(None).await;

    let status = h.engine.manifest_status().await.unwrap();
    assert_eq!(status.len(), 1);
    assert!(
        !status[0].stale,
        "after a clean close the manifest agrees with the index"
    );
    assert_eq!(status[0].entries, 2);
    let on_disk = h.media_dir().join(&status[0].relative_path);
    assert!(on_disk.is_file());
    assert_eq!(
        uguisu_archive::manifest::parse(&std::fs::read_to_string(&on_disk).unwrap())
            .unwrap()
            .len(),
        2
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_artifact_with_no_sidecar_gets_one_on_the_next_pass() {
    let h = Harness::new().await;
    let (_, episodes) = archived_podcast(&h, 2).await;
    wait_for("the sidecars", || async {
        let mut reader = h.engine.storage().reader().await.unwrap();
        archive_files::without_sidecar(&mut reader, None, 10)
            .await
            .unwrap()
            .is_empty()
    })
    .await;

    // Simulate a crash between registering and writing the sidecar: the
    // record says it has none, which is exactly what the catch-up pass
    // looks for.
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    {
        // A re-registration is exactly how this state arises in practice:
        // fresh bytes carry no sidecar, so the upsert clears the mark.
        let mut w = h.engine.storage().writer().await.unwrap();
        let mut cleared = file.clone();
        cleared.sidecar_written_at = None;
        archive_files::upsert(&mut w, &cleared).await.unwrap();
    }
    let sidecar_path = h
        .media_dir()
        .join(layout::sidecar_of(&RelativePath::parse(&file.relative_path).unwrap()).as_str());
    std::fs::remove_file(&sidecar_path).unwrap();

    assert_eq!(h.engine.write_pending_sidecars(10).await.unwrap(), 1);
    assert!(sidecar_path.is_file());
    assert_eq!(
        h.engine.write_pending_sidecars(10).await.unwrap(),
        0,
        "a second pass has nothing to do"
    );
    let restored = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert!(restored.sidecar_written_at.is_some());
    assert_eq!(
        restored.verification_state, file.verification_state,
        "writing a sidecar is not a verification"
    );
    assert_ne!(restored.verification_state, VerificationState::Missing);
    h.engine.close().await;
}

#[tokio::test]
async fn an_enormous_manifest_is_refused() {
    let h = Harness::new().await;
    let (podcast, _) = archived_podcast(&h, 1).await;
    h.engine.write_stale_manifests().await.unwrap();
    let path = h
        .media_dir()
        .join(uguisu_archive::manifest::path_for(podcast.id).as_str());
    // Sparse: the length is what counts, and only a byte past the cap is read.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(uguisu_archive::manifest::MAX_MANIFEST_BYTES + 1)
        .unwrap();

    let refused = h
        .engine
        .verify_manifest(podcast.id, false)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("larger than"), "{refused}");
    h.engine.close().await;
}
