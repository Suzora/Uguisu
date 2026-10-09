//! Reading someone else's archive (ADR 0025).
//!
//! The pure half of import: a trait for the layouts Uguisu can read, what
//! a scan observes about one file, and the deterministic matcher that
//! decides which episode it is. Copying, hashing, registering and the
//! sidecar all live in the engine — nothing here touches the database or
//! writes a byte.
//!
//! Four rules the engine is built to keep, stated here because they are
//! what the types are shaped around:
//!
//! * **Copy, never move.** The source archive belongs to the user and
//!   nothing modifies it. There is no `--move`.
//! * **An ambiguous file is never imported.** See
//!   [`matching`]: a wrong import is worse than an unresolved one.
//! * **Never overwrite.** A target that already holds different bytes is
//!   a reported collision.
//! * **Nothing is deleted.** Files that could not be matched, leftovers
//!   from an interrupted run and unreadable entries are all reported.

pub mod matching;
pub mod normalize;
pub mod podgrab;

use std::path::PathBuf;

use time::Date;
use uguisu_core::ids::EpisodeId;

use crate::path::RelativePath;
use crate::scan::ScannedFile;

pub use matching::{EpisodeFacts, MatchedBy, Reason, Score, Verdict, classify, score};

/// Everything a layout could work out about one source file, without
/// reading more than its first bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Path relative to the import root.
    pub relative: RelativePath,
    /// Where the file is on this machine.
    pub absolute: PathBuf,
    /// Length in bytes.
    pub size_bytes: u64,
    /// Modification time, when the platform reported one.
    pub mtime_unix: Option<i64>,
    /// Whether the first bytes look like media at all.
    pub looks_like_media: bool,
    /// Which podcast the layout thinks this belongs to, as free text.
    pub podcast_hint: Option<String>,
    /// The episode title the layout read out of the name.
    pub title: Option<String>,
    /// Other readings of the title, tried in order when [`Self::title`]
    /// does not match clearly: a name with a prefix that may or may not
    /// belong to it, or the file's own title tag (ADR 0050).
    pub fallback_titles: Vec<String>,
    /// A publication date the layout read out of the name.
    pub published: Option<Date>,
    /// A season number.
    pub season: Option<u32>,
    /// An episode number.
    pub number: Option<u32>,
    /// A duration, when a layout's side metadata carried one.
    pub duration_secs: Option<u32>,
    /// An episode the engine already identified for certain — the bytes
    /// are known, or an Uguisu sidecar named it. Set by the engine, never
    /// by a layout, and it short-circuits scoring entirely.
    pub exact_match: Option<(EpisodeId, MatchedBy)>,
}

impl Candidate {
    /// A candidate with nothing known about it. For tests and for layouts
    /// that fill in one field at a time.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            relative: RelativePath::from_trusted("unknown".to_owned()),
            absolute: PathBuf::new(),
            size_bytes: 0,
            mtime_unix: None,
            looks_like_media: true,
            podcast_hint: None,
            title: None,
            fallback_titles: Vec::new(),
            published: None,
            season: None,
            number: None,
            duration_secs: None,
            exact_match: None,
        }
    }

    /// The plain facts every layout starts from.
    #[must_use]
    pub fn from_file(file: &ScannedFile) -> Self {
        Self {
            looks_like_media: file.looks_like_media(),
            relative: file.relative.clone(),
            absolute: file.absolute.clone(),
            size_bytes: file.size_bytes,
            mtime_unix: file.mtime_unix,
            ..Self::empty()
        }
    }
}

/// A foreign archive layout Uguisu can read.
///
/// One implementation per tool, each in its own file, registered in
/// [`ImportFormat`].
pub trait SourceFormat: Send + Sync {
    /// Stable name, as the CLI and the API spell it.
    fn id(&self) -> &'static str;

    /// How well this layout explains a sample of the tree, 0 to 100.
    ///
    /// Cheap and structural: it looks at shapes of names and directories,
    /// never at file contents, so detection cannot be expensive.
    fn detect(&self, sample: &[ScannedFile]) -> u32;

    /// What this layout can read out of one file.
    fn observe(&self, file: &ScannedFile) -> Candidate;

    /// Sibling files this layout would like read alongside a media file.
    ///
    /// The layout names them; the caller reads them, bounded, and hands
    /// the text back to [`Self::refine`]. That split is what keeps this
    /// crate free of I/O while still letting a layout use the `.json` and
    /// `.nfo` files companion tooling writes.
    fn side_files(&self, file: &ScannedFile) -> Vec<RelativePath> {
        let _ = file;
        Vec::new()
    }

    /// Fills in what a side file was able to add.
    fn refine(&self, candidate: &mut Candidate, side: &str) {
        let _ = (candidate, side);
    }
}

/// The layouts this build can read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ImportFormat {
    /// Podgrab's directory-per-podcast layout.
    Podgrab,
    /// No particular tool: read whatever the names say.
    #[default]
    Generic,
}

