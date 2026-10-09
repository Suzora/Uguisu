//! Domain model: podcasts, sources, episodes, enclosures and extras.
//!
//! Field names follow `docs/DATA_MODEL.md`. These are plain data types;
//! persistence lives in
//! `uguisu-storage`, parsing in `uguisu-feed`, behaviour in `uguisu-engine`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

use crate::ids::{EnclosureId, EpisodeId, PodcastId, SourceId};

/// Syntax family of a feed document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    /// RSS 2.0 (`<rss>`).
    Rss2,
    /// Atom 1.0 (`<feed>`).
    Atom,
    /// RSS 1.0 / RDF (`<rdf:RDF>`).
    Rss1,
}

impl FeedKind {
    /// Stable string form used in the database and JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rss2 => "rss2",
            Self::Atom => "atom",
            Self::Rss1 => "rss1",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rss2" => Some(Self::Rss2),
            "atom" => Some(Self::Atom),
            "rss1" => Some(Self::Rss1),
            _ => None,
        }
    }
}

impl std::fmt::Display for FeedKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle status of a podcast (`docs/STATE_MACHINES.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PodcastStatus {
    /// Refreshed on schedule.
    Active,
    /// Not refreshed until resumed.
    Paused,
    /// Repeated fetch failures; still retried at a slower cadence.
    Error,
    /// No more refreshes; files kept.
    Archived,
}

impl PodcastStatus {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Error => "error",
            Self::Archived => "archived",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "error" => Some(Self::Error),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

/// A show as the user sees it. Never contains provider-specific fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Podcast {
    /// Identifier.
    pub id: PodcastId,
    /// Title as published.
    pub title: String,
    /// Normalized title for ordering (articles stripped, case folded).
    pub sort_title: String,
    /// `itunes:subtitle`.
    pub subtitle: Option<String>,
    /// `itunes:author`.
    pub author: Option<String>,
    /// Publisher (`managingEditor` / `dc:publisher`) when distinct from the author.
    pub publisher: Option<String>,
    /// `itunes:owner/itunes:name`.
    pub owner_name: Option<String>,
    /// `itunes:owner/itunes:email`.
    pub owner_email: Option<String>,
    /// Description with markup, as published.
    pub description_html: Option<String>,
    /// Description reduced to text.
    pub description_text: Option<String>,
    /// Show website.
    pub website: Option<Url>,
    /// Artwork URL as published.
    pub artwork_url: Option<Url>,
    /// BCP-47 language tag as published.
    pub language: Option<String>,
    /// Categories, flattened (`Technology`, `Technology / Tech News`).
    pub categories: Vec<String>,
    /// `itunes:explicit`.
    pub explicit: Option<bool>,
    /// `copyright`.
    pub copyright: Option<String>,
    /// `podcast:guid`.
    pub podcast_guid: Option<String>,
    /// Feed syntax family of the current source.
    pub feed_kind: FeedKind,
    /// Lifecycle status.
    pub status: PodcastStatus,
    /// Per-podcast refresh interval override.
    pub refresh_interval_secs: Option<u64>,
    /// Scheduler: next planned refresh.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_refresh_at: Option<OffsetDateTime>,
    /// Scheduler: last successful refresh.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_refresh_at: Option<OffsetDateTime>,
    /// Last error message shown to the user.
    pub last_error: Option<String>,
    /// Resolved top-level folder name, filled by the archive engine.
    pub directory_name: Option<String>,
    /// Hash of the comparable channel fields (change detection).
    pub metadata_hash: String,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last modification time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Fetch state of a podcast source (`docs/STATE_MACHINES.md` §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchState {
    /// Never fetched.
    NeverFetched,
    /// A refresh is in progress.
    Fetching,
    /// The last attempt fetched and processed a feed document.
    Fetched,
    /// The last attempt found the feed unchanged (304 or identical body).
    NotModified,
    /// The last attempt failed; the previous good state is retained.
    Failed,
    /// Refreshes are disabled for this source.
    Disabled,
}

impl FetchState {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NeverFetched => "never_fetched",
            Self::Fetching => "fetching",
            Self::Fetched => "fetched",
            Self::NotModified => "not_modified",
            Self::Failed => "failed",
            Self::Disabled => "disabled",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "never_fetched" => Some(Self::NeverFetched),
            "fetching" => Some(Self::Fetching),
            "fetched" => Some(Self::Fetched),
            "not_modified" => Some(Self::NotModified),
            "failed" => Some(Self::Failed),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// Fetch bookkeeping of a source: what happened last and what the next
/// conditional request must send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FetchStatus {
    /// Current state.
    pub state: FetchState,
    /// Last attempt, successful or not.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_attempt_at: Option<OffsetDateTime>,
    /// Last attempt that fetched and processed a feed.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_success_at: Option<OffsetDateTime>,
    /// Last attempt answered "not modified".
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_not_modified_at: Option<OffsetDateTime>,
    /// Last failed attempt.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_error_at: Option<OffsetDateTime>,
    /// Failures since the last success or not-modified answer.
    pub consecutive_failures: u32,
    /// HTTP status of the last attempt, when one was received.
    pub last_http_status: Option<u16>,
    /// Classification of the last error.
    pub last_error_kind: Option<crate::feed::FetchErrorKind>,
    /// Detail of the last error.
    pub last_error_detail: Option<String>,
    /// `ETag` of the last successful fetch.
    pub etag: Option<String>,
    /// `Last-Modified` of the last successful fetch.
    pub last_modified: Option<String>,
    /// SHA-256 (hex) of the last successfully processed body.
    pub content_fingerprint: Option<String>,
    /// Size in bytes of the last successfully processed body.
    pub last_content_length: Option<u64>,
}

impl Default for FetchStatus {
    fn default() -> Self {
        Self {
            state: FetchState::NeverFetched,
            last_attempt_at: None,
            last_success_at: None,
            last_not_modified_at: None,
            last_error_at: None,
            consecutive_failures: 0,
            last_http_status: None,
            last_error_kind: None,
            last_error_detail: None,
            etag: None,
            last_modified: None,
            content_fingerprint: None,
            last_content_length: None,
        }
    }
}

/// Why a source was replaced by another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ReplacementReason {
    /// A permanent HTTP redirect pointed elsewhere.
    Redirect,
    /// `itunes:new-feed-url` announced a new location.
    NewFeedUrl,
    /// The user changed the URL.
    Manual,
    /// Matched during an import.
    MatchedOnImport,
}

impl ReplacementReason {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Redirect => "redirect",
            Self::NewFeedUrl => "new-feed-url",
            Self::Manual => "manual",
            Self::MatchedOnImport => "matched-on-import",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "redirect" => Some(Self::Redirect),
            "new-feed-url" => Some(Self::NewFeedUrl),
            "manual" => Some(Self::Manual),
            "matched-on-import" => Some(Self::MatchedOnImport),
            _ => None,
        }
    }
}

/// Where a podcast's feed comes from, with history. Exactly one source per
/// podcast is current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PodcastSource {
    /// Identifier.
    pub id: SourceId,
    /// Owning podcast.
    pub podcast_id: PodcastId,
    /// Feed URL as configured or discovered.
    pub feed_url: Url,
    /// Canonical URL from `atom:link rel="self"` or redirects.
    pub canonical_url: Option<Url>,
    /// Website URL at discovery time.
    pub website_url: Option<Url>,
    /// Provider that produced the source (`apple`, `website`, `manual`, …).
    pub provider: String,
    /// Provider-specific reference.
    pub provider_ref: Option<String>,
    /// When the source was discovered.
    #[serde(with = "time::serde::rfc3339")]
    pub discovered_at: OffsetDateTime,
    /// When the feed was last verified as a podcast feed.
    #[serde(with = "time::serde::rfc3339::option")]
    pub verified_at: Option<OffsetDateTime>,
    /// Whether this is the current source.
    pub is_current: bool,
    /// Successor source, when replaced.
    pub replaced_by_source_id: Option<SourceId>,
    /// Why it was replaced.
    pub replacement_reason: Option<ReplacementReason>,
    /// Fetch bookkeeping.
    pub fetch: FetchStatus,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last modification time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Which signal produced an episode's identity key (ADR 0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySource {
    /// A GUID unique within the feed.
    Guid,
    /// The normalized primary enclosure URL, unique within the feed.
    EnclosureUrl,
    /// A fingerprint of title, publish day and enclosure length.
    Fingerprint,
}

impl IdentitySource {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Guid => "guid",
            Self::EnclosureUrl => "enclosure_url",
            Self::Fingerprint => "fingerprint",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "guid" => Some(Self::Guid),
            "enclosure_url" => Some(Self::EnclosureUrl),
            "fingerprint" => Some(Self::Fingerprint),
            _ => None,
        }
    }
}

/// The computed identity of a feed item and the signals behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EpisodeIdentity {
    /// Stable key, e.g. `guid:<normalized>`, `url:<normalized>`, `fp:<hex>`.
    pub key: String,
    /// Which cascade level produced the key.
    pub source: IdentitySource,
    /// Normalized GUID when the feed provided one (unique or not).
    pub guid_key: Option<String>,
    /// Normalized primary enclosure URL key when present.
    pub enclosure_key: Option<String>,
    /// Fingerprint key, always computed when a title exists.
    pub fingerprint_key: Option<String>,
    /// Human-readable explanation of the choice.
    pub reason: String,
}

/// Quality of a normalized date (`docs/FEED_ENGINE.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DateQuality {
    /// Parsed with an explicit offset.
    Exact,
    /// No offset in the source; UTC was assumed.
    AssumedUtc,
    /// Parsed, but more than a day in the future at parse time.
    Future,
    /// Parsed, but before 1990.
    Ancient,
    /// Could not be parsed; only the raw value is kept.
    Invalid,
}

impl DateQuality {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::AssumedUtc => "assumed_utc",
            Self::Future => "future",
            Self::Ancient => "ancient",
            Self::Invalid => "invalid",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "exact" => Some(Self::Exact),
            "assumed_utc" => Some(Self::AssumedUtc),
            "future" => Some(Self::Future),
            "ancient" => Some(Self::Ancient),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }
}

