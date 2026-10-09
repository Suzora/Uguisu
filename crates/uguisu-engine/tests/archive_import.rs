//! Importing a foreign archive through the engine.
//!
//! The property every test here defends: the source tree belongs to the
//! user. It is hashed before and after every operation, including the one
//! that copies from it, and it must come out byte for byte identical.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;

use std::path::{Path, PathBuf};

use common::{Harness, synthetic_feed_with_media, tree_digest};
use sha2::{Digest, Sha256};
use uguisu_archive::import::ImportFormat;
use uguisu_core::archive::{ArchiveOrigin, VerificationState, VerifyDepth, reason};
use uguisu_core::model::Episode;
use uguisu_engine::import::{Action, ImportOptions};
use uguisu_http::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Adds the synthetic podcast without downloading anything: an import has
/// a library to match against but an empty archive.
async fn library(h: &Harness, count: usize) -> Vec<Episode> {
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
}

/// A Podgrab-shaped tree named after the episodes, with a date prefix as
/// most downloaders write one.
fn source_tree(dir: &Path, episodes: &[Episode]) -> PathBuf {
    let root = dir.join("podgrab");
    let show = root.join("Synthetic Show");
    std::fs::create_dir_all(&show).unwrap();
    for (i, episode) in episodes.iter().enumerate() {
        let date = episode
            .published_at
            .map(|d| d.date().to_string())
            .unwrap_or_default();
        let title = episode.title.replace([':', '/'], "-");
        let name = format!("{date} - {title}.mp3");
        let mut body = b"ID3\x04".to_vec();
        body.extend(format!("uguisu import fixture {i}").bytes());
        std::fs::write(show.join(name), body).unwrap();
    }
    root
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dry_run_reads_everything_and_writes_nothing() {
    let h = Harness::new().await;
    let episodes = library(&h, 3).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);
    let before = tree_digest(&source);

    let plan = h
        .engine
        .import_plan(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert!(!plan.applied);
    assert_eq!(plan.format, ImportFormat::Podgrab);
    assert_eq!(plan.counts.scanned, 3);
    assert_eq!(
        plan.counts.imported, 3,
        "every file matched its episode: {:?}",
        plan.items
    );
    assert_eq!(plan.counts.ambiguous, 0);
    assert_eq!(plan.counts.unmatched, 0);

    for item in &plan.items {
        assert_eq!(item.action, Action::Import);
        assert!(item.episode_id.is_some());
        assert!(item.confidence >= plan.threshold, "{item:?}");
        let target = item.target_path.as_ref().expect("a planned path");
        assert!(
            !h.media_dir().join(target).exists(),
            "a dry run creates nothing"
        );
    }
    for episode in &episodes {
        assert!(h.engine.archive_file(episode.id).await.unwrap().is_none());
    }
    assert_eq!(tree_digest(&source), before, "the source was only read");
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn applying_copies_the_files_in_and_leaves_the_source_alone() {
    let h = Harness::new().await;
    let episodes = library(&h, 3).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);
    let before = tree_digest(&source);

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert!(report.applied);
    assert_eq!(report.counts.imported, 3, "{:?}", report.items);

    for episode in &episodes {
        let file = h
            .engine
            .archive_file(episode.id)
            .await
            .unwrap()
            .expect("a record");
        assert_eq!(file.origin, ArchiveOrigin::Import);
        assert_eq!(
            file.verification_state,
            VerificationState::Unchecked,
            "hashed while copying, but not checked afterwards"
        );
        assert_eq!(file.verification_reason.as_deref(), Some(reason::IMPORTED));
        assert_eq!(
            file.source_hash_value.as_ref(),
            Some(&file.hash_value),
            "an import's provenance is the bytes it found"
        );
        let copied = h.media_dir().join(&file.relative_path);
        assert!(copied.is_file());
        assert_eq!(
            hex::encode(Sha256::digest(std::fs::read(&copied).unwrap())),
            file.hash_value
        );
        // The sidecar was written, so the imported file describes itself
        // exactly as a downloaded one does.
        let sidecar = h.engine.read_sidecar(episode.id).await.unwrap().unwrap();
        assert_eq!(sidecar.archive.origin, ArchiveOrigin::Import);
        assert_eq!(sidecar.archive.hash_value, file.hash_value);
    }

    assert_eq!(
        tree_digest(&source),
        before,
        "copy, never move: the source archive is untouched"
    );
    assert!(
        h.engine.manifest_status().await.unwrap()[0].stale,
        "the index moved, so the manifest is behind"
    );

    // And the records stand up to a real check.
    let summary = h
        .engine
        .verify_all(
            &uguisu_storage::archive_files::ArchiveFilter::default(),
            VerifyDepth::Full,
        )
        .await
        .unwrap();
    assert_eq!(summary.verified, 3, "{summary:?}");
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn importing_the_same_tree_twice_changes_nothing_the_second_time() {
    let h = Harness::new().await;
    let episodes = library(&h, 2).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);

    h.engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    let first: Vec<_> = {
        let mut out = Vec::new();
        for e in &episodes {
            out.push(h.engine.archive_file(e.id).await.unwrap().unwrap());
        }
        out
    };

    let again = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(again.counts.already_present, 2, "{:?}", again.items);
    assert_eq!(again.counts.imported, 0);
    for (before, episode) in first.iter().zip(episodes.iter()) {
        let after = h.engine.archive_file(episode.id).await.unwrap().unwrap();
        assert_eq!(after.id, before.id, "the record kept its identity");
        assert_eq!(after.hash_value, before.hash_value);
        assert_eq!(after.relative_path, before.relative_path);
    }
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_that_could_be_two_episodes_is_never_imported() {
    let h = Harness::new().await;
    let episodes = library(&h, 3).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);

    // A name with no date and nothing that distinguishes one episode of
    // this show from another.
    std::fs::write(
        source.join("Synthetic Show").join("Episode.mp3"),
        b"ID3\x04ambiguous fixture",
    )
    .unwrap();
    // And something that is not media at all.
    std::fs::write(
        source.join("Synthetic Show").join("readme.mp3"),
        b"just some notes I kept next to the episodes",
    )
    .unwrap();
    let before = tree_digest(&source);

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    let ambiguous = report
        .items
        .iter()
        .find(|i| i.source_path.ends_with("Episode.mp3"))
        .expect("the ambiguous file is in the report");
    assert_eq!(ambiguous.action, Action::Ambiguous, "{ambiguous:?}");
    assert!(ambiguous.episode_id.is_none(), "and it was never placed");

    let invalid = report
        .items
        .iter()
        .find(|i| i.source_path.ends_with("readme.mp3"))
        .expect("the text file is in the report");
    assert_eq!(invalid.action, Action::Invalid, "{invalid:?}");

    assert_eq!(
        report.counts.imported, 3,
        "the three real ones still landed"
    );
    assert_eq!(
        tree_digest(&source),
        before,
        "nothing in the source was changed, not even what was refused"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn somebody_elses_file_at_the_target_is_never_overwritten() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);

    // Put an unrelated file exactly where the template would put the
    // episode.
    let plan = h
        .engine
        .import_plan(&source, &ImportOptions::default())
        .await
        .unwrap();
    let natural = plan.items[0].target_path.clone().unwrap();
    let full = h.media_dir().join(&natural);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, b"somebody else's file, thank you").unwrap();

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.counts.imported, 1, "{:?}", report.items);
    let landed = report.items[0].target_path.clone().unwrap();
    assert_ne!(
        landed, natural,
        "collision handling moved aside rather than writing over it"
    );
    assert_eq!(
        std::fs::read(&full).unwrap(),
        b"somebody else's file, thank you",
        "the file that was there is still there, byte for byte"
    );
    assert!(h.media_dir().join(&landed).is_file());

    // And when the disambiguated path is taken as well, there is nowhere
    // left to go: reported, and still nothing overwritten.
    let h2 = Harness::new().await;
    let episodes = library(&h2, 1).await;
    let outside2 = tempfile::tempdir().unwrap();
    let source2 = source_tree(outside2.path(), &episodes);
    let plan = h2
        .engine
        .import_plan(&source2, &ImportOptions::default())
        .await
        .unwrap();
    let natural = plan.items[0].target_path.clone().unwrap();
    for path in [natural.clone(), suffixed_form(&natural, episodes[0].id)] {
        let full = h2.media_dir().join(&path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, b"not yours either").unwrap();
    }
    let report = h2
        .engine
        .import_apply(&source2, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.counts.conflicts, 1, "{:?}", report.items);
    assert_eq!(report.counts.imported, 0);
    assert!(
        h2.engine
            .archive_file(episodes[0].id)
            .await
            .unwrap()
            .is_none()
    );
    h.engine.close().await;
    h2.engine.close().await;
}

