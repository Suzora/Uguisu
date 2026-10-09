//! Reading a foreign archive, over two fixture trees (ADR 0025, ADR 0050).
//!
//! The fixtures under `tests/fixtures/archives/podgrab/` are names other
//! tools write in Podgrab's directory-per-podcast shape, deliberately
//! awkward: dated and numbered names, a name with no date at all, two
//! titles that differ by one character, Unicode with typographic quotes,
//! several podcasts, a season marker, a name that says nothing, a text
//! file called `.mp3`, and a leftover under a control directory. Each one
//! is a way an import can go wrong quietly.
//!
//! `tests/fixtures/archives/podgrab-assets/` is what Podgrab itself writes
//! (ADR 0050): a folder named by `sanitize(podcast title)`, episodes named
//! `[<n>-][<YYYY-MM-DD>-]<kebab title>.mp3` with each combination of its two
//! naming settings, and its cover, NFO and `images/` beside them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use time::{Date, Month};
use uguisu_archive::import::{Candidate, EpisodeFacts, ImportFormat, classify};
use uguisu_archive::scan::{self, ScanOptions, ScannedFile};
use uguisu_core::ids::EpisodeId;

const THRESHOLD: u32 = 85;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/archives/podgrab")
}

fn scanned() -> Vec<ScannedFile> {
    let options = ScanOptions::default();
    scan::walk(&fixtures(), &options)
        .filter_map(Result::ok)
        .collect()
}

fn day(y: i32, m: u8, d: u8) -> Date {
    Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
}

fn observed(name: &str) -> Candidate {
    let reader = ImportFormat::Podgrab.reader();
    let file = scanned()
        .into_iter()
        .find(|f| f.relative.as_str() == name)
        .unwrap_or_else(|| panic!("fixture `{name}` is missing"));
    reader.observe(&file)
}

/// A digest of every file in the tree, so "the source was not touched"
/// can be asserted rather than assumed.
fn tree_digest() -> String {
    let mut hasher = Sha256::new();
    let mut entries: Vec<(String, Vec<u8>)> = walkdir_all(&fixtures())
        .into_iter()
        .map(|p| {
            let relative = p
                .strip_prefix(fixtures())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (relative, std::fs::read(&p).unwrap())
        })
        .collect();
    entries.sort();
    for (path, body) in entries {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(&body);
    }
    hex::encode(hasher.finalize())
}

