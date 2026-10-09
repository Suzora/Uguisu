//! Reading a Podgrab archive (ADR 0025, ADR 0050).
//!
//! Podgrab writes one directory per podcast and one file per episode, as
//! read from its source:
//!
//! ```text
//! <sanitize(podcast title)>/[<n>-][<YYYY-MM-DD>-]<kebab(sanitize(episode title))>.<ext>
//! ```
//!
//! `sanitize` lowercases, transliterates accents, turns `[ &_=+:]` into
//! `-` and drops everything but ASCII letters, digits, `-` and `.`. The two
//! prefixes are settings, both off by default; `<n>` is Podgrab's own
//! counter, not the feed's episode number, and comes before the date. The
//! detector looks at where files sit and `observe` at their names; folding
//! titles for comparison undoes the kebab case. The engine can also read
//! Podgrab's database, which names each file exactly; that lives there,
//! because this crate does no I/O.
//!
//! Names other tools write in this shape (`2024-01-05 - 013 - Title.mp3`)
//! are read with the helpers every layout shares in [`super::normalize`].
//!
//! Side metadata (`<episode>.json`, `<episode>.nfo`, which companion
//! tooling writes) is *requested* here and *read* by the engine: this
//! crate performs no I/O. What comes back is a stranger's file, so it is
//! size-capped by the caller and every field it might supply is optional.

use serde::Deserialize;

use super::normalize;
use super::{Candidate, SourceFormat};
use crate::path::RelativePath;
use crate::scan::ScannedFile;

/// Largest side-metadata file worth reading, for the caller to enforce.
pub const MAX_SIDE_BYTES: usize = 64 * 1024;

/// Reads Podgrab's directory-per-podcast layout.
#[derive(Debug, Clone, Copy)]
pub struct PodgrabFormat;

impl SourceFormat for PodgrabFormat {
    fn id(&self) -> &'static str {
        "podgrab"
    }

    fn detect(&self, sample: &[ScannedFile]) -> u32 {
        if sample.is_empty() {
            return 0;
        }
        // Podgrab's shape is exactly `<podcast>/<episode>.<ext>`: one
        // directory level, and the same handful of directories repeated.
        let at_depth_two = sample
            .iter()
            .filter(|f| f.relative.components().count() == 2)
            .count();
        let share = at_depth_two * 100 / sample.len();
        if share < 80 {
            return 0;
        }
        // A tree where every file sits in its own directory is not a
        // podcast archive with a few shows in it; it is something else
        // that happens to be two levels deep.
        let directories: std::collections::BTreeSet<&str> =
            sample.iter().filter_map(ScannedFile::parent_name).collect();
        if directories.len() == sample.len() && sample.len() > 2 {
            return 20;
        }
        u32::try_from(share).unwrap_or(100)
    }

    fn observe(&self, file: &ScannedFile) -> Candidate {
        let mut candidate = Candidate::from_file(file);
        // The directory is the podcast: the one the file sits in, so a root
        // above Podgrab's own data directory still names the right show.
        candidate.podcast_hint = file.parent_name().map(str::to_owned);

        // Podgrab's counter numbers its downloads, not the feed's episodes,
        // so with the date behind it, it is dropped.
        let stem = match counter(file.stem()) {
            Some(rest) if normalize::strip_leading_date(rest).0.is_some() => rest,
            _ => file.stem(),
        };
        let (published, rest) = normalize::strip_leading_date(stem);
        let (number, rest) = normalize::strip_track_prefix(rest);
        let (season, episode) = normalize::season_episode(stem);
        candidate.published = published;
        candidate.season = season;
        candidate.number = episode.or(number);
        let title = rest.trim();
        candidate.title = (!title.is_empty()).then(|| title.to_owned());
        // A lone leading number is Podgrab's counter or the title's own:
        // `123: Intro` is written `123-intro` too. The title keeps it, and
        // the reading without it is tried when that does not match.
        if published.is_none()
            && number.is_none()
            && let Some(rest) = counter(title)
        {
            candidate.fallback_titles.push(rest.to_owned());
        }
        candidate
    }

    fn side_files(&self, file: &ScannedFile) -> Vec<RelativePath> {
        let raw = file.relative.as_str();
        let stem_end = raw.len() - file.extension().map_or(0, |e| e.len() + 1);
        let Some(stem) = raw.get(..stem_end) else {
            return Vec::new();
        };
        ["json", "nfo"]
            .iter()
            .filter_map(|ext| RelativePath::parse(&format!("{stem}.{ext}")).ok())
            .collect()
    }

    fn refine(&self, candidate: &mut Candidate, side: &str) {
        let read = if side.trim_start().starts_with('{') {
            from_json(side)
        } else {
            from_nfo(side)
        };
        // Side metadata refines, it never overrides: what the name said is
        // what the user's own tool put there, and a stray metadata file
        // copied next to the wrong episode must not be able to rename it.
        if candidate.title.is_none() {
            candidate.title = read.title;
        }
        if candidate.published.is_none() {
            candidate.published = read.published;
        }
        if candidate.season.is_none() {
            candidate.season = read.season;
        }
        if candidate.number.is_none() {
            candidate.number = read.number;
        }
        if candidate.duration_secs.is_none() {
            candidate.duration_secs = read.duration_secs;
        }
    }
}