/// The path collision handling would fall back to.
fn suffixed_form(path: &str, episode: uguisu_core::ids::EpisodeId) -> String {
    let suffix = uguisu_archive::collision::suffix_for(episode);
    let relative = uguisu_archive::path::RelativePath::parse(path).unwrap();
    uguisu_archive::collision::with_suffix(
        &relative,
        &suffix,
        uguisu_core::archive::PathProfile::Portable,
    )
    .unwrap()
    .as_str()
    .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_archive_cannot_be_imported_into_itself() {
    let h = Harness::new().await;
    library(&h, 1).await;
    let media = h.media_dir();
    std::fs::create_dir_all(media.join("inner")).unwrap();

    for candidate in [media.clone(), media.join("inner")] {
        let err = h
            .engine
            .import_plan(&candidate, &ImportOptions::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot be imported into itself"),
            "{err}"
        );
    }
    // A directory that simply is not there is refused too, rather than
    // silently importing nothing.
    assert!(
        h.engine
            .import_plan(Path::new("/nonexistent/uguisu"), &ImportOptions::default())
            .await
            .is_err()
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_already_in_place_gets_the_record_it_is_missing() {
    // What an import interrupted between the rename and the registration
    // leaves behind: the bytes are at the target path and nothing in the
    // database knows about them. Re-running converges instead of copying
    // a second time.
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);

    let plan = h
        .engine
        .import_plan(&source, &ImportOptions::default())
        .await
        .unwrap();
    let item = &plan.items[0];
    let target = h.media_dir().join(item.target_path.clone().unwrap());
    let source_file = source
        .join("Synthetic Show")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::copy(&source_file, &target).unwrap();

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.counts.imported, 1, "{:?}", report.items);
    assert!(
        report.items[0]
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("only the record is missing"),
        "{:?}",
        report.items[0]
    );
    let file = h
        .engine
        .archive_file(episodes[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(std::fs::read(&target).unwrap())),
        file.hash_value
    );
    h.engine.close().await;
}

/// The guard against importing the archive into itself has to hold on a
/// fresh install too, where the media directory has not been created yet.
///
/// It did not: the check canonicalised the media root and skipped itself
/// when that failed, which is exactly the state of every installation
/// before the first download. The source tree would then have been copied
/// into a directory inside itself.
#[tokio::test]
async fn importing_a_parent_directory_is_refused() {
    let h = Harness::new().await;
    let media = h.media_dir();
    let parent = media.parent().unwrap().to_path_buf();
    assert!(
        !media.exists(),
        "this test is only meaningful while the media directory is absent"
    );

    let err = h
        .engine
        .import_plan(&parent, &ImportOptions::default())
        .await
        .expect_err("a parent of the media directory is not an import source");
    assert!(err.to_string().contains("imported into itself"), "{err}");

    // And the directory itself, still absent, is refused for the same
    // reason rather than for "no such file".
    let err = h
        .engine
        .import_plan(&media, &ImportOptions::default())
        .await
        .expect_err("the media directory is never an import source");
    assert!(err.to_string().contains("import_source_invalid"), "{err}");
    h.engine.close().await;
}

/// Downloads every episode, so the archive holds Uguisu's own records.
async fn download_all(h: &Harness, episodes: &[Episode]) -> Vec<uguisu_core::archive::ArchiveFile> {
    for episode in episodes {
        h.engine
            .downloads()
            .enqueue_episode(episode.id, uguisu_core::download::Priority::Normal)
            .await
            .unwrap();
    }
    h.engine.start_downloads();
    h.engine.downloads().wait_idle().await.unwrap();
    // Registration, verification and the sidecar follow the download as
    // three writes; a record compared before the last is still moving.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let mut out = Vec::new();
        for episode in episodes {
            if let Some(file) = h.engine.archive_file(episode.id).await.unwrap()
                && file.verification_state == VerificationState::Verified
                && file.sidecar_written_at.is_some()
            {
                out.push(file);
            }
        }
        if out.len() == episodes.len() {
            return out;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "downloads never verified and described"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archived_episode_keeps_its_record() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let downloaded = download_all(&h, &episodes).await.remove(0);
    assert_eq!(downloaded.origin, ArchiveOrigin::Download);
    let on_disk = std::fs::read(h.media_dir().join(&downloaded.relative_path)).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    let item = &report.items[0];
    assert_eq!(item.action, Action::Conflict, "{item:?}");
    assert_eq!(
        item.target_path.as_deref(),
        Some(downloaded.relative_path.as_str())
    );
    assert!(
        item.detail
            .as_deref()
            .unwrap_or_default()
            .contains("already archived at"),
        "{item:?}"
    );
    assert_eq!(
        h.engine
            .archive_file(episodes[0].id)
            .await
            .unwrap()
            .unwrap(),
        downloaded,
        "the download's record is untouched"
    );
    assert_eq!(
        std::fs::read(h.media_dir().join(&downloaded.relative_path)).unwrap(),
        on_disk
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archived_bytes_are_already_present() {
    // Restoring an archived file that went missing is not an import's job:
    // the bytes are the record's, so the file is already accounted for.
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);
    h.engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    let archived = h
        .engine
        .archive_file(episodes[0].id)
        .await
        .unwrap()
        .unwrap();
    let full = h.media_dir().join(&archived.relative_path);
    std::fs::rename(&full, outside.path().join("moved-away.mp3")).unwrap();

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        report.items[0].action,
        Action::AlreadyPresent,
        "{:?}",
        report.items
    );
    assert!(
        report.items[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("archive restore")),
        "the plan points at the repair: {:?}",
        report.items[0].detail
    );
    assert_eq!(
        h.engine
            .archive_file(episodes[0].id)
            .await
            .unwrap()
            .unwrap(),
        archived
    );
    assert!(!full.exists(), "nothing was copied back");
    h.engine.close().await;
}

/// The source file of the first episode, and a second name for it in the
/// same directory.
fn twin(source: &Path) -> (PathBuf, PathBuf) {
    let first = source
        .join("Synthetic Show")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let second = first.with_extension("m4a");
    (first, second)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_files_one_episode_import_neither() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);
    let (_, second) = twin(&source);
    std::fs::write(&second, b"ID3\x04a different recording").unwrap();

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.counts.conflicts, 2, "{:?}", report.items);
    for item in &report.items {
        assert!(
            item.detail
                .as_deref()
                .unwrap_or_default()
                .contains("none was imported"),
            "{item:?}"
        );
    }
    assert!(
        h.engine
            .archive_file(episodes[0].id)
            .await
            .unwrap()
            .is_none()
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_copies_import_once() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);
    let (first, second) = twin(&source);
    std::fs::copy(&first, &second).unwrap();

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    let actions: Vec<Action> = report.items.iter().map(|i| i.action).collect();
    assert_eq!(
        actions,
        [Action::Import, Action::AlreadyPresent],
        "{:?}",
        report.items
    );
    assert!(
        report.items[1]
            .detail
            .as_deref()
            .unwrap_or_default()
            .starts_with("the same bytes as"),
        "{:?}",
        report.items[1]
    );
    let file = h
        .engine
        .archive_file(episodes[0].id)
        .await
        .unwrap()
        .unwrap();
    assert!(h.media_dir().join(&file.relative_path).is_file());
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_episodes_never_share_a_target() {
    let h = Harness::new().await;
    // Every episode of a year renders to one path.
    h.engine
        .set_setting(
            "UGUISU_ARCHIVE_TEMPLATE",
            "{podcast.title}/{episode.year}.{extension}",
            Some("test"),
        )
        .await
        .unwrap();
    let episodes = library(&h, 2).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);

    let plan = h
        .engine
        .import_plan(&source, &ImportOptions::default())
        .await
        .unwrap();
    let targets: Vec<_> = plan.items.iter().map(|i| i.target_path.clone()).collect();
    assert_eq!(plan.counts.imported, 2, "{:?}", plan.items);
    assert_ne!(targets[0], targets[1], "{targets:?}");

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.counts.imported, 2, "{:?}", report.items);
    for episode in &episodes {
        assert!(h.engine.archive_file(episode.id).await.unwrap().is_some());
    }
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn imported_event_names_the_match() {
    let h = Harness::new().await;
    let episodes = library(&h, 1).await;
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);
    let mut events = h.engine.subscribe();

    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    let item = &report.items[0];
    assert_eq!(item.action, Action::Import, "{item:?}");
    let mut seen = None;
    while let Some(event) = events.try_recv() {
        if let uguisu_core::events::EventKind::ArchiveImported {
            confidence,
            matched_by,
            ..
        } = event.kind
        {
            seen = Some((confidence, matched_by));
        }
    }
    assert_eq!(
        seen,
        Some((item.confidence, item.matched_by.clone().unwrap())),
        "the event says what the plan said"
    );
    assert!(
        item.confidence < 100,
        "a scored match, not a certain one: {item:?}"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_length_enclosure_is_ignored() {
    let h = Harness::new().await;
    let feed = String::from_utf8(synthetic_feed_with_media(
        1,
        h.media.base(),
        &["/normal/2048"],
    ))
    .unwrap();
    let start = feed.find("length=\"").unwrap() + "length=\"".len();
    let end = start + feed[start..].find('"').unwrap();
    let feed = format!("{}0{}", &feed[..start], &feed[end..]);
    Mock::given(method("GET"))
        .and(path("/zero.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(feed)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/zero.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast;
    let episode = h
        .engine
        .episodes(podcast.id, None, 1)
        .await
        .unwrap()
        .episodes[0]
        .clone();
    assert_eq!(episode.primary_enclosure().unwrap().length_bytes, Some(0));

    // Named by its title alone: no date, no number, only the title to go on.
    let outside = tempfile::tempdir().unwrap();
    let show = outside.path().join("podgrab").join("Synthetic Show");
    std::fs::create_dir_all(&show).unwrap();
    std::fs::write(
        show.join(format!("{}.mp3", episode.title.replace(':', "-"))),
        b"ID3\x04title only",
    )
    .unwrap();

    let plan = h
        .engine
        .import_plan(&outside.path().join("podgrab"), &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(plan.items[0].action, Action::Import, "{:?}", plan.items);
    h.engine.close().await;
}

/// Two episodes with a GUID each, no duration and no declared length, so
/// a file is matched on its title, date and tags alone.
async fn tagged_library(h: &Harness) -> Vec<Episode> {
    let feed = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>Synthetic Show</title><link>https://synthetic.example/</link>
<description>Tags</description>
<item><title>Der Einbruch</title><guid isPermaLink="false">guid-einbruch</guid>
<pubDate>Fri, 05 Jan 2024 10:00:00 +0000</pubDate>
<enclosure url="https://cdn.synthetic.example/einbruch.mp3" type="audio/mpeg" length="0"/></item>
<item><title>Die Spur</title><guid isPermaLink="false">guid-spur</guid>
<pubDate>Sat, 10 Feb 2024 10:00:00 +0000</pubDate>
<enclosure url="https://cdn.synthetic.example/spur.mp3" type="audio/mpeg" length="0"/></item>
</channel></rss>"#;
    Mock::given(method("GET"))
        .and(path("/tagged.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(feed)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    let podcast = h
        .engine
        .add_podcast(&h.url("/tagged.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast;
    h.engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes
}

/// A real MP3 at `dir/Synthetic Show/<name>`, carrying these tags.
fn tagged_file(dir: &Path, name: &str, tags: &[(uguisu_metadata::Field, &str)]) -> PathBuf {
    let show = dir.join("Synthetic Show");
    std::fs::create_dir_all(&show).unwrap();
    let path = show.join(name);
    let mut silence = Vec::new();
    for _ in 0..20 {
        silence.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        silence.extend(std::iter::repeat_n(0u8, 413));
    }
    std::fs::write(&path, silence).unwrap();
    let mut set = uguisu_metadata::TagSet::default();
    for (field, value) in tags {
        set.set(*field, *value);
    }
    uguisu_metadata::write_tags(&path, &set, uguisu_core::archive::TagMode::Sync).unwrap();
    path
}

fn episode_titled<'a>(episodes: &'a [Episode], title: &str) -> &'a Episode {
    episodes.iter().find(|e| e.title == title).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn embedded_guid_names_the_episode() {
    let h = Harness::new().await;
    let episodes = tagged_library(&h).await;
    let outside = tempfile::tempdir().unwrap();
    tagged_file(
        outside.path(),
        "track07.mp3",
        &[(uguisu_metadata::Field::EpisodeGuid, "guid-spur")],
    );

    let plan = h
        .engine
        .import_plan(outside.path(), &ImportOptions::default())
        .await
        .unwrap();
    let item = &plan.items[0];
    assert_eq!(item.action, Action::Import, "{item:?}");
    assert_eq!(
        item.episode_id,
        Some(episode_titled(&episodes, "Die Spur").id)
    );
    assert_eq!(item.matched_by.as_deref(), Some("embedded_guid"));
    assert_eq!(item.confidence, 100);
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tag_title_rescues_a_mangled_name() {
    let h = Harness::new().await;
    let episodes = tagged_library(&h).await;
    let outside = tempfile::tempdir().unwrap();
    tagged_file(
        outside.path(),
        "x7f3.mp3",
        &[(uguisu_metadata::Field::Title, "Der Einbruch")],
    );

    let plan = h
        .engine
        .import_plan(outside.path(), &ImportOptions::default())
        .await
        .unwrap();
    let item = &plan.items[0];
    assert_eq!(item.action, Action::Import, "{item:?}");
    assert_eq!(
        item.episode_id,
        Some(episode_titled(&episodes, "Der Einbruch").id)
    );
    assert_eq!(item.matched_by.as_deref(), Some("scored"));
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contradicted_guid_is_ambiguous() {
    let h = Harness::new().await;
    tagged_library(&h).await;
    let outside = tempfile::tempdir().unwrap();
    // The name and the date say one episode, a stale tag says the other.
    tagged_file(
        outside.path(),
        "2024-01-05-der-einbruch.mp3",
        &[(uguisu_metadata::Field::EpisodeGuid, "guid-spur")],
    );

    let plan = h
        .engine
        .import_plan(outside.path(), &ImportOptions::default())
        .await
        .unwrap();
    let item = &plan.items[0];
    assert_eq!(item.action, Action::Ambiguous, "{item:?}");
    assert!(
        item.detail
            .as_deref()
            .unwrap_or_default()
            .contains("embedded episode GUID"),
        "{item:?}"
    );
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broken_tags_change_nothing() {
    let h = Harness::new().await;
    let episodes = tagged_library(&h).await;
    let outside = tempfile::tempdir().unwrap();
    let show = outside.path().join("Synthetic Show");
    std::fs::create_dir_all(&show).unwrap();
    // Looks like ID3 to the scan, and lofty cannot read a tag out of it.
    std::fs::write(
        show.join("2024-02-10-die-spur.mp3"),
        b"ID3\x04 not really a tag",
    )
    .unwrap();

    let plan = h
        .engine
        .import_plan(outside.path(), &ImportOptions::default())
        .await
        .unwrap();
    let item = &plan.items[0];
    assert_eq!(item.action, Action::Import, "{item:?}");
    assert_eq!(
        item.episode_id,
        Some(episode_titled(&episodes, "Die Spur").id)
    );
    assert_eq!(item.matched_by.as_deref(), Some("scored"));
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn imported_episode_is_not_downloaded() {
    use uguisu_core::UguisuError;
    use uguisu_core::download::Priority;

    let h = Harness::new().await;
    let episodes = library(&h, 2).await;
    // A job the import knows nothing about: queued, then cancelled.
    let job = h
        .engine
        .downloads()
        .enqueue_episode(episodes[1].id, Priority::Normal)
        .await
        .unwrap()
        .job()
        .clone();
    let outside = tempfile::tempdir().unwrap();
    let source = source_tree(outside.path(), &episodes);
    let plan = h
        .engine
        .import_plan(&source, &ImportOptions::default())
        .await
        .unwrap();
    let queued = plan
        .items
        .iter()
        .find(|i| i.episode_id == Some(episodes[1].id))
        .unwrap();
    assert!(
        queued
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("still queued")),
        "the plan names the job that would fetch the file again: {queued:?}"
    );
    h.engine.downloads().cancel(job.id).await.unwrap();
    let report = h
        .engine
        .import_apply(&source, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.counts.imported, 2, "{:?}", report.items);

    let refused = h
        .engine
        .downloads()
        .enqueue_episode(episodes[0].id, Priority::Normal)
        .await;
    assert!(
        matches!(refused, Err(UguisuError::Conflict(_))),
        "{refused:?}"
    );
    assert!(
        h.engine
            .downloads()
            .job_for_episode(episodes[0].id)
            .await
            .unwrap()
            .is_none()
    );
    let all = h
        .engine
        .downloads()
        .enqueue_podcast(episodes[0].podcast_id, Priority::Normal)
        .await
        .unwrap();
    assert_eq!(all.created + all.requeued, 0, "{all:?}");
    assert!(
        all.skipped.iter().all(|s| s.reason == "already_archived") && all.skipped.len() == 2,
        "{all:?}"
    );
    let retried = h.engine.downloads().retry(job.id).await;
    assert!(
        matches!(retried, Err(UguisuError::Conflict(_))),
        "{retried:?}"
    );
    h.engine.close().await;
}

/// Makes `link` point at the directory `target`: a symlink on Unix, a
/// junction on Windows, which needs no privilege.
fn link_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_media_root_is_refused() {
    let h = Harness::new().await;
    library(&h, 1).await;
    std::fs::create_dir_all(h.media_dir().join("inner")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    for (name, target) in [
        ("root", h.media_dir()),
        ("inner", h.media_dir().join("inner")),
    ] {
        let link = outside.path().join(name);
        link_dir(&target, &link);
        let refused = h
            .engine
            .import_plan(&link, &ImportOptions::default())
            .await
            .unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("cannot be imported into itself"),
            "{name}: {refused}"
        );
    }
    h.engine.close().await;
}
