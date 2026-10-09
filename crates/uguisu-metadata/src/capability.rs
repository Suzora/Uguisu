//! Which containers Uguisu tags, and what each of them can hold
//! (ADR 0012, ADR 0026).
//!
//! Two different questions live here and are deliberately kept apart.
//!
//! *Can this container carry tags at all?* WAV and AIFF technically can —
//! `lofty` will write `RiffInfo` and `AiffText` — but those formats have
//! no place for a season, an episode GUID, a feed URL or cover art, and no
//! podcast client reads them. Uguisu therefore declares them unsupported
//! **by policy, not by capability**, and says so rather than writing four
//! of eighteen fields and calling it done.
//!
//! *Can this container carry this field?* Where a format has no equivalent
//! the field is [`Support::Unsupported`] and simply not written. It is
//! reported, not silently dropped: a caller records it in the sidecar, so
//! the value is still somewhere even when the file cannot hold it.

use lofty::file::FileType;
use lofty::tag::TagType;

use crate::field::Field;

/// Whether a format can hold a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// The format has a place for it.
    Native,
    /// It can be stored, but not in the way the format intends.
    Approximated(&'static str),
    /// The format has nowhere to put it.
    Unsupported,
}

impl Support {
    /// Whether the field can be written at all.
    #[must_use]
    pub const fn is_writable(self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// The containers Uguisu writes tags into, as `sniffed_type` spells them.
pub const SUPPORTED: [&str; 6] = ["mp3", "m4a", "mp4", "flac", "ogg", "opus"];

/// Whether Uguisu writes tags into this container.
///
/// The decision is made from the bytes, never from the extension
/// (`docs/SECURITY.md` §3.2): `lofty` identifies the file and this answers
/// for what it found.
#[must_use]
pub const fn writes(file_type: FileType) -> bool {
    matches!(
        file_type,
        FileType::Mpeg | FileType::Mp4 | FileType::Flac | FileType::Vorbis | FileType::Opus
    )
}

/// The tag format Uguisu writes into a container.
#[must_use]
pub const fn tag_type(file_type: FileType) -> Option<TagType> {
    match file_type {
        FileType::Mpeg => Some(TagType::Id3v2),
        FileType::Mp4 => Some(TagType::Mp4Ilst),
        FileType::Flac | FileType::Vorbis | FileType::Opus => Some(TagType::VorbisComments),
        _ => None,
    }
}

/// Whether one tag format can hold one field.
#[must_use]
pub const fn support(tag: TagType, field: Field) -> Support {
    match tag {
        // ID3v2 has a frame for everything Uguisu manages, including the
        // podcast frames Apple introduced.
        TagType::Id3v2 => Support::Native,
        TagType::Mp4Ilst => match field {
            // `lofty` maps no atom to the publisher and drops the value on
            // write, so `sync` would find it missing and rewrite it on
            // every run. Kept in the sidecar instead.
            Field::Publisher => Support::Unsupported,
            _ => Support::Native,
        },
        TagType::VorbisComments => match field {
            // The two podcast-specific fields. `lofty` writes them into
            // Vorbis comments its own reader does not return, so Uguisu
            // cannot tell afterwards whether they are there - and a field
            // it cannot read back is one `sync` would rewrite for ever,
            // moving the archive's hash on every run. Declared
            // unsupported, and kept in the sidecar instead, which has no
            // such limits.
            Field::Description | Field::EpisodeGuid => Support::Unsupported,
            Field::DiscNumber => Support::Approximated("the season is written as DISCNUMBER"),
            _ => Support::Native,
        },
        // Everything else: see the module documentation.
        _ => Support::Unsupported,
    }
}

/// Why a container is not tagged.
#[must_use]
pub fn why_unsupported(file_type: FileType) -> &'static str {
    match file_type {
        FileType::Wav | FileType::Aiff => {
            "the format's tag chunks hold none of the podcast fields and no player reads them"
        }
        _ => "Uguisu does not write tags into this container",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn only_supportable_containers_are_written() {
        for ft in [
            FileType::Mpeg,
            FileType::Mp4,
            FileType::Flac,
            FileType::Vorbis,
            FileType::Opus,
        ] {
            assert!(writes(ft), "{ft:?}");
            assert!(tag_type(ft).is_some(), "{ft:?}");
        }
        // Refused on purpose, not for want of a code path: `lofty` would
        // write to both of these.
        for ft in [FileType::Wav, FileType::Aiff] {
            assert!(!writes(ft), "{ft:?}");
            assert_eq!(tag_type(ft), None);
            assert!(why_unsupported(ft).contains("no player reads them"));
        }
    }

    #[test]
    fn every_field_answers_for_every_format() {
        for ft in [FileType::Mpeg, FileType::Mp4, FileType::Flac] {
            let tag = tag_type(ft).unwrap();
            for field in Field::ALL {
                // No field is ever silently absent from the table: an
                // `Unsupported` answer is a decision, not a gap.
                let _: Support = support(tag, field);
            }
        }
        assert_eq!(support(TagType::Id3v2, Field::EpisodeGuid), Support::Native);
        assert_eq!(
            support(TagType::VorbisComments, Field::EpisodeGuid),
            Support::Unsupported,
            "a field Uguisu cannot read back is one it must not write"
        );
        assert!(matches!(
            support(TagType::VorbisComments, Field::DiscNumber),
            Support::Approximated(_)
        ));
        assert_eq!(
            support(TagType::VorbisComments, Field::Title),
            Support::Native
        );
        assert_eq!(
            support(TagType::RiffInfo, Field::Title),
            Support::Unsupported
        );
        assert!(Support::Approximated("x").is_writable());
        assert!(!Support::Unsupported.is_writable());
    }
}
