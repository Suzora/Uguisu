//! Choosing a free path when the rendered one is taken (ADR 0022).
//!
//! Two episodes of the same podcast published on the same day with the
//! same title render to the same path. The rule that separates them must
//! be **stable**: a counter (` (2)`, ` (3)`, as ADR 0009 first proposed)
//! depends on the order in which episodes happen to be downloaded, so the
//! same episode would land somewhere else after a restart or a re-scan.
//! Uguisu therefore disambiguates with a short suffix derived from the
//! episode's own id, which is the same on every run and on every machine.
//!
//! Deciding is separated from looking: [`Occupancy`] answers "who owns
//! this path", so the rule can be tested exhaustively without a
//! filesystem, and the engine can answer from the database plus one
//! `symlink_metadata` call.

use uguisu_core::archive::PathProfile;
use uguisu_core::ids::EpisodeId;

use crate::path::{PathError, RelativePath};
use crate::sanitize;

/// How many characters of the episode id the suffix uses. 6 of Crockford's
/// base-32 alphabet are 30 bits: enough that a collision between two
/// episodes that already collide on title and date is not a practical
/// concern, and short enough to stay readable.
pub const SUFFIX_CHARS: usize = 6;

/// Who holds a candidate path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// Nothing is there, and no record claims it.
    Free,
    /// The episode that is being placed already owns this path.
    SameEpisode,
    /// Another episode's artifact owns it.
    OtherEpisode,
    /// Something exists on disk that Uguisu has no record of.
    ForeignFile,
}

impl Holder {
    /// Whether the episode being placed may write here.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Free | Self::SameEpisode)
    }
}

/// What a caller must be able to answer for a candidate path.
///
/// Implementations do not create anything: the question is asked while
/// choosing, before any directory exists.
pub trait Occupancy {
    /// Who holds `path`, from the point of view of `episode`.
    fn holder(&self, path: &RelativePath, episode: EpisodeId) -> Holder;
}

/// What the collision rule decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The path to use.
    pub path: RelativePath,
    /// The suffix that was added, if the preferred path was taken.
    pub suffix: Option<String>,
    /// Whether the episode already owned the chosen path.
    pub reused: bool,
}

/// The stable suffix for an episode: the last [`SUFFIX_CHARS`] characters
/// of its id, in brackets.
///
/// The tail is used rather than the head because a ULID's leading
/// characters encode the timestamp, so episodes added in the same
/// millisecond share them; the tail is the random part.
#[must_use]
pub fn suffix_for(episode: EpisodeId) -> String {
    let id = episode.to_string();
    let tail: String = id
        .chars()
        .rev()
        .take(SUFFIX_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("[{}]", tail.to_lowercase())
}

/// Appends `suffix` to the file name's stem, keeping the extension.
pub fn with_suffix(
    path: &RelativePath,
    suffix: &str,
    profile: PathProfile,
) -> Result<RelativePath, PathError> {
    let name = path.file_name();
    // Split at the last dot, but only when it leaves a stem: a name like
    // `.hidden` has no extension, it is all stem.
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, ext),
        _ => (name, ""),
    };
    let renamed = sanitize::file_name(stem, extension, Some(suffix), profile);
    match path.parent() {
        Some(parent) => RelativePath::parse(&format!("{parent}/{renamed}")),
        None => RelativePath::parse(&renamed),
    }
}

/// Picks the path `episode` should occupy, given the one the template
/// rendered.
///
/// 1. The preferred path, when it is free or already this episode's.
/// 2. Otherwise the preferred path with the episode's stable suffix.
/// 3. If even that is taken by someone else, the caller is told: a
///    suffixed path that another episode holds means two different
///    episodes share an id tail *and* a rendered name, which is a
///    condition to report, not to paper over with a third guess.
///
/// Nothing is created and nothing is moved here; the decision is pure.
pub fn place<O: Occupancy>(
    preferred: &RelativePath,
    episode: EpisodeId,
    profile: PathProfile,
    occupancy: &O,
) -> Result<Placement, Collision> {
    let holder = occupancy.holder(preferred, episode);
    if holder.is_usable() {
        return Ok(Placement {
            path: preferred.clone(),
            suffix: None,
            reused: holder == Holder::SameEpisode,
        });
    }

    let suffix = suffix_for(episode);
    let candidate = with_suffix(preferred, &suffix, profile).map_err(|source| Collision {
        path: preferred.as_str().to_owned(),
        holder,
        detail: source.to_string(),
    })?;
    let candidate_holder = occupancy.holder(&candidate, episode);
    if candidate_holder.is_usable() {
        return Ok(Placement {
            path: candidate,
            suffix: Some(suffix),
            reused: candidate_holder == Holder::SameEpisode,
        });
    }
    Err(Collision {
        path: candidate.as_str().to_owned(),
        holder: candidate_holder,
        detail: "the preferred path and its disambiguated form are both taken".to_owned(),
    })
}

/// No usable path could be chosen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path} is held by {holder:?}: {detail}")]
pub struct Collision {
    /// The path that was refused.
    pub path: String,
    /// Who holds it.
    pub holder: Holder,
    /// What went wrong.
    pub detail: String,
}

#[cfg(test)]
mod tests {
    // The extension under test is a literal lowercase `mp3`, so an exact
    // suffix assertion is the point.
    #![allow(clippy::case_sensitive_file_extension_comparisons)]
    #![allow(clippy::unwrap_used)]