/// The rest of a name after a leading `<digits>-`, when there is a rest.
fn counter(name: &str) -> Option<&str> {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 5 {
        return None;
    }
    name[digits..]
        .strip_prefix('-')
        .filter(|rest| !rest.is_empty())
}

/// What a side-metadata file was able to say.
#[derive(Debug, Default, PartialEq, Eq)]
struct SideMetadata {
    title: Option<String>,
    published: Option<time::Date>,
    season: Option<u32>,
    number: Option<u32>,
    duration_secs: Option<u32>,
}

/// The handful of fields companion tools agree on, all optional.
#[derive(Debug, Default, Deserialize)]
struct SideJson {
    #[serde(default)]
    title: Option<String>,
    #[serde(default, alias = "pubDate", alias = "published")]
    pub_date: Option<String>,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default, alias = "episode")]
    episode_number: Option<u32>,
    #[serde(default, alias = "duration")]
    duration_secs: Option<u32>,
}

fn from_json(text: &str) -> SideMetadata {
    let Ok(parsed) = serde_json::from_str::<SideJson>(text) else {
        return SideMetadata::default();
    };
    SideMetadata {
        title: parsed.title.filter(|t| !t.trim().is_empty()),
        published: parsed.pub_date.as_deref().and_then(leading_date),
        season: parsed.season,
        number: parsed.episode_number,
        duration_secs: parsed.duration_secs,
    }
}

/// Reads five known element names out of an NFO file.
///
/// Deliberately not an XML parser. An NFO is a stranger's file used here
/// only to fill in fields the name did not carry, so a scan for known
/// elements that gives up on anything unexpected is the right amount of
/// trust — and it keeps a full XML parser out of this crate's dependency
/// list and out of reach of untrusted input.
fn from_nfo(text: &str) -> SideMetadata {
    let element = |name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = text.find(&open)? + open.len();
        let end = text.get(start..)?.find(&close)? + start;
        let value = text.get(start..end)?.trim();
        (!value.is_empty()).then(|| value.to_owned())
    };
    SideMetadata {
        title: element("title"),
        published: element("aired")
            .or_else(|| element("releasedate"))
            .as_deref()
            .and_then(leading_date),
        season: element("season").and_then(|v| v.parse().ok()),
        number: element("episode").and_then(|v| v.parse().ok()),
        // Kodi writes runtime in minutes.
        duration_secs: element("runtime")
            .and_then(|v| v.parse::<u32>().ok())
            .and_then(|m| m.checked_mul(60)),
    }
}

