//! The portable half of the archive, checked against the outside world.
//!
//! A manifest is only worth writing if a tool that has never heard of
//! Uguisu can check it, so the interoperability test really runs
//! `sha256sum -c` over a real directory. A sidecar is only worth writing
//! if it survives being copied somewhere else, so the round-trip tests go
//! through bytes rather than through the struct.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The interoperability check reports on stderr when it has to skip: a
// silent pass would look like the format was verified when it was not.
#![allow(clippy::print_stderr)]

use std::collections::BTreeMap;

use proptest::prelude::*;
use time::OffsetDateTime;
use uguisu_archive::path::RelativePath;
use uguisu_archive::{layout, manifest, sidecar};
use uguisu_core::archive::{
    ArchiveOrigin, ManifestEntry, Sidecar, SidecarArchive, SidecarEpisode, SidecarPodcast, TagState,
};
use uguisu_core::ids::{EpisodeId, PodcastId};

const P: &str = "01J0000000000000000000000P";

fn podcast() -> PodcastId {
    P.parse().unwrap()
}

fn at() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

/// Writes real files, renders a manifest of them from their real digests,
/// and asks coreutils whether it agrees.
#[cfg(unix)]
#[test]
fn sha256sum_can_check_the_manifest() {
    use sha2::{Digest, Sha256};

    if std::process::Command::new("sha256sum")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("sha256sum is not installed; skipping the interoperability check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Names a naive writer would get wrong: spaces, a hash character that
    // must not read as a comment, and non-ASCII text.
    let files = [
        ("Show/2024/2024-01-05 - Folge 1.mp3", "eins"),
        ("Show/2024/2024-02-10 - Grüße aus »Köln«.mp3", "zwei"),
        ("Show/2024/#3 - Nummernzeichen.mp3", "drei"),
    ];
    let mut entries = Vec::new();
    for (path, body) in files {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, body).unwrap();
        entries.push(ManifestEntry {
            relative_path: path.to_owned(),
            hash_value: hex::encode(Sha256::digest(body.as_bytes())),
        });
    }
    let written = manifest::write_atomic(root, podcast(), at(), {
        let mut sorted = entries.clone();
        sorted.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        sorted
    })
    .unwrap();

    let out = std::process::Command::new("sha256sum")
        .arg("-c")
        .arg(written.relative_path.as_str())
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sha256sum -c refused Uguisu's manifest:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).lines().count(),
        3,
        "every listed file was checked, and the comment header was skipped"
    );

    // And it notices when a file changes, which is the whole point.
    std::fs::write(root.join(files[0].0), "geändert").unwrap();
    let out = std::process::Command::new("sha256sum")
        .arg("-c")
        .arg(written.relative_path.as_str())
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("FAILED"));
}

#[test]
fn a_manifest_lists_media_only() {
    // The rendered document only ever contains what the caller passed, and
    // the reader refuses to accept control paths back — so a manifest can
    // never come to list its own directory, a sidecar or artwork.
    let good = hex::encode([1; 32]);
    for path in [
        ".uguisu/manifests/01J/manifest.sha256",
        ".uguisu/artwork/01J/ab.jpg",
        "Show/.uguisu-tmp/job.part",
    ] {
        let line = format!("{good}  {path}");
        assert!(
            manifest::parse_line(&line, 1).is_err(),
            "`{path}` is bookkeeping, not an artifact"
        );
    }
    // A sidecar is not refused by the reader — it is simply never written
    // into a manifest, because only media has an archive record.
    let sidecar_path = RelativePath::parse("Show/a.mp3.json").unwrap();
    assert!(layout::is_sidecar_path(&sidecar_path));
    assert!(!layout::is_control_path(&sidecar_path));
}

#[test]
fn a_stale_manifest_reports_exactly_what_moved() {
    let listed: Vec<ManifestEntry> = ["Show/a.mp3", "Show/b.mp3", "Show/c.mp3"]
        .iter()
        .enumerate()
        .map(|(i, p)| ManifestEntry {
            relative_path: (*p).to_owned(),
            hash_value: hex::encode([u8::try_from(i).unwrap(); 32]),
        })
        .collect();
    let found: BTreeMap<String, Option<String>> = [
        ("Show/a.mp3".to_owned(), Some(hex::encode([0; 32]))),
        ("Show/b.mp3".to_owned(), Some(hex::encode([99; 32]))),
        ("Show/d.mp3".to_owned(), Some(hex::encode([3; 32]))),
    ]
    .into_iter()
    .collect();
    let diff = manifest::compare(&listed, &found);
    assert_eq!(diff.unchanged, 1);
    assert_eq!(diff.changed.sample, vec!["Show/b.mp3"]);
    assert_eq!(diff.missing.sample, vec!["Show/c.mp3"]);
    assert_eq!(diff.added.sample, vec!["Show/d.mp3"]);
    assert!(!diff.is_clean());
}