/// Archive state of an episode (denormalized; owned by the download engine on).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveState {
    /// Known from the feed, not yet handled by a policy.
    Expected,
    /// Excluded by policy or marked as a duplicate candidate.
    Skipped,
    /// Queued for download.
    Queued,
    /// Download in progress.
    Downloading,
    /// Stored and verified.
    Archived,
    /// File missing from the archive.
    Missing,
    /// File differs from the recorded hash.
    Modified,
    /// Download failed permanently.
    Failed,
}

impl ArchiveState {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Expected => "expected",
            Self::Skipped => "skipped",
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Archived => "archived",
            Self::Missing => "missing",
            Self::Modified => "modified",
            Self::Failed => "failed",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "expected" => Some(Self::Expected),
            "skipped" => Some(Self::Skipped),
            "queued" => Some(Self::Queued),
            "downloading" => Some(Self::Downloading),
            "archived" => Some(Self::Archived),
            "missing" => Some(Self::Missing),
            "modified" => Some(Self::Modified),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// How a person resolved a candidate duplicate (ADR 0051).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateResolution {
    /// One episode: the candidate merges into the episode it duplicates.
    Same,
    /// Two episodes: the candidate becomes one of its own.
    Separate,
}

impl DuplicateResolution {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Separate => "separate",
        }
    }
}