fn leading_date(value: &str) -> Option<time::Date> {
    normalize::strip_leading_date(value.trim()).0
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use time::{Date, Month};

    use super::*;

    fn file(path: &str) -> ScannedFile {
        ScannedFile {
            relative: RelativePath::parse(path).unwrap(),
            absolute: PathBuf::from(path),
            size_bytes: 2048,
            mtime_unix: Some(1_700_000_000),
            head: b"ID3\x04............".to_vec(),
        }
    }

    fn day(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    #[test]
    fn podgrab_name_prefixes_are_read() {
        for (name, title, date, fallback) in [
            ("show/der-einbruch.mp3", "der-einbruch", None, None),
            (
                "show/2024-01-05-der-einbruch.mp3",
                "der-einbruch",
                Some(day(2024, 1, 5)),
                None,
            ),
            (
                "show/13-2024-01-05-der-einbruch.mp3",
                "der-einbruch",
                Some(day(2024, 1, 5)),
                None,
            ),
            (
                "show/13-der-einbruch.mp3",
                "13-der-einbruch",
                None,
                Some("der-einbruch"),
            ),
            (
                "show/2020-in-review.mp3",
                "2020-in-review",
                None,
                Some("in-review"),
            ),
        ] {
            let c = PodgrabFormat.observe(&file(name));
            assert_eq!(c.title.as_deref(), Some(title), "{name}");
            assert_eq!(c.published, date, "{name}");
            assert_eq!(
                c.number, None,
                "{name}: Podgrab's counter is not the episode number"
            );
            assert_eq!(
                c.fallback_titles.first().map(String::as_str),
                fallback,
                "{name}"
            );
        }
    }

    #[test]
    fn parent_directory_names_the_podcast() {
        for name in [
            "darknet-diaries/der-einbruch.mp3",
            "assets/darknet-diaries/der-einbruch.mp3",
        ] {
            let c = PodgrabFormat.observe(&file(name));
            assert_eq!(c.podcast_hint.as_deref(), Some("darknet-diaries"), "{name}");
        }
        assert_eq!(
            PodgrabFormat.observe(&file("lonely.mp3")).podcast_hint,
            None
        );
    }

    #[test]
    fn directory_is_podcast_name_is_episode() {
        let c = PodgrabFormat.observe(&file("Darknet Diaries/Der große Fall.mp3"));
        assert_eq!(c.podcast_hint.as_deref(), Some("Darknet Diaries"));
        assert_eq!(c.title.as_deref(), Some("Der große Fall"));
        assert_eq!(c.published, None);
        assert_eq!(c.number, None);
    }

    #[test]
    fn a_configured_scheme_uses_shared_helpers() {
        for (name, title, date, number) in [
            (
                "Show/2024-01-05 - Folge 13.mp3",
                "Folge 13",
                Some(day(2024, 1, 5)),
                None,
            ),
            ("Show/013 - Folge.mp3", "Folge", None, Some(13)),
            (
                "Show/2024-01-05 013 - Folge.mp3",
                "Folge",
                Some(day(2024, 1, 5)),
                Some(13),
            ),
        ] {
            let c = PodgrabFormat.observe(&file(name));
            assert_eq!(c.title.as_deref(), Some(title), "{name}");
            assert_eq!(c.published, date, "{name}");
            assert_eq!(c.number, number, "{name}");
        }
    }

    #[test]
    fn the_layout_declines_when_unsure() {
        let podgrab: Vec<ScannedFile> = [
            "Show A/a.mp3",
            "Show A/b.mp3",
            "Show A/c.mp3",
            "Show B/d.mp3",
            "Show B/e.mp3",
        ]
        .iter()
        .map(|p| file(p))
        .collect();
        assert!(PodgrabFormat.detect(&podgrab) >= 80);
        assert_eq!(
            super::super::ImportFormat::detect(&podgrab),
            super::super::ImportFormat::Podgrab
        );

        // Deeply nested: not this layout.
        let nested: Vec<ScannedFile> = ["Show/2024/a.mp3", "Show/2024/b.mp3"]
            .iter()
            .map(|p| file(p))
            .collect();
        assert_eq!(PodgrabFormat.detect(&nested), 0);

        // One directory per file is some other tool's idea, not a library
        // of a few shows.
        let per_file: Vec<ScannedFile> = ["a/a.mp3", "b/b.mp3", "c/c.mp3", "d/d.mp3"]
            .iter()
            .map(|p| file(p))
            .collect();
        assert_eq!(PodgrabFormat.detect(&per_file), 20);
        assert_eq!(PodgrabFormat.detect(&[]), 0);
    }

    #[test]
    fn side_metadata_fills_gaps_only() {
        let f = file("Show/Folge.mp3");
        assert_eq!(
            PodgrabFormat
                .side_files(&f)
                .iter()
                .map(|p| p.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec!["Show/Folge.json", "Show/Folge.nfo"]
        );

        let mut c = PodgrabFormat.observe(&f);
        assert_eq!(c.title.as_deref(), Some("Folge"));
        PodgrabFormat.refine(
            &mut c,
            r#"{"title":"Ein anderer Titel","pubDate":"2024-01-05T10:00:00Z","episode":7,"duration":1800}"#,
        );
        assert_eq!(
            c.title.as_deref(),
            Some("Folge"),
            "a metadata file next to the wrong episode must not rename it"
        );
        assert_eq!(c.published, Some(day(2024, 1, 5)));
        assert_eq!(c.number, Some(7));
        assert_eq!(c.duration_secs, Some(1800));
    }

    #[test]
    fn an_nfo_yields_its_known_fields() {
        let mut c = Candidate::empty();
        PodgrabFormat.refine(
            &mut c,
            "<episodedetails><title>Das Finale</title><aired>2024-02-10</aired>\
             <season>2</season><episode>7</episode><runtime>30</runtime></episodedetails>",
        );
        assert_eq!(c.title.as_deref(), Some("Das Finale"));
        assert_eq!(c.published, Some(day(2024, 2, 10)));
        assert_eq!(c.season, Some(2));
        assert_eq!(c.number, Some(7));
        assert_eq!(c.duration_secs, Some(1800), "runtime is in minutes");
    }

    #[test]
    fn unreadable_side_metadata_changes_nothing() {
        let before = Candidate::empty();
        for text in [
            "{ not json",
            "<episodedetails><title></title></episodedetails>",
            "",
            "just some text",
            "{\"title\": 42}",
        ] {
            let mut c = before.clone();
            PodgrabFormat.refine(&mut c, text);
            assert_eq!(c, before, "`{text}` must not be guessed at");
        }
    }
}
