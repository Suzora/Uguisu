//! Artwork and tag writing through the engine.
//!
//! Two operations that reach outside Uguisu: one fetches bytes from a
//! stranger's URL, the other rewrites a file the user owns. Both are
//! explicit, and both are tested for what they refuse as much as for what
//! they do.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;

use std::time::Duration;

use common::Harness;
use sha2::Digest;
use uguisu_core::archive::{
    ArtworkFormat, TagMode, TagState, VerificationState, VerifyDepth, reason,
};
use uguisu_core::download::Priority;
use uguisu_core::ids::EpisodeId;
use uguisu_core::model::{ArchiveState, Podcast};
use uguisu_core::{EventKind, UguisuError};
use uguisu_engine::artwork::ArtworkOutcome;
use uguisu_engine::restore::{RestoreAction, RestoreOptions};
use uguisu_http::CancellationToken;
use uguisu_storage::archive_files::{self, ArchiveFilter};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// One valid MPEG-1 Layer III frame of silence, repeated, so the file is
/// something `lofty` will actually parse.
fn silent_mp3() -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..20 {
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        out.extend(std::iter::repeat_n(0u8, 413));
    }
    out
}

/// A one-pixel PNG.
fn tiny_png() -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend_from_slice(&[0, 0, 0, 13]);
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]);
    out.extend_from_slice(&[0x3A, 0x7E, 0x9B, 0x55]);
    out.extend_from_slice(&[0, 0, 0, 0x0A]);
    out.extend_from_slice(b"IDAT");
    out.extend_from_slice(&[0x78, 0x9C, 0x63, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01]);
    out.extend_from_slice(&[0x0D, 0x0A, 0x2D, 0xB4]);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
    out
}

/// A feed with real artwork and a real audio enclosure, both on the mock
/// server so the whole path runs end to end.
fn feed(base: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\n\
         <channel>\n\
         <title>Uguisu Test Show</title>\n\
         <link>https://example.test/</link>\n\
         <description>For the asset tests</description>\n\
         <language>de</language>\n\
         <copyright>(c) 2024 Uguisu</copyright>\n\
         <itunes:author>Die Autorin</itunes:author>\n\
         <itunes:image href=\"{base}/cover.png\"/>\n\
         <itunes:category text=\"Technology\"/>\n\
         <item><title>Folge 1: Gr&#252;&#223;e aus K&#246;ln</title>\
         <guid isPermaLink=\"false\">ep-1</guid>\
         <pubDate>05 Jan 2024 10:00:00 +0000</pubDate>\
         <description>Die erste Folge.</description>\
         <itunes:episode>1</itunes:episode><itunes:season>2</itunes:season>\
         <enclosure url=\"{base}/audio.mp3\" type=\"audio/mpeg\" length=\"8340\"/>\
         </item>\n\
         </channel>\n</rss>\n"
    )
    .into_bytes()
}

async fn setup(h: &Harness) -> (Podcast, Vec<EpisodeId>) {
    setup_with(h, silent_mp3()).await
}

/// [`setup`], serving `audio` as the episode's file.
async fn setup_with(h: &Harness, audio: Vec<u8>) -> (Podcast, Vec<EpisodeId>) {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(feed(&h.server.uri()))
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/audio.mp3"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(audio)
                .insert_header("content-type", "audio/mpeg"),
        )
        .mount(&h.server)
        .await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
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
    // The record exists before the check on completion has run, and that
    // check changes it: wait for both, or a test reads it half-way.
    wait_for("the download to be archived and checked", || async {
        let mut reader = h.engine.storage().reader().await.unwrap();
        let files = archive_files::list(&mut reader, &ArchiveFilter::default(), None, 10)
            .await
            .unwrap();
        !files.is_empty()
            && files
                .iter()
                .all(|f| f.verification_state != VerificationState::Unchecked)
    })
    .await;
    let ids = h
        .engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes
        .iter()
        .map(|e| e.id)
        .collect();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artwork_is_fetched_validated_and_stored_under_its_own_hash() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tiny_png())
                .insert_header("content-type", "image/png")
                .insert_header("etag", "\"v1\""),
        )
        .mount(&h.server)
        .await;
    let (podcast, _) = setup(&h).await;

    let outcome = h.engine.fetch_artwork(podcast.id, false).await.unwrap();
    let ArtworkOutcome::Fetched(art) = outcome else {
        panic!("expected a fetch, got {outcome:?}");
    };
    assert_eq!(art.format, ArtworkFormat::Png);
    assert_eq!(art.size_bytes, tiny_png().len() as u64);
    assert!(art.is_current);
    assert_eq!(art.etag.as_deref(), Some("\"v1\""));
    assert!(
        art.relative_path.contains(&art.hash_value),
        "the file is named after its own bytes: {}",
        art.relative_path
    );
    let on_disk = h.media_dir().join(&art.relative_path);
    assert_eq!(std::fs::read(&on_disk).unwrap(), tiny_png());

    // A second fetch sends the validator and costs nothing.
    h.server.reset().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&h.server)
        .await;
    let outcome = h.engine.fetch_artwork(podcast.id, false).await.unwrap();
    assert_eq!(outcome, ArtworkOutcome::Unchanged(art.id));
    assert_eq!(
        h.engine.artwork_history(podcast.id).await.unwrap().len(),
        1,
        "nothing new was stored"
    );
    h.engine.close().await;
}