/// Media kind of an enclosure, from its declared MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnclosureKind {
    /// `audio/*`.
    Audio,
    /// `video/*`.
    Video,
    /// Anything else (documents, images, unknown).
    Other,
}

impl EnclosureKind {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Other => "other",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "audio" => Some(Self::Audio),
            "video" => Some(Self::Video),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    /// Classifies a MIME type.
    #[must_use]
    pub fn from_mime(mime: Option<&str>) -> Self {
        match mime.map(|m| m.trim().to_ascii_lowercase()) {
            Some(m) if m.starts_with("audio/") => Self::Audio,
            Some(m) if m.starts_with("video/") => Self::Video,
            _ => Self::Other,
        }
    }
}

/// A media file offered by an episode (primary enclosure or an alternate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Enclosure {
    /// Identifier (stable across refreshes for the same URL).
    pub id: EnclosureId,
    /// Owning episode.
    pub episode_id: EpisodeId,
    /// Media URL as published.
    pub url: Url,
    /// Declared MIME type, lower-cased.
    pub mime_type: Option<String>,
    /// Declared length in bytes.
    pub length_bytes: Option<u64>,
    /// Whether this is the item's primary enclosure.
    pub is_primary: bool,
    /// Media kind derived from the MIME type.
    pub kind: EnclosureKind,
    /// Position within the item (0 = first).
    pub position: u32,
    /// `podcast:alternateEnclosure@bitrate`.
    pub bitrate: Option<u64>,
    /// `podcast:alternateEnclosure@height`.
    pub height: Option<u32>,
    /// `podcast:alternateEnclosure@codecs`.
    pub codecs: Option<String>,
    /// `podcast:alternateEnclosure@lang`.
    pub lang: Option<String>,
    /// `podcast:alternateEnclosure@title`.
    pub title: Option<String>,
    /// `podcast:integrity@type`.
    pub integrity_type: Option<String>,
    /// `podcast:integrity@value`.
    pub integrity_value: Option<String>,
    /// Additional `podcast:source` URIs.
    pub sources: Vec<String>,
}