fn sample_sidecar(path: &str) -> Sidecar {
    Sidecar {
        schema: Sidecar::SCHEMA,
        generator: Sidecar::GENERATOR.to_owned(),
        written_at: at(),
        podcast: SidecarPodcast {
            id: podcast(),
            title: "Show".to_owned(),
            author: None,
            publisher: None,
            feed_url: None,
            language: None,
            categories: vec![],
        },
        episode: SidecarEpisode {
            id: EpisodeId::new(),
            identity_key: "guid:x".to_owned(),
            identity_source: Some("guid".to_owned()),
            title: "Folge 1".to_owned(),
            published_at: Some(at()),
            season: None,
            number: None,
            duration_secs: None,
            description_text: None,
            guid: None,
            link: None,
            enclosure_url: None,
            enclosure_type: None,
            enclosure_length_bytes: None,
            artwork_url: None,
            chapters: Vec::new(),
            transcripts: Vec::new(),
        },
        archive: SidecarArchive {
            relative_path: path.to_owned(),
            size_bytes: 4,
            hash_algo: "sha256".to_owned(),
            hash_value: hex::encode([1; 32]),
            content_type: None,
            sniffed_type: None,
            origin: ArchiveOrigin::Download,
            tag_state: TagState::Untagged,
            tag_mode: None,
            tagged_at: None,
            registered_at: at(),
            original_tags: None,
        },
        source: None,
    }
}

#[test]
fn a_sidecar_travels_with_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let media = RelativePath::parse("Show/2024/Folge 1.mp3").unwrap();
    std::fs::create_dir_all(root.join("Show/2024")).unwrap();
    std::fs::write(root.join(media.as_str()), b"eins").unwrap();

    let written = sidecar::write_atomic(root, &media, &sample_sidecar(media.as_str())).unwrap();
    assert_eq!(written, layout::sidecar_of(&media));

    // Copy the pair somewhere else, as a user moving one episode would,
    // and the document still reads — its meaning does not depend on the
    // database or on where it sits.
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::copy(
        root.join(written.as_str()),
        elsewhere.path().join("Folge 1.mp3.json"),
    )
    .unwrap();
    let moved = RelativePath::parse("Folge 1.mp3.json").unwrap();
    let read = sidecar::read(elsewhere.path(), &moved).unwrap().unwrap();
    assert_eq!(read.episode.title, "Folge 1");
    assert_eq!(read.archive.hash_value, hex::encode([1; 32]));
    assert_eq!(
        layout::media_of(&moved).unwrap().as_str(),
        "Folge 1.mp3",
        "the media file it belongs to is derivable from the name alone"
    );
}

proptest! {
    /// Whatever order the rows arrive in, the same set of artifacts must
    /// render byte-identically: the manifest is a set, not a log, and a
    /// file that changed only because SQLite paged differently would make
    /// every `stale` decision meaningless.
    #[test]
    fn rendering_is_order_independent_and_re_readable(
        mut paths in proptest::collection::hash_set("[a-z][a-z0-9 ]{0,12}\\.mp3", 1..12),
        shuffle in 0usize..64,
    ) {
        let entries: Vec<ManifestEntry> = paths
            .drain()
            .enumerate()
            .map(|(i, p)| ManifestEntry {
                relative_path: format!("Show/{p}"),
                hash_value: hex::encode([u8::try_from(i % 256).unwrap(); 32]),
            })
            .collect();
        let canonical = manifest::render(podcast(), at(), entries.clone());

        let mut rotated = entries.clone();
        rotated.rotate_left(shuffle % entries.len().max(1));
        rotated.reverse();
        prop_assert_eq!(&manifest::render(podcast(), at(), rotated), &canonical);

        let back = manifest::parse(&canonical).unwrap();
        prop_assert_eq!(back.len(), entries.len());
        let mut expected = entries;
        expected.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        prop_assert_eq!(back, expected);
    }
}