/// The artwork write is file first, then one transaction that demotes the
/// old row and records the new one. A crash at either boundary leaves the
/// old image current and intact, and the next fetch converges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupted_artwork_write_converges() {
    // The same pixel with different bytes: a new image by its hash.
    let mut second = tiny_png();
    second.extend_from_slice(b"trailing bytes");
    let second_hash = hex::encode(sha2::Sha256::digest(&second));

    for renamed in [false, true] {
        let mut h = Harness::new().await;
        Mock::given(method("GET"))
            .and(path("/cover.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(tiny_png())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&h.server)
            .await;
        let (podcast, _) = setup(&h).await;
        let ArtworkOutcome::Fetched(first) =
            h.engine.fetch_artwork(podcast.id, false).await.unwrap()
        else {
            panic!("the first fetch stores the image");
        };

        // The state a crash fetching the second image leaves: a temporary
        // file cut short before its rename, or the renamed file with no row.
        let target = h.media_dir().join(
            uguisu_archive::layout::artwork_path(podcast.id, &second_hash, ArtworkFormat::Png)
                .as_str(),
        );
        let leftover = target.with_extension("png.4242.0.tmp");
        if renamed {
            std::fs::write(&target, &second).unwrap();
        } else {
            std::fs::write(&leftover, &second[..10]).unwrap();
        }
        h.restart(None).await;

        let current = h.engine.current_artwork(podcast.id).await.unwrap().unwrap();
        assert_eq!(current.id, first.id, "renamed={renamed}");
        assert_eq!(
            std::fs::read(h.media_dir().join(&current.relative_path)).unwrap(),
            tiny_png(),
            "renamed={renamed}: the current image is whole"
        );

        h.server.reset().await;
        Mock::given(method("GET"))
            .and(path("/cover.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(second.clone())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&h.server)
            .await;
        let ArtworkOutcome::Fetched(now) = h.engine.fetch_artwork(podcast.id, false).await.unwrap()
        else {
            panic!("renamed={renamed}: the next fetch records the new image");
        };
        assert_eq!(now.hash_value, second_hash);
        assert_eq!(std::fs::read(&target).unwrap(), second, "renamed={renamed}");
        let history = h.engine.artwork_history(podcast.id).await.unwrap();
        assert_eq!(history.len(), 2, "renamed={renamed}: {history:?}");
        assert_eq!(
            history.iter().filter(|a| a.is_current).count(),
            1,
            "renamed={renamed}"
        );
        assert_eq!(
            std::fs::read(h.media_dir().join(&first.relative_path)).unwrap(),
            tiny_png(),
            "renamed={renamed}: the image before it is kept"
        );
        assert_eq!(
            leftover.exists(),
            !renamed,
            "a leftover is kept, never cleaned up"
        );
        h.engine.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bytes_that_are_not_an_image_never_reach_the_archive() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                // An HTML error page served as a PNG: the single most
                // common thing a podcast artwork URL actually returns.
                .set_body_bytes(b"<!DOCTYPE html><html><body>404</body></html>".to_vec())
                .insert_header("content-type", "image/png"),
        )
        .mount(&h.server)
        .await;
    let (podcast, _) = setup(&h).await;

    let err = h.engine.fetch_artwork(podcast.id, false).await.unwrap_err();
    assert!(err.to_string().contains("not a JPEG, PNG or WebP"), "{err}");
    assert!(
        h.engine
            .current_artwork(podcast.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        h.engine
            .artwork_history(podcast.id)
            .await
            .unwrap()
            .is_empty(),
        "nothing was stored, so there is nothing to clean up"
    );
    let control = h.media_dir().join(".uguisu").join("artwork");
    assert!(
        !control.exists() || std::fs::read_dir(&control).unwrap().next().is_none(),
        "and nothing was written to disk either"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tagging_replaces_the_file_and_moves_the_record_with_it() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tiny_png())
                .insert_header("content-type", "image/png"),
        )
        .mount(&h.server)
        .await;
    let (podcast, episodes) = setup(&h).await;
    h.engine.fetch_artwork(podcast.id, false).await.unwrap();
    h.engine.write_stale_manifests().await.unwrap();

    let before = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(before.tag_state, TagState::Untagged);
    let media = h.media_dir().join(&before.relative_path);
    let original_bytes = std::fs::read(&media).unwrap();

    let result = h
        .engine
        .write_episode_tags(episodes[0], TagMode::FillMissing)
        .await
        .unwrap();
    assert_eq!(result.state, "written", "{result:?}");
    assert!(result.fields.contains(&"title".to_owned()));
    assert!(result.fields.contains(&"album".to_owned()));
    assert!(result.cover_written);

    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.tag_state, TagState::Written);
    assert_eq!(after.tag_mode, Some(TagMode::FillMissing));
    assert!(after.tagged_at.is_some());
    assert_ne!(
        after.hash_value, before.hash_value,
        "the bytes changed, so the record did"
    );
    assert_eq!(
        after.source_hash_value, before.source_hash_value,
        "provenance still records what was downloaded"
    );
    assert_eq!(after.source_size_bytes, before.source_size_bytes);
    assert_eq!(
        after.verification_state,
        VerificationState::Verified,
        "the replacement was hashed as it was written"
    );
    assert_eq!(after.verification_reason.as_deref(), Some(reason::TAGGED));

    // The record matches the bytes on disk, which a real check confirms.
    let on_disk = std::fs::read(&media).unwrap();
    assert_ne!(on_disk, original_bytes);
    assert_eq!(
        hex::encode(sha2::Sha256::digest(&on_disk)),
        after.hash_value
    );
    let tags = uguisu_metadata::read_tags(&media).unwrap();
    assert_eq!(
        tags.values
            .get(&uguisu_metadata::Field::Title)
            .map(String::as_str),
        Some("Folge 1: Grüße aus Köln")
    );
    assert!(tags.cover.is_some(), "the podcast artwork was embedded");

    // And the two derived artefacts followed.
    let sidecar = h.engine.read_sidecar(episodes[0]).await.unwrap().unwrap();
    assert_eq!(sidecar.archive.hash_value, after.hash_value);
    assert_eq!(sidecar.archive.tag_state, TagState::Written);
    assert_eq!(
        sidecar.source.unwrap().hash_value,
        before.hash_value,
        "the sidecar still records the bytes that were downloaded"
    );
    assert!(
        h.engine.manifest_status().await.unwrap()[0].stale,
        "the file changed, so the manifest is behind"
    );

    // No scratch copies left behind.
    let tmp = h.media_dir().join(".uguisu").join("tmp");
    assert!(
        !tmp.exists() || std::fs::read_dir(&tmp).unwrap().next().is_none(),
        "the working copy was renamed into place, not abandoned"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tagging_twice_is_a_no_op_and_cannot_move_the_hash() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    h.engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap();
    let first = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();

    let again = h
        .engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap();
    assert_eq!(again.state, "nothing_to_write", "{again:?}");
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.hash_value, first.hash_value);
    assert_eq!(after.size_bytes, first.size_bytes);
    assert_eq!(after.tag_state, TagState::Written, "still tagged");

    // A crash before a later write's rename leaves the tagged bytes, which
    // are still tagged.
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::set_tag_state(
            &mut w,
            first.id,
            TagState::Pending,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    }
    assert_eq!(h.engine.recover_interrupted_tagging().await.unwrap(), 1);
    let recovered = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(recovered.tag_state, TagState::Written);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_write_keeps_original_tags() {
    let h = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let tagged = dir.path().join("tagged.mp3");
    std::fs::write(&tagged, silent_mp3()).unwrap();
    let mut theirs = uguisu_metadata::TagSet::default();
    theirs.set(uguisu_metadata::Field::Title, "Their title");
    theirs.set(uguisu_metadata::Field::Album, "Their album");
    uguisu_metadata::write_tags(&tagged, &theirs, TagMode::Sync).unwrap();
    let (_, episodes) = setup_with(&h, std::fs::read(&tagged).unwrap()).await;

    h.engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap();
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let original = file
        .original_tags
        .clone()
        .expect("captured before the write");
    assert_eq!(
        original.values.get("title").map(String::as_str),
        Some("Their title")
    );
    assert_eq!(
        original.values.get("album").map(String::as_str),
        Some("Their album")
    );
    assert_eq!(original.cover, None);
    let now_reads = h.engine.read_episode_tags(episodes[0]).await.unwrap();
    assert_ne!(
        now_reads
            .values
            .get(&uguisu_metadata::Field::Title)
            .map(String::as_str),
        Some("Their title"),
        "the write did replace them"
    );

    // A later write is not the first: the snapshot stays the original.
    h.engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap();
    let again = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(again.original_tags, Some(original.clone()));

    h.engine.write_sidecar(episodes[0]).await.unwrap();
    let sidecar = h.engine.read_sidecar(episodes[0]).await.unwrap().unwrap();
    assert_eq!(sidecar.archive.original_tags, Some(original));
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_that_does_not_match_its_record_is_never_tagged() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let media = h.media_dir().join(&file.relative_path);

    // Something changed the file behind Uguisu's back. Tagging it would
    // replace a detectable problem with an undetectable one.
    std::fs::write(&media, b"not what the record says at all").unwrap();
    let err = h
        .engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("only written to a file"), "{err}");
    assert_eq!(
        std::fs::read(&media).unwrap(),
        b"not what the record says at all",
        "and the file was left exactly as it was found"
    );
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.tag_state, TagState::Untagged);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_rename_failure_stays_pending() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let media = h.media_dir().join(&file.relative_path);
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::refuse_tag_records(&mut w).await.unwrap();
    }
    h.engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap_err();
    let on_disk = hex::encode(sha2::Sha256::digest(std::fs::read(&media).unwrap()));
    assert_ne!(
        on_disk, file.hash_value,
        "the tagged copy replaced the file"
    );
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.tag_state, TagState::Pending, "the marker survives");

    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::allow_tag_records(&mut w).await.unwrap();
    }
    assert_eq!(h.engine.recover_interrupted_tagging().await.unwrap(), 1);
    let recovered = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(recovered.tag_state, TagState::Written);
    assert_eq!(recovered.hash_value, on_disk);
    assert_eq!(
        recovered.verification_reason.as_deref(),
        Some(reason::RETAG_RECOVERED)
    );
    h.engine.close().await;
}