impl ImportFormat {
    /// Every variant.
    pub const ALL: [Self; 2] = [Self::Podgrab, Self::Generic];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Podgrab => "podgrab",
            Self::Generic => "generic",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|f| f.as_str().eq_ignore_ascii_case(s))
    }

    /// The reader for this layout.
    #[must_use]
    pub fn reader(self) -> Box<dyn SourceFormat> {
        match self {
            Self::Podgrab => Box::new(podgrab::PodgrabFormat),
            Self::Generic => Box::new(GenericFormat),
        }
    }

    /// The layout that best explains a sample of the tree.
    ///
    /// Ties go to [`Self::Generic`], because guessing at a tool's
    /// conventions when the evidence is weak reads more into the names
    /// than is there.
    #[must_use]
    pub fn detect(sample: &[ScannedFile]) -> Self {
        let mut best = (Self::Generic, GenericFormat.detect(sample));
        for format in Self::ALL {
            let score = format.reader().detect(sample);
            if score > best.1 {
                best = (format, score);
            }
        }
        best.0
    }
}

impl std::fmt::Display for ImportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The layout of last resort: read the file name, believe nothing else.
///
/// Strips a leading date and a track prefix, reads a season marker, and
/// takes the parent directory as the podcast. That is what almost every
/// downloader produces, and it is the floor every other layout has to
/// beat.
#[derive(Debug, Clone, Copy)]
pub struct GenericFormat;

impl SourceFormat for GenericFormat {
    fn id(&self) -> &'static str {
        ImportFormat::Generic.as_str()
    }

    fn detect(&self, _sample: &[ScannedFile]) -> u32 {
        // It always applies, and always as the weakest reading.
        1
    }

    fn observe(&self, file: &ScannedFile) -> Candidate {
        let mut candidate = Candidate::from_file(file);
        candidate.podcast_hint = file.parent_name().map(str::to_owned);
        let stem = file.stem();
        let (published, rest) = normalize::strip_leading_date(stem);
        let (number, rest) = normalize::strip_track_prefix(rest);
        let (season, episode) = normalize::season_episode(stem);
        candidate.published = published;
        candidate.season = season;
        candidate.number = episode.or(number);
        let title = rest.trim();
        candidate.title = (!title.is_empty()).then(|| title.to_owned());
        candidate
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::Month;

    use super::*;

    fn file(path: &str) -> ScannedFile {
        ScannedFile {
            relative: RelativePath::parse(path).unwrap(),
            absolute: PathBuf::from(path),
            size_bytes: 1024,
            mtime_unix: Some(1_700_000_000),
            head: b"ID3\x04............".to_vec(),
        }
    }

    #[test]
    fn the_generic_layout_reads_the_name() {
        let c = GenericFormat.observe(&file("Darknet Diaries/2024-01-05 - 013 - Der Fall.mp3"));
        assert_eq!(c.podcast_hint.as_deref(), Some("Darknet Diaries"));
        assert_eq!(c.title.as_deref(), Some("Der Fall"));
        assert_eq!(
            c.published,
            Some(Date::from_calendar_date(2024, Month::January, 5).unwrap())
        );
        assert_eq!(c.number, Some(13));
        assert!(c.looks_like_media);
        assert_eq!(c.size_bytes, 1024);

        let c = GenericFormat.observe(&file("Show/S02E07 - Das Finale.mp3"));
        assert_eq!(c.season, Some(2));
        assert_eq!(c.number, Some(7));
        assert_eq!(c.title.as_deref(), Some("S02E07 - Das Finale"));
    }

    #[test]
    fn an_empty_name_yields_nothing() {
        let c = GenericFormat.observe(&file("Show/1234.mp3"));
        assert_eq!(
            c.title.as_deref(),
            Some("1234"),
            "a bare number is a title, not a track number"
        );
        assert_eq!(c.number, None);
        assert_eq!(c.published, None);
    }

    #[test]
    fn formats_round_trip_through_their_names() {
        for f in ImportFormat::ALL {
            assert_eq!(ImportFormat::parse(f.as_str()), Some(f));
            assert_eq!(f.reader().id(), f.as_str());
        }
        assert_eq!(ImportFormat::parse("PODGRAB"), Some(ImportFormat::Podgrab));
        assert_eq!(ImportFormat::parse("gpodder"), None);
        assert_eq!(ImportFormat::default(), ImportFormat::Generic);
    }

    #[test]
    fn detection_falls_back_rather_than_guessing() {
        // A flat directory of files says nothing about which tool wrote
        // it, so the reading that assumes least wins.
        let sample = vec![file("a.mp3"), file("b.mp3")];
        assert_eq!(ImportFormat::detect(&sample), ImportFormat::Generic);
        assert_eq!(ImportFormat::detect(&[]), ImportFormat::Generic);
    }
}