/// A chapters document reference (`podcast:chapters`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChaptersRef {
    /// URL.
    pub url: String,
    /// MIME type.
    pub mime_type: Option<String>,
}

/// A transcript reference (`podcast:transcript`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TranscriptRef {
    /// URL.
    pub url: String,
    /// MIME type.
    pub mime_type: Option<String>,
    /// Language.
    pub language: Option<String>,
    /// `rel` (e.g. `captions`).
    pub rel: Option<String>,
}

/// A person credit (`podcast:person`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Person {
    /// Name.
    pub name: String,
    /// Role.
    pub role: Option<String>,
    /// Group.
    pub group: Option<String>,
    /// Image URL.
    pub img: Option<String>,
    /// Link.
    pub href: Option<String>,
}

/// A location (`podcast:location`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Location {
    /// Name.
    pub name: String,
    /// `geo` URI.
    pub geo: Option<String>,
    /// OpenStreetMap reference.
    pub osm: Option<String>,
}

/// A soundbite (`podcast:soundbite`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Soundbite {
    /// Start time in seconds.
    pub start_time: f64,
    /// Duration in seconds.
    pub duration: f64,
    /// Title.
    pub title: Option<String>,
}

/// A funding link (`podcast:funding`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Funding {
    /// URL.
    pub url: String,
    /// Call to action.
    pub text: Option<String>,
}

/// A licence (`podcast:license`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct License {
    /// Identifier or text.
    pub text: String,
    /// URL of the licence.
    pub url: Option<String>,
}

/// A `podcast:txt` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Txt {
    /// Purpose.
    pub purpose: Option<String>,
    /// Value.
    pub value: String,
}

/// An element Uguisu does not model, kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RawExtension {
    /// Qualified name as written (`media:content`).
    pub name: String,
    /// Attributes.
    pub attributes: BTreeMap<String, String>,
    /// Text content.
    pub text: Option<String>,
}

/// Podcasting 2.0 data of an episode, stored as one JSON document.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EpisodeExtras {
    /// Chapter documents.
    #[serde(default)]
    pub chapters: Vec<ChaptersRef>,
    /// Transcripts.
    #[serde(default)]
    pub transcripts: Vec<TranscriptRef>,
    /// People.
    #[serde(default)]
    pub persons: Vec<Person>,
    /// Location.
    #[serde(default)]
    pub location: Option<Location>,
    /// Soundbites.
    #[serde(default)]
    pub soundbites: Vec<Soundbite>,
    /// `podcast:value` block, kept as JSON.
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// Funding links.
    #[serde(default)]
    pub funding: Vec<Funding>,
    /// Licence.
    #[serde(default)]
    pub license: Option<License>,
    /// `podcast:txt` records.
    #[serde(default)]
    pub txt: Vec<Txt>,
    /// Unmodelled elements.
    #[serde(default)]
    pub raw_extensions: Vec<RawExtension>,
}