/// Leaves a tag write failed after its rename: Uguisu's bytes in place and
/// the record still `pending` on the old ones. Returns their hash.
async fn interrupt_after_rename(h: &Harness, episode: EpisodeId) -> String {
    let file = h.engine.archive_file(episode).await.unwrap().unwrap();
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::refuse_tag_records(&mut w).await.unwrap();
    }
    h.engine
        .write_episode_tags(episode, TagMode::Sync)
        .await
        .unwrap_err();
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::allow_tag_records(&mut w).await.unwrap();
    }
    let media = h.media_dir().join(&file.relative_path);
    hex::encode(sha2::Sha256::digest(std::fs::read(media).unwrap()))
}

/// Uguisu's own bytes, left by an interrupted write, are adopted before a
/// verification judges them, never called a mismatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_write_settles_before_verdict() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    let on_disk = interrupt_after_rename(&h, episodes[0]).await;

    let mut sub = h.engine.subscribe();
    for depth in [VerifyDepth::Light, VerifyDepth::Full] {
        let verified = h.engine.verify_episode(episodes[0], depth).await.unwrap();
        assert_eq!(verified.state, VerificationState::Verified, "{depth}");
    }
    let summary = h
        .engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(summary.invalid, 0);

    let after = h.engine.episode(episodes[0]).await.unwrap();
    let record = after.archive.unwrap();
    assert_eq!(
        (
            record.tag_state,
            record.hash_value,
            record.verification_state
        ),
        (TagState::Written, on_disk, VerificationState::Verified)
    );
    assert_eq!(after.episode.archive_state, ArchiveState::Archived);
    while let Some(event) = sub.try_recv() {
        assert!(
            !matches!(event.kind, EventKind::ArchiveInvalid { .. }),
            "{event:?}"
        );
    }
    h.engine.close().await;
}

