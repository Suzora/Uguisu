//! Typed identifiers.
//!
//! Every persisted entity is keyed by a ULID (`docs/DATA_MODEL.md`):
//! time-sortable, 26 characters, generated in-process. `Id<T>` carries the
//! entity type as a phantom parameter so a podcast id cannot be passed
//! where an episode id is expected; it serializes as the plain string.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Process-wide generator so identifiers created within the same
/// millisecond still increase strictly (events and pages rely on id order).
static GENERATOR: Mutex<Option<ulid::Generator>> = Mutex::new(None);

fn next_ulid() -> ulid::Ulid {
    let mut guard = GENERATOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let generator = guard.get_or_insert_with(ulid::Generator::new);
    if let Ok(id) = generator.generate() {
        return id;
    }
    // The random part overflowed within one millisecond (astronomically
    // unlikely); restart the generator.
    let mut fresh = ulid::Generator::new();
    let id = fresh.generate().unwrap_or_else(|_| ulid::Ulid::generate());
    *generator = fresh;
    id
}

/// A ULID-backed identifier for entities of type `T`.
pub struct Id<T> {
    raw: ulid::Ulid,
    _marker: PhantomData<fn() -> T>,
}

/// Error returned when a string is not a valid identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid identifier `{0}`: expected a 26-character ULID")]
pub struct InvalidId(pub String);

impl<T> Id<T> {
    /// Generates a fresh, time-ordered identifier.
    #[must_use]
    pub fn new() -> Self {
        Self::from_ulid(next_ulid())
    }

    /// Wraps an existing ULID.
    #[must_use]
    pub const fn from_ulid(raw: ulid::Ulid) -> Self {
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// The identifier as its canonical 26-character string.
    #[must_use]
    pub fn as_string(&self) -> String {
        self.raw.to_string()
    }

    /// Parses the canonical string form.
    pub fn parse(s: &str) -> Result<Self, InvalidId> {
        let trimmed = s.trim();
        ulid::Ulid::from_string(trimmed)
            .map(Self::from_ulid)
            .map_err(|_| InvalidId(trimmed.to_owned()))
    }

    /// The underlying ULID.
    #[must_use]
    pub const fn ulid(&self) -> ulid::Ulid {
        self.raw
    }

    /// Re-types the identifier (for tables that share key spaces in tests).
    #[must_use]
    pub const fn cast<U>(self) -> Id<U> {
        Id::from_ulid(self.raw)
    }
}

impl<T> Default for Id<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Id<T> {}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<T> Eq for Id<T> {}

impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl<T> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.raw)
    }
}

impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.raw, f)
    }
}

impl<T> FromStr for Id<T> {
    type Err = InvalidId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<T> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.raw)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl<T> utoipa::PartialSchema for Id<T> {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .description(Some(
                "A ULID: 26 characters of Crockford base32, time-ordered.",
            ))
            .pattern(Some("^[0-9A-HJKMNP-TV-Z]{26}$"))
            .examples([serde_json::json!("01J8Z9WQ7K2M4N6P8R0T2V4X6Z")])
            .into()
    }
}

impl<T> utoipa::ToSchema for Id<T> {
    fn name() -> std::borrow::Cow<'static, str> {
        // One schema for every identifier space: the marker is a Rust-side
        // distinction, and on the wire they are all the same string.
        std::borrow::Cow::Borrowed("Ulid")
    }
}

/// Marker types for the identifier spaces.
pub mod markers {
    /// `podcasts.id`.
    #[derive(Debug)]
    pub enum Podcast {}
    /// `podcast_sources.id`.
    #[derive(Debug)]
    pub enum Source {}
    /// `episodes.id`.
    #[derive(Debug)]
    pub enum Episode {}
    /// `enclosures.id`.
    #[derive(Debug)]
    pub enum Enclosure {}
    /// `feed_fetches.id`.
    #[derive(Debug)]
    pub enum Fetch {}
    /// `events.id`.
    #[derive(Debug)]
    pub enum Event {}
    /// `episode_changes.id`.
    #[derive(Debug)]
    pub enum Change {}
    /// `download_jobs.id`.
    #[derive(Debug)]
    pub enum Job {}
    /// `download_attempts.id`.
    #[derive(Debug)]
    pub enum Attempt {}
    /// `archive_files.id`.
    #[derive(Debug)]
    pub enum ArchiveFile {}
    /// `podcast_artwork.id`.
    #[derive(Debug)]
    pub enum Artwork {}
    /// `discovery_records.id`.
    #[derive(Debug)]
    pub enum DiscoveryRecord {}
    /// `auth_sessions.id`.
    #[derive(Debug)]
    pub enum Session {}
    /// `auth_tokens.id`.
    #[derive(Debug)]
    pub enum ApiToken {}
}

/// Identifier of a podcast.
pub type PodcastId = Id<markers::Podcast>;
/// Identifier of a podcast source (feed URL with history).
pub type SourceId = Id<markers::Source>;
/// Identifier of an episode.
pub type EpisodeId = Id<markers::Episode>;
/// Identifier of an enclosure.
pub type EnclosureId = Id<markers::Enclosure>;
/// Identifier of a feed fetch record.
pub type FetchId = Id<markers::Fetch>;
/// Identifier of an event.
pub type EventId = Id<markers::Event>;
/// Identifier of an episode change record.
pub type ChangeId = Id<markers::Change>;
/// Identifier of a download job.
pub type JobId = Id<markers::Job>;
/// Identifier of a download attempt.
pub type AttemptId = Id<markers::Attempt>;
/// Identifier of an archive file record.
pub type ArchiveFileId = Id<markers::ArchiveFile>;
/// Identifier of a stored podcast artwork file.
pub type ArtworkId = Id<markers::Artwork>;
/// Identifier of a recorded feed resolution.
pub type DiscoveryRecordId = Id<markers::DiscoveryRecord>;
/// Identifier of a browser session.
pub type SessionId = Id<markers::Session>;
/// Identifier of an API token.
pub type ApiTokenId = Id<markers::ApiToken>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn ids_are_time_ordered_and_round_trip() {
        let a = PodcastId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = PodcastId::new();
        assert!(a < b, "later ids sort after earlier ones");
        let burst: Vec<EventId> = (0..1000).map(|_| EventId::new()).collect();
        assert!(
            burst.windows(2).all(|w| w[0] < w[1]),
            "ids generated in the same millisecond are strictly increasing"
        );
        let s = a.to_string();
        assert_eq!(s.len(), 26);
        assert_eq!(PodcastId::parse(&s).unwrap(), a);
        assert_eq!(PodcastId::parse(&format!("  {s} ")).unwrap(), a);
        assert!(PodcastId::parse("nope").is_err());
        assert!(PodcastId::parse("").is_err());
    }

    #[test]
    fn serializes_as_a_plain_string() {
        let id = EpisodeId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        let back: EpisodeId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(serde_json::from_str::<EpisodeId>("\"x\"").is_err());
    }

    #[test]
    fn typed_ids_do_not_mix() {
        let p = PodcastId::new();
        let e: EpisodeId = p.cast();
        assert_eq!(e.to_string(), p.to_string());
        assert_eq!(format!("{p:?}"), format!("Id({p})"));
    }
}