impl EpisodeExtras {
    /// True when nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chapters.is_empty()
            && self.transcripts.is_empty()
            && self.persons.is_empty()
            && self.location.is_none()
            && self.soundbites.is_empty()
            && self.value.is_none()
            && self.funding.is_empty()
            && self.license.is_none()
            && self.txt.is_empty()
            && self.raw_extensions.is_empty()
    }
}

/// An episode as Uguisu understands the feed item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Episode {
    /// Identifier.
    pub id: EpisodeId,
    /// Owning podcast.
    pub podcast_id: PodcastId,
    /// GUID as published (may be empty or duplicated in bad feeds).
    pub guid: Option<String>,
    /// `guid@isPermaLink`.
    pub guid_is_permalink: Option<bool>,
    /// Stable identity (ADR 0006).
    pub identity: EpisodeIdentity,
    /// Title.
    pub title: String,
    /// `itunes:subtitle`.
    pub subtitle: Option<String>,
    /// Normalized title for ordering.
    pub sort_title: String,
    /// Description with markup.
    pub description_html: Option<String>,
    /// Description as text.
    pub description_text: Option<String>,
    /// Episode web page.
    pub link: Option<Url>,
    /// Normalized publication time (UTC).
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
    /// Publication time as written in the feed.
    pub published_at_raw: Option<String>,
    /// How trustworthy `published_at` is.
    pub published_at_quality: DateQuality,
    /// Source-declared update time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at_source: Option<OffsetDateTime>,
    /// Duration in seconds.
    pub duration_secs: Option<u32>,
    /// Duration as written.
    pub duration_raw: Option<String>,
    /// Season number.
    pub season: Option<u32>,
    /// Episode number.
    pub episode_number: Option<u32>,
    /// `full` / `trailer` / `bonus`.
    pub episode_type: Option<String>,
    /// Explicit flag.
    pub explicit: Option<bool>,
    /// Episode artwork.
    pub artwork_url: Option<Url>,
    /// Item author.
    pub author: Option<String>,
    /// Hash of the comparable fields (change detection).
    pub content_hash: String,
    /// Archive state, owned by the download and archive engines.
    pub archive_state: ArchiveState,
    /// Why the episode was skipped (policy rule or `duplicate of <id>`).
    pub skip_reason: Option<String>,
    /// The item could not be fully parsed.
    pub malformed: bool,
    /// Why it is malformed.
    pub malformed_reason: Option<String>,
    /// Candidate duplicate of this episode (ADR 0006 / 0014).
    pub duplicate_of_episode_id: Option<EpisodeId>,
    /// Reasons for the duplicate candidacy.
    pub duplicate_reasons: Vec<String>,
    /// Consecutive complete fetches that did not contain the item.
    pub missing_streak: u32,
    /// First time the item was seen.
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen_at: OffsetDateTime,
    /// Last time the item was seen in the feed.
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_in_feed_at: OffsetDateTime,
    /// When removal was detected (never deletes archive state).
    #[serde(with = "time::serde::rfc3339::option")]
    pub removed_from_feed_at: Option<OffsetDateTime>,
    /// Sort key: `published_at`, else `first_seen_at`.
    #[serde(with = "time::serde::rfc3339")]
    pub sort_at: OffsetDateTime,
    /// Raw parsed item as Uguisu understood it (source vs normalized).
    pub source_metadata: Option<serde_json::Value>,
    /// Media offered by the item.
    pub enclosures: Vec<Enclosure>,
    /// Podcasting 2.0 data.
    pub extras: EpisodeExtras,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last modification time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl Episode {
    /// The primary enclosure, if any.
    #[must_use]
    pub fn primary_enclosure(&self) -> Option<&Enclosure> {
        self.enclosures
            .iter()
            .find(|e| e.is_primary)
            .or_else(|| self.enclosures.first())
    }
}