/// A retry of an interrupted write settles the first one from its bytes and
/// goes on from there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_settles_interrupted_write() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    interrupt_after_rename(&h, episodes[0]).await;

    h.engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap();
    let record = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let media = h.media_dir().join(&record.relative_path);
    let on_disk = hex::encode(sha2::Sha256::digest(std::fs::read(media).unwrap()));
    assert_eq!(
        (record.tag_state, record.hash_value),
        (TagState::Written, on_disk)
    );
    assert_eq!(h.engine.recover_interrupted_tagging().await.unwrap(), 0);
    h.engine.close().await;
}

/// A file gone from its path is missing, whether or not a tag write to it
/// was interrupted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_write_still_reports_missing() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    interrupt_after_rename(&h, episodes[0]).await;
    let tagged = std::fs::read(h.media_dir().join(&file.relative_path)).unwrap();
    std::fs::remove_file(h.media_dir().join(&file.relative_path)).unwrap();

    for depth in VerifyDepth::ALL {
        let verified = h.engine.verify_episode(episodes[0], depth).await.unwrap();
        assert_eq!(verified.state, VerificationState::Missing, "{depth}");
        assert_eq!(verified.file.tag_state, TagState::Pending, "{depth}");
    }
    let summary = h
        .engine
        .verify_all(&ArchiveFilter::default(), VerifyDepth::Existence)
        .await
        .unwrap();
    assert_eq!(summary.missing, 1);
    let after = h.engine.episode(episodes[0]).await.unwrap();
    assert_eq!(after.episode.archive_state, ArchiveState::Missing);

    // The marker waited for the bytes, so they are adopted when they return.
    std::fs::write(h.media_dir().join(&file.relative_path), &tagged).unwrap();
    let back = h
        .engine
        .verify_episode(episodes[0], VerifyDepth::Full)
        .await
        .unwrap();
    assert_eq!(
        (back.state, back.file.tag_state),
        (VerificationState::Verified, TagState::Written)
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupted_tag_write_is_resolved_from_the_marker() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let media = h.media_dir().join(&file.relative_path);

    // (1) The crash happened before the replacement landed: the bytes
    // still match the record, so there is nothing to adopt.
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::set_tag_state(
            &mut w,
            file.id,
            TagState::Pending,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    }
    assert_eq!(h.engine.recover_interrupted_tagging().await.unwrap(), 1);
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.tag_state, TagState::Untagged);
    assert_eq!(after.hash_value, file.hash_value);
    assert_eq!(after.verification_state, file.verification_state);

    // (2) The crash happened after it landed: the marker was committed
    // before a single byte moved, so these bytes are Uguisu's own work.
    let mut tagged = silent_mp3();
    tagged.extend_from_slice(b"tagged by uguisu");
    std::fs::write(&media, &tagged).unwrap();
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::set_tag_state(
            &mut w,
            file.id,
            TagState::Pending,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    }
    assert_eq!(h.engine.recover_interrupted_tagging().await.unwrap(), 1);
    let after = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_eq!(after.tag_state, TagState::Written);
    assert_eq!(
        after.hash_value,
        hex::encode(sha2::Sha256::digest(&tagged)),
        "the record adopted the bytes the write had already produced"
    );
    assert_eq!(
        after.verification_reason.as_deref(),
        Some(reason::RETAG_RECOVERED),
        "recorded as a resumed write, never as a verification"
    );
    assert_eq!(after.verification_state, VerificationState::Unchecked);
    assert_eq!(after.verified_at, None);
    assert_eq!(
        after.source_hash_value, file.source_hash_value,
        "and the provenance is still the download's"
    );
    assert_eq!(
        h.engine.recover_interrupted_tagging().await.unwrap(),
        0,
        "nothing is left in flight"
    );
    h.engine.close().await;
}