fn walkdir_all(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn the_scan_sees_media_and_nothing_else() {
    let found: Vec<String> = scanned()
        .iter()
        .map(|f| f.relative.as_str().to_owned())
        .collect();
    assert_eq!(
        found,
        vec![
            "Darknet Diaries/2024-01-05 - 013 - Der Einbruch.mp3",
            "Darknet Diaries/2024-02-10 - 014 - Die Spur.mp3",
            "Darknet Diaries/Ohne Datum.mp3",
            "Der Fall/Der Fall, Teil 1.mp3",
            "Der Fall/Der Fall, Teil 2.mp3",
            "Der Fall/Der Fall.mp3",
            "Grüße aus Köln/S02E07 - Das Finale.mp3",
            "Grüße aus Köln/S02E08 - Nachspiel »Grüße«.mp3",
            "Unsortiert/1234567.mp3",
            "Unsortiert/kaputt.mp3",
        ],
        "artwork, side metadata and everything under `.uguisu` are not candidates"
    );

    // The text file named `.mp3` is scanned and visibly not media, so a
    // plan can report it instead of copying it into the archive.
    let fake = scanned()
        .into_iter()
        .find(|f| f.relative.as_str() == "Unsortiert/kaputt.mp3")
        .unwrap();
    assert!(!fake.looks_like_media());
    assert!(
        scanned()
            .iter()
            .filter(|f| f.relative.as_str() != "Unsortiert/kaputt.mp3")
            .all(ScannedFile::looks_like_media)
    );
}

#[test]
fn the_layout_comes_from_the_tree() {
    assert_eq!(ImportFormat::detect(&scanned()), ImportFormat::Podgrab);
}

#[test]
fn every_shape_of_name_is_read() {
    let c = observed("Darknet Diaries/2024-01-05 - 013 - Der Einbruch.mp3");
    assert_eq!(c.podcast_hint.as_deref(), Some("Darknet Diaries"));
    assert_eq!(c.title.as_deref(), Some("Der Einbruch"));
    assert_eq!(c.published, Some(day(2024, 1, 5)));
    assert_eq!(c.number, Some(13));

    // No date in the name: the field stays empty rather than being
    // invented from the modification time, which says when the file was
    // copied and nothing about when the episode was published.
    let c = observed("Darknet Diaries/Ohne Datum.mp3");
    assert_eq!(c.title.as_deref(), Some("Ohne Datum"));
    assert_eq!(c.published, None);
    assert_eq!(c.number, None);

    let c = observed("Grüße aus Köln/S02E07 - Das Finale.mp3");
    assert_eq!(c.podcast_hint.as_deref(), Some("Grüße aus Köln"));
    assert_eq!(c.season, Some(2));
    assert_eq!(c.number, Some(7));

    let c = observed("Grüße aus Köln/S02E08 - Nachspiel »Grüße«.mp3");
    assert_eq!(c.title.as_deref(), Some("S02E08 - Nachspiel »Grüße«"));
    assert_eq!(c.number, Some(8));

    // A name that carries nothing: reported as such, never padded out.
    let c = observed("Unsortiert/1234567.mp3");
    assert_eq!(c.title.as_deref(), Some("1234567"));
    assert_eq!(c.published, None);
    assert_eq!(c.season, None);
    assert_eq!(c.number, None);
}

#[test]
fn side_metadata_is_asked_by_name() {
    let reader = ImportFormat::Podgrab.reader();
    let file = scanned()
        .into_iter()
        .find(|f| f.relative.as_str() == "Darknet Diaries/Ohne Datum.mp3")
        .unwrap();
    let wanted: Vec<String> = reader
        .side_files(&file)
        .iter()
        .map(|p| p.as_str().to_owned())
        .collect();
    assert_eq!(
        wanted,
        vec![
            "Darknet Diaries/Ohne Datum.json",
            "Darknet Diaries/Ohne Datum.nfo"
        ]
    );

    // The caller reads it; this crate performs no I/O of its own.
    let text = std::fs::read_to_string(fixtures().join(&wanted[0])).unwrap();
    let mut candidate = reader.observe(&file);
    reader.refine(&mut candidate, &text);
    assert_eq!(candidate.published, Some(day(2024, 3, 1)));
    assert_eq!(candidate.duration_secs, Some(1800));
    assert_eq!(
        candidate.title.as_deref(),
        Some("Ohne Datum"),
        "the name still wins over a file sitting next to it"
    );
}

#[test]
fn the_ambiguous_file_stays_out() {
    // A small library, as the engine would have loaded it for one podcast.
    let einbruch = EpisodeFacts {
        id: EpisodeId::new(),
        title: "013 – Der Einbruch".to_owned(),
        published: Some(day(2024, 1, 5)),
        season: None,
        number: Some(13),
        duration_secs: None,
        enclosure_bytes: None,
    };
    let spur = EpisodeFacts {
        id: EpisodeId::new(),
        title: "014 – Die Spur".to_owned(),
        published: Some(day(2024, 2, 10)),
        number: Some(14),
        ..einbruch.clone()
    };
    let teil1 = EpisodeFacts {
        id: EpisodeId::new(),
        title: "Der Fall, Teil 1".to_owned(),
        published: None,
        season: None,
        number: None,
        duration_secs: None,
        enclosure_bytes: None,
    };
    let teil2 = EpisodeFacts {
        id: EpisodeId::new(),
        title: "Der Fall, Teil 2".to_owned(),
        ..teil1.clone()
    };
    let library = vec![einbruch.clone(), spur.clone(), teil1.clone(), teil2.clone()];

    let mut verdicts = BTreeMap::new();
    for file in scanned() {
        let c = ImportFormat::Podgrab.reader().observe(&file);
        verdicts.insert(
            file.relative.as_str().to_owned(),
            classify(&c, &library, THRESHOLD),
        );
    }

    assert_eq!(
        verdicts["Darknet Diaries/2024-01-05 - 013 - Der Einbruch.mp3"].episode_id(),
        Some(einbruch.id)
    );
    assert_eq!(
        verdicts["Darknet Diaries/2024-02-10 - 014 - Die Spur.mp3"].episode_id(),
        Some(spur.id)
    );

    // Two titles a character apart. A string metric alone rates them
    // about 0.97; comparing the digits separately is what keeps them
    // apart, and each file lands on its own episode.
    assert_eq!(
        verdicts["Der Fall/Der Fall, Teil 1.mp3"].episode_id(),
        Some(teil1.id),
        "{:?}",
        verdicts["Der Fall/Der Fall, Teil 1.mp3"]
    );
    assert_eq!(
        verdicts["Der Fall/Der Fall, Teil 2.mp3"].episode_id(),
        Some(teil2.id)
    );

    // The same file without the part number explains both equally well,
    // and nothing else is there to break the tie. Neither is chosen: a
    // wrong import is worse than an unresolved one.
    let v = &verdicts["Der Fall/Der Fall.mp3"];
    assert_eq!(v.state(), "ambiguous", "{v:?}");
    assert_eq!(v.episode_id(), None, "it must not be imported");

    // Nothing in the library explains these, and nothing is forced.
    for name in [
        "Unsortiert/1234567.mp3",
        "Unsortiert/kaputt.mp3",
        "Grüße aus Köln/S02E07 - Das Finale.mp3",
    ] {
        assert_eq!(verdicts[name].episode_id(), None, "{name}");
    }
}

#[test]
fn reading_a_foreign_archive_never_touches_it() {
    let before = tree_digest();
    // Everything an import does before it copies: scan, observe, read side
    // metadata, match.
    let library: Vec<EpisodeFacts> = Vec::new();
    for file in scanned() {
        let reader = ImportFormat::Podgrab.reader();
        let mut c = reader.observe(&file);
        for side in reader.side_files(&file) {
            if let Ok(text) = std::fs::read_to_string(fixtures().join(side.as_str())) {
                reader.refine(&mut c, &text);
            }
        }
        let _ = classify(&c, &library, THRESHOLD);
    }
    assert_eq!(
        tree_digest(),
        before,
        "the source archive belongs to the user and is read only"
    );
}

#[test]
fn real_podgrab_names_find_episodes() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/archives/podgrab-assets");
    let files: Vec<ScannedFile> = scan::walk(&root, &ScanOptions::default())
        .filter_map(Result::ok)
        .collect();
    // The cover, the NFO and `images/` are not media and never reach the matcher.
    assert_eq!(files.len(), 4, "{files:?}");
    assert_eq!(ImportFormat::detect(&files), ImportFormat::Podgrab);

    let episode = |title: &str, published: Date| EpisodeFacts {
        id: EpisodeId::new(),
        title: title.to_owned(),
        published: Some(published),
        season: None,
        number: None,
        duration_secs: None,
        enclosure_bytes: None,
    };
    let episodes = [
        episode("Der Einbruch", day(2024, 1, 5)),
        episode("Die Spur", day(2024, 2, 10)),
        episode("Folge 12: Das Finale", day(2024, 3, 1)),
        episode("Folge 11: Nachspiel", day(2024, 2, 20)),
    ];
    let expected: BTreeMap<&str, &str> = [
        ("der-einbruch.mp3", "Der Einbruch"),
        ("2024-02-10-die-spur.mp3", "Die Spur"),
        (
            "3-2024-03-01-folge-12-das-finale.mp3",
            "Folge 12: Das Finale",
        ),
        ("4-folge-11-nachspiel.mp3", "Folge 11: Nachspiel"),
    ]
    .into_iter()
    .collect();

    let reader = ImportFormat::Podgrab.reader();
    for file in &files {
        let candidate = reader.observe(file);
        assert_eq!(candidate.podcast_hint.as_deref(), Some("darknet-diaries"));
        let verdict = classify(&candidate, &episodes, THRESHOLD);
        let matched = episodes
            .iter()
            .find(|e| Some(e.id) == verdict.episode_id())
            .map(|e| e.title.as_str());
        assert_eq!(
            matched,
            expected.get(file.relative.file_name()).copied(),
            "{}: {verdict:?}",
            file.relative.as_str()
        );
    }
}
