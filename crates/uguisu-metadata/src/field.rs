//! The fields Uguisu manages, and nothing else (ADR 0012, ADR 0026).
//!
//! This list is the whole contract. A field on it is one Uguisu will write
//! and, in `sync` mode, keep in step with the feed. **Every other tag in a
//! file is left exactly as it is, in every mode** — which is not a policy
//! decision that could be forgotten but a mechanism: a write starts from a
//! clone of the file's existing tag and changes only these keys, so
//! anything not named here survives by construction.

use lofty::tag::ItemKey;

/// One piece of metadata Uguisu manages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    /// Episode title.
    Title,
    /// Podcast title, written as the album.
    Album,
    /// Episode author, or the podcast's when the episode names none.
    Artist,
    /// The podcast's author or publisher, written as the album artist.
    AlbumArtist,
    /// Publisher.
    Publisher,
    /// Episode description, plain text.
    Description,
    /// First category of the podcast.
    Genre,
    /// Publication date, as `YYYY-MM-DD`. Deliberately not a full
    /// timestamp: a trailing `Z` does not survive a write-then-read, and a
    /// value that does not round-trip is one `sync` rewrites for ever.
    RecordingDate,
    /// Episode number within its season.
    TrackNumber,
    /// Season, written where the format has somewhere to put it.
    DiscNumber,
    /// BCP 47 language tag.
    Language,
    /// Copyright notice.
    Copyright,
    /// The episode's feed GUID.
    EpisodeGuid,
}

// What is deliberately absent, and why. A field only belongs in this
// table if it survives a write-then-read unchanged: `sync` decides whether
// to write by comparing the value it wants against the value the file
// reports, so a field that comes back different is a field that is
// rewritten on every single run - and every rewrite moves the archive's
// hash. The guard against adding one back is a test, not this comment.
//
// * ID3v2's `PCST` podcast flag: `lofty` models it as a flag rather than
//   as text, and writing it makes the whole frame set fail to encode,
//   taking every other field with it.
// * The show name: written, but not readable back.
// * The year: ID3v2 stores it in the same frame as the recording date, so
//   the two overwrite each other. `RecordingDate` carries it.
// * The feed and enclosure URLs: `lofty` writes them into URL frames that
//   its own reader does not return as text.
//
// All four stay in the sidecar, which has no such limits (ADR 0026).

impl Field {
    /// Every field Uguisu manages.
    pub const ALL: [Self; 13] = [
        Self::Title,
        Self::Album,
        Self::Artist,
        Self::AlbumArtist,
        Self::Publisher,
        Self::Description,
        Self::Genre,
        Self::RecordingDate,
        Self::TrackNumber,
        Self::DiscNumber,
        Self::Language,
        Self::Copyright,
        Self::EpisodeGuid,
    ];

    /// Stable name, as the CLI, the API and the events spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Album => "album",
            Self::Artist => "artist",
            Self::AlbumArtist => "album_artist",
            Self::Publisher => "publisher",
            Self::Description => "description",
            Self::Genre => "genre",
            Self::RecordingDate => "recording_date",
            Self::TrackNumber => "track_number",
            Self::DiscNumber => "disc_number",
            Self::Language => "language",
            Self::Copyright => "copyright",
            Self::EpisodeGuid => "episode_guid",
        }
    }

    /// Parses the stable name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|f| f.as_str() == s)
    }

    /// The tag item this field is stored in.
    ///
    /// `lofty` maps one [`ItemKey`] onto each format's own spelling, so
    /// this is the only mapping Uguisu has to state: ID3v2's `TIT2`, MP4's
    /// `©nam` and Vorbis' `TITLE` are all `TrackTitle` here.
    #[must_use]
    pub const fn item_key(self) -> ItemKey {
        match self {
            Self::Title => ItemKey::TrackTitle,
            Self::Album => ItemKey::AlbumTitle,
            Self::Artist => ItemKey::TrackArtist,
            Self::AlbumArtist => ItemKey::AlbumArtist,
            Self::Publisher => ItemKey::Publisher,
            Self::Description => ItemKey::PodcastDescription,
            Self::Genre => ItemKey::Genre,
            Self::RecordingDate => ItemKey::RecordingDate,
            Self::TrackNumber => ItemKey::TrackNumber,
            Self::DiscNumber => ItemKey::DiscNumber,
            Self::Language => ItemKey::Language,
            Self::Copyright => ItemKey::CopyrightMessage,
            Self::EpisodeGuid => ItemKey::PodcastGlobalUniqueId,
        }
    }
}

impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_is_distinct() {
        let names: std::collections::BTreeSet<&str> =
            Field::ALL.iter().map(|f| f.as_str()).collect();
        assert_eq!(names.len(), Field::ALL.len(), "two fields share a name");
        // `ItemKey` is not ordered, so compare the debug forms: the point
        // is only that no two fields would write to the same tag item.
        let keys: std::collections::BTreeSet<String> = Field::ALL
            .iter()
            .map(|f| format!("{:?}", f.item_key()))
            .collect();
        assert_eq!(
            keys.len(),
            Field::ALL.len(),
            "two fields would write to the same tag item"
        );
        for f in Field::ALL {
            assert_eq!(Field::parse(f.as_str()), Some(f));
        }
        assert_eq!(Field::parse("lyrics"), None, "not a field Uguisu manages");
    }
}