/// Serves the feed at `/feed.xml`, with one more item when `second` is set,
/// so a refresh sees a changed body.
async fn serve_feed(h: &Harness, second: bool) {
    let mut body = String::from_utf8(feed(&h.server.uri())).unwrap();
    if second {
        body = body.replace(
            "</channel>",
            "<item><title>Folge 2</title><guid isPermaLink=\"false\">ep-2</guid>\
             <enclosure url=\"https://cdn.example/2.mp3\" type=\"audio/mpeg\"/></item></channel>",
        );
    }
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresh_fetches_artwork_when_enabled() {
    let h = Harness::new().await;
    h.engine
        .set_setting("UGUISU_ARCHIVE_ARTWORK_FETCH", "true", Some("test"))
        .await
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tiny_png())
                .insert_header("content-type", "image/png"),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    serve_feed(&h, false).await;

    let podcast = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast;
    let art = h.engine.current_artwork(podcast.id).await.unwrap();
    assert_eq!(art.map(|a| a.format), Some(ArtworkFormat::Png));
    h.server.verify().await;
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unchanged_artwork_url_is_not_refetched() {
    let h = Harness::new().await;
    h.engine
        .set_setting("UGUISU_ARCHIVE_ARTWORK_FETCH", "true", Some("test"))
        .await
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tiny_png())
                .insert_header("content-type", "image/png"),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    serve_feed(&h, false).await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast;

    // A new episode changes the body; the cover URL stays the same.
    h.server.reset().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tiny_png()))
        .expect(0)
        .mount(&h.server)
        .await;
    serve_feed(&h, true).await;
    let report = h
        .engine
        .refresh_podcast(podcast.id, uguisu_engine::RefreshOptions::default())
        .await
        .unwrap();
    assert_eq!(
        report.outcome,
        uguisu_core::feed::RefreshOutcome::Fetched,
        "{report:?}"
    );
    assert_eq!(report.episodes.added, 1, "{report:?}");
    h.server.verify().await;
    h.engine.close().await;
}