    use std::collections::HashMap;

    use super::*;

    /// A table of who owns what, so the rule is tested without a disk.
    #[derive(Default)]
    struct Table(HashMap<String, Option<EpisodeId>>);

    impl Table {
        fn owned(mut self, path: &str, by: EpisodeId) -> Self {
            self.0.insert(path.to_owned(), Some(by));
            self
        }
        fn foreign(mut self, path: &str) -> Self {
            self.0.insert(path.to_owned(), None);
            self
        }
    }

    impl Occupancy for Table {
        fn holder(&self, path: &RelativePath, episode: EpisodeId) -> Holder {
            match self.0.get(path.as_str()) {
                None => Holder::Free,
                Some(None) => Holder::ForeignFile,
                Some(Some(owner)) if *owner == episode => Holder::SameEpisode,
                Some(Some(_)) => Holder::OtherEpisode,
            }
        }
    }

    fn path(s: &str) -> RelativePath {
        RelativePath::parse(s).unwrap()
    }

    #[test]
    fn a_free_path_is_used_as_rendered() {
        let episode = EpisodeId::new();
        let p = place(
            &path("Show/Ep.mp3"),
            episode,
            PathProfile::Portable,
            &Table::default(),
        )
        .unwrap();
        assert_eq!(p.path.as_str(), "Show/Ep.mp3");
        assert_eq!(p.suffix, None);
        assert!(!p.reused);
    }

    #[test]
    fn own_path_is_reused_not_suffixed() {
        let episode = EpisodeId::new();
        let table = Table::default().owned("Show/Ep.mp3", episode);
        let p = place(&path("Show/Ep.mp3"), episode, PathProfile::Portable, &table).unwrap();
        assert_eq!(p.path.as_str(), "Show/Ep.mp3");
        assert_eq!(p.suffix, None);
        assert!(p.reused, "re-registering must not move the file");
    }

    #[test]
    fn a_taken_path_gets_a_suffix() {
        let first = EpisodeId::new();
        let second = EpisodeId::new();
        let table = Table::default().owned("Show/Ep.mp3", first);
        let p = place(&path("Show/Ep.mp3"), second, PathProfile::Portable, &table).unwrap();
        let expected = format!("Show/Ep {}.mp3", suffix_for(second));
        assert_eq!(p.path.as_str(), expected);
        assert_eq!(p.suffix.as_deref(), Some(suffix_for(second).as_str()));
        assert!(p.path.as_str().ends_with(".mp3"), "{}", p.path);
    }

    #[test]
    fn a_foreign_file_is_never_overwritten() {
        let episode = EpisodeId::new();
        let table = Table::default().foreign("Show/Ep.mp3");
        let p = place(&path("Show/Ep.mp3"), episode, PathProfile::Portable, &table).unwrap();
        assert_ne!(p.path.as_str(), "Show/Ep.mp3");
        assert!(p.suffix.is_some());
    }

    #[test]
    fn the_choice_is_deterministic() {
        let first = EpisodeId::new();
        let second = EpisodeId::new();
        let table = Table::default().owned("Show/Ep.mp3", first);
        let a = place(&path("Show/Ep.mp3"), second, PathProfile::Portable, &table).unwrap();
        let b = place(&path("Show/Ep.mp3"), second, PathProfile::Portable, &table).unwrap();
        assert_eq!(a, b, "a counter would have produced a different path here");
    }

    #[test]
    fn a_doubly_taken_path_is_reported() {
        let other = EpisodeId::new();
        let episode = EpisodeId::new();
        let suffixed = format!("Show/Ep {}.mp3", suffix_for(episode));
        let table = Table::default()
            .owned("Show/Ep.mp3", other)
            .owned(&suffixed, other);
        let err = place(&path("Show/Ep.mp3"), episode, PathProfile::Portable, &table).unwrap_err();
        assert_eq!(err.holder, Holder::OtherEpisode);
        assert_eq!(err.path, suffixed);
    }

    #[test]
    fn a_suffix_keeps_extension_and_directory() {
        let episode = EpisodeId::new();
        let s = suffix_for(episode);
        assert_eq!(
            with_suffix(&path("Show/2024/Ep 1.mp3"), &s, PathProfile::Portable)
                .unwrap()
                .as_str(),
            format!("Show/2024/Ep 1 {s}.mp3")
        );
        // A name without an extension keeps its whole self as the stem.
        assert_eq!(
            with_suffix(&path("Show/Ep"), &s, PathProfile::Portable)
                .unwrap()
                .as_str(),
            format!("Show/Ep {s}")
        );
        // A leading dot is part of the name, not an extension.
        assert_eq!(
            with_suffix(&path(".hidden"), &s, PathProfile::Portable)
                .unwrap()
                .as_str(),
            format!(".hidden {s}")
        );
    }

    #[test]
    fn the_suffix_derives_from_the_id() {
        let episode = EpisodeId::new();
        let s = suffix_for(episode);
        assert_eq!(s, suffix_for(episode));
        assert_eq!(s.chars().count(), SUFFIX_CHARS + 2);
        assert!(s.starts_with('[') && s.ends_with(']'));
        let tail = episode.to_string().to_lowercase();
        assert!(tail.ends_with(&s[1..s.len() - 1]), "{s} vs {tail}");
        assert_ne!(s, suffix_for(EpisodeId::new()));
    }
}