/// A recorded change of one episode field (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EpisodeChange {
    /// Identifier.
    pub id: crate::ids::ChangeId,
    /// Episode.
    pub episode_id: EpisodeId,
    /// Podcast.
    pub podcast_id: PodcastId,
    /// The fetch that observed the change.
    pub fetch_id: Option<crate::ids::FetchId>,
    /// When it was observed.
    #[serde(with = "time::serde::rfc3339")]
    pub changed_at: OffsetDateTime,
    /// Field name (`title`, `enclosure_url`, `identity_source`, …).
    pub field: String,
    /// Previous value (truncated).
    pub old_value: Option<String>,
    /// New value (truncated).
    pub new_value: Option<String>,
}

/// Normalizes a title for sorting: case-folded, leading articles removed,
/// whitespace collapsed.
#[must_use]
pub fn sort_title(title: &str) -> String {
    let folded: String = title
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for article in [
        "the ", "a ", "an ", "der ", "die ", "das ", "le ", "la ", "les ", "el ", "los ",
    ] {
        if let Some(rest) = folded.strip_prefix(article)
            && !rest.is_empty()
        {
            return rest.to_owned();
        }
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_string_forms_round_trip() {
        for k in [FeedKind::Rss2, FeedKind::Atom, FeedKind::Rss1] {
            assert_eq!(FeedKind::parse(k.as_str()), Some(k));
        }
        for s in [
            FetchState::NeverFetched,
            FetchState::Fetching,
            FetchState::Fetched,
            FetchState::NotModified,
            FetchState::Failed,
            FetchState::Disabled,
        ] {
            assert_eq!(FetchState::parse(s.as_str()), Some(s));
        }
        for a in [
            ArchiveState::Expected,
            ArchiveState::Skipped,
            ArchiveState::Queued,
            ArchiveState::Downloading,
            ArchiveState::Archived,
            ArchiveState::Missing,
            ArchiveState::Modified,
            ArchiveState::Failed,
        ] {
            assert_eq!(ArchiveState::parse(a.as_str()), Some(a));
        }
        assert_eq!(
            ReplacementReason::parse("new-feed-url"),
            Some(ReplacementReason::NewFeedUrl)
        );
        assert_eq!(
            DateQuality::parse("assumed_utc"),
            Some(DateQuality::AssumedUtc)
        );
        assert_eq!(
            IdentitySource::parse("enclosure_url"),
            Some(IdentitySource::EnclosureUrl)
        );
        assert_eq!(FeedKind::parse("jsonfeed"), None);
    }

    #[test]
    fn enclosure_kind_from_mime() {
        assert_eq!(
            EnclosureKind::from_mime(Some("audio/mpeg")),
            EnclosureKind::Audio
        );
        assert_eq!(
            EnclosureKind::from_mime(Some(" Video/MP4 ")),
            EnclosureKind::Video
        );
        assert_eq!(
            EnclosureKind::from_mime(Some("application/pdf")),
            EnclosureKind::Other
        );
        assert_eq!(EnclosureKind::from_mime(None), EnclosureKind::Other);
    }

    #[test]
    fn sort_title_strips_articles_and_folds() {
        assert_eq!(sort_title("The Daily"), "daily");
        assert_eq!(sort_title("  Darknet   Diaries "), "darknet diaries");
        assert_eq!(sort_title("A"), "a");
        assert_eq!(sort_title("Die Nachrichten"), "nachrichten");
        assert_eq!(sort_title("Theatre Talk"), "theatre talk");
    }

    #[test]
    fn extras_default_is_empty() {
        assert!(EpisodeExtras::default().is_empty());
        let e = EpisodeExtras {
            funding: vec![Funding {
                url: "https://x".into(),
                text: None,
            }],
            ..EpisodeExtras::default()
        };
        assert!(!e.is_empty());
    }
}