/// A podcast whose feed names `/cover.png`, which serves `tiny_png`.
async fn podcast_with_cover(h: &Harness) -> Podcast {
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tiny_png())
                .insert_header("content-type", "image/png"),
        )
        .mount(&h.server)
        .await;
    serve_feed(h, false).await;
    h.engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast
}

/// Where `fetch_artwork` puts `tiny_png` for this podcast.
fn cover_path(h: &Harness, podcast: &Podcast) -> std::path::PathBuf {
    let hash = hex::encode(sha2::Sha256::digest(tiny_png()));
    let relative = uguisu_archive::layout::artwork_path(podcast.id, &hash, ArtworkFormat::Png);
    h.media_dir().join(relative.as_str())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrecorded_artwork_on_disk_adopted() {
    let h = Harness::new().await;
    let podcast = podcast_with_cover(&h).await;

    // A crash between the rename and the commit leaves the file and no row.
    let planted = cover_path(&h, &podcast);
    std::fs::create_dir_all(planted.parent().unwrap()).unwrap();
    std::fs::write(&planted, tiny_png()).unwrap();
    let written = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&planted)
        .unwrap()
        .set_modified(written)
        .unwrap();
    assert!(
        h.engine
            .current_artwork(podcast.id)
            .await
            .unwrap()
            .is_none()
    );

    let outcome = h.engine.fetch_artwork(podcast.id, false).await.unwrap();
    let ArtworkOutcome::Fetched(art) = outcome else {
        panic!("expected a fetch, got {outcome:?}");
    };
    assert_eq!(h.media_dir().join(&art.relative_path), planted);
    assert_eq!(
        std::fs::metadata(&planted).unwrap().modified().unwrap(),
        written,
        "the file on disk was adopted, not written again"
    );
    let names: Vec<_> = std::fs::read_dir(planted.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [planted.file_name().unwrap()]);
    assert_eq!(h.engine.artwork_history(podcast.id).await.unwrap().len(), 1);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leftover_artwork_tmp_is_kept() {
    let h = Harness::new().await;
    let podcast = podcast_with_cover(&h).await;

    // A crash during the write leaves a partial file under a temporary name.
    let cover = cover_path(&h, &podcast);
    let mut leftover = cover.clone().into_os_string();
    leftover.push(uguisu_archive::layout::tmp_suffix());
    let leftover = std::path::PathBuf::from(leftover);
    std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
    std::fs::write(&leftover, &tiny_png()[..10]).unwrap();

    let outcome = h.engine.fetch_artwork(podcast.id, false).await.unwrap();
    assert!(matches!(outcome, ArtworkOutcome::Fetched(_)), "{outcome:?}");
    assert_eq!(std::fs::read(&cover).unwrap(), tiny_png());
    assert_eq!(std::fs::read(&leftover).unwrap(), &tiny_png()[..10]);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_reports_bytes_before_tagging() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    let received = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let backup = tempfile::tempdir().unwrap();
    std::fs::copy(
        h.media_dir().join(&received.relative_path),
        backup.path().join("as-downloaded.mp3"),
    )
    .unwrap();
    h.engine
        .write_episode_tags(episodes[0], TagMode::Sync)
        .await
        .unwrap();
    let tagged = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    assert_ne!(
        tagged.size_bytes, received.size_bytes,
        "tags change the size"
    );
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::rename(
        h.media_dir().join(&tagged.relative_path),
        elsewhere.path().join("tagged.mp3"),
    )
    .unwrap();
    h.engine
        .verify_episode(episodes[0], VerifyDepth::Light)
        .await
        .unwrap();

    let plan = h
        .engine
        .restore(backup.path(), &RestoreOptions::default())
        .await
        .unwrap();
    assert_eq!(plan.items[0].action, RestoreAction::SourceOnly, "{plan:?}");
    assert_eq!(
        plan.items[0].source_path.as_deref(),
        Some("as-downloaded.mp3")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_copy_is_a_report_line() {
    let h = Harness::new().await;
    let (_, episodes) = setup(&h).await;
    let file = h.engine.archive_file(episodes[0]).await.unwrap().unwrap();
    let backup = tempfile::tempdir().unwrap();
    std::fs::rename(
        h.media_dir().join(&file.relative_path),
        backup.path().join("kept.mp3"),
    )
    .unwrap();
    h.engine
        .verify_episode(episodes[0], VerifyDepth::Light)
        .await
        .unwrap();
    // The scratch directory cannot be made: a file has its name.
    let scratch = h.media_dir().join(".uguisu").join("tmp");
    if scratch.is_dir() {
        std::fs::remove_dir(&scratch).unwrap();
    }
    std::fs::create_dir_all(scratch.parent().unwrap()).unwrap();
    std::fs::write(&scratch, b"in the way").unwrap();

    let done = h
        .engine
        .restore(
            backup.path(),
            &RestoreOptions {
                apply: true,
                ..RestoreOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(done.items[0].action, RestoreAction::Failed, "{done:?}");
    assert!(done.items[0].detail.is_some());
    assert!(
        backup.path().join("kept.mp3").is_file(),
        "the source is kept"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_artwork_address_is_refused() {
    let h = Harness::new().await;
    // The harness allows loopback for its own servers; a cloud metadata
    // address stays refused, before anything is sent to it.
    let body = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel>
<title>Private Cover</title><link>https://example.test/</link><description>d</description>
<itunes:image href="http://169.254.169.254/latest/meta-data/cover.png"/>
<item><title>One</title><guid isPermaLink="false">one</guid>
<enclosure url="https://media.example.test/one.mp3" length="1" type="audio/mpeg"/></item>
</channel></rss>"#;
    Mock::given(method("GET"))
        .and(path("/private.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/private.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast;

    let refused = h.engine.fetch_artwork(podcast.id, false).await;
    assert!(
        matches!(refused, Err(UguisuError::BlockedByPolicy(_))),
        "{refused:?}"
    );
}
