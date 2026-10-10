//! Archive vocabulary: the persisted artifact record, what a verification
//! pass learned about it, the path-template configuration and the
//! automatic archive policy (`docs/ARCHIVE_ENGINE.md`, ADRs 0021–0023).
//!
//! The enums keep the stable string forms that are stored in the database
//! and shown on the wire (`as_str`/`parse`), like the other model enums.
//! Nothing here performs I/O: the algorithms live in `uguisu-archive`, the
//! persistence in `uguisu-storage` and the wiring in `uguisu-engine`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

use crate::download::Priority;
use crate::ids::{ArchiveFileId, ArtworkId, EpisodeId, PodcastId};
use crate::model::{ChaptersRef, TranscriptRef};

/// What the last verification learned about the file on disk.
///
/// This describes the **artifact**, never the transfer: a job's
/// [`DownloadState`](crate::download::DownloadState) says how the bytes
/// arrived, this says whether they are still there and still correct.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    /// Registered but not checked since (a fresh download is trusted:
    /// its hash was computed while streaming).
    #[default]
    Unchecked,
    /// The file is there and matches what was recorded.
    Verified,
    /// Nothing is at the recorded path any more.
    Missing,
    /// Something is there, but it is not what was recorded.
    Invalid,
}

impl VerificationState {
    /// Every variant.
    pub const ALL: [Self; 4] = [
        Self::Unchecked,
        Self::Verified,
        Self::Missing,
        Self::Invalid,
    ];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Verified => "verified",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Whether the artifact needs a user's attention.
    #[must_use]
    pub const fn is_problem(self) -> bool {
        matches!(self, Self::Missing | Self::Invalid)
    }
}

impl std::fmt::Display for VerificationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why an artifact is in its verification state: a closed vocabulary, so
/// the reason can be matched on and translated rather than parsed.
pub mod reason {
    /// Recorded when the download finished; not checked since.
    pub const REGISTERED: &str = "registered";
    /// Existence pass: something is at the path. Whether it is the right
    /// something was not asked, so this never upgrades a record's state.
    pub const PRESENT: &str = "present";
    /// Light pass: size and, when one was recorded, mtime still match.
    pub const SIZE_MATCH: &str = "size_match";
    /// Light pass: the size matches but the file was modified since its
    /// mtime was recorded, so only a full pass can say whether it is intact.
    pub const MTIME_CHANGED: &str = "mtime_changed";
    /// Full pass: the hash still matches.
    pub const HASH_MATCH: &str = "hash_match";
    /// Nothing exists at the path.
    pub const NOT_FOUND: &str = "not_found";
    /// A directory or another non-file sits at the path.
    pub const NOT_A_FILE: &str = "not_a_file";
    /// The file has a different length than recorded.
    pub const SIZE_MISMATCH: &str = "size_mismatch";
    /// The file has the recorded length but different bytes.
    pub const HASH_MISMATCH: &str = "hash_mismatch";
    /// The file is empty (never a valid artifact).
    pub const EMPTY: &str = "empty";
    /// The file could not be read.
    pub const PERMISSION_DENIED: &str = "permission_denied";
    /// Any other I/O failure while checking.
    pub const IO_ERROR: &str = "io_error";
    /// The path resolves outside the archive root (symlink or template).
    pub const OUTSIDE_ROOT: &str = "outside_root";
    /// A verified artifact was moved by a relocation; a rename keeps the bytes.
    pub const RELOCATED: &str = "relocated";
    /// The record was reconstructed from a sidecar. It says what the
    /// sidecar claimed, which is not the same as having checked the file:
    /// a rebuilt record is always `unchecked`.
    pub const REBUILT: &str = "rebuilt";
    /// The record was created by copying a file in from a foreign archive.
    pub const IMPORTED: &str = "imported";
    /// The bytes changed because Uguisu wrote metadata tags, and the new
    /// hash was recorded in the same step.
    pub const TAGGED: &str = "tagged";
    /// A tag write was interrupted by a crash after the replacement was in
    /// place; recovery adopted the bytes the write had already produced.
    /// This is the one place a hash is re-read to fit the file, and it is
    /// only reachable for a row that said `pending` before the first byte
    /// moved.
    pub const RETAG_RECOVERED: &str = "retag_recovered";
}

/// How hard a verification pass looks.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VerifyDepth {
    /// Only that something is at the path (a server's check, `archive reconcile`).
    Existence,
    /// Existence, type and size, plus mtime when it was recorded.
    #[default]
    Light,
    /// Everything above and the SHA-256 of the whole file.
    Full,
}

impl VerifyDepth {
    /// Every variant.
    pub const ALL: [Self; 3] = [Self::Existence, Self::Light, Self::Full];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Existence => "existence",
            Self::Light => "light",
            Self::Full => "full",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Whether this depth reads the whole file.
    #[must_use]
    pub const fn hashes(self) -> bool {
        matches!(self, Self::Full)
    }
}

impl std::fmt::Display for VerifyDepth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A file Uguisu owns in the archive: one active record per episode.
///
/// Record identity: `episode_id`, `podcast_id`, `registered_at`.
/// **The bytes on disk now**: `size_bytes`, `hash_algo`, `hash_value`,
/// `mtime_unix` — these follow the file, so a tag write moves them.
/// **The bytes as received**: `source_size_bytes`, `source_hash_algo`,
/// `source_hash_value` — provenance, written once per download and never
/// again, which is what tells a file Uguisu retagged apart from one that
/// was altered behind its back. (A *re*-download of the same episode
/// replaces them, because they then describe different bytes; they are
/// immutable for the life of one download, not of the row.)
/// Derived facts: `relative_path` (a relocation moves it), `content_type`,
/// `sniffed_type`. Verification metadata: `verification_state`,
/// `verification_reason`, `verified_at`. Metadata state: `origin`,
/// `tag_state`, `tag_mode`, `tagged_at`, `sidecar_written_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ArchiveFile {
    /// Identifier.
    pub id: ArchiveFileId,
    /// The episode this artifact belongs to (unique: one active file).
    pub episode_id: EpisodeId,
    /// Its podcast.
    pub podcast_id: PodcastId,
    /// Path relative to the media root, POSIX separators.
    pub relative_path: String,
    /// Length in bytes as recorded.
    pub size_bytes: u64,
    /// `Content-Type` as served during the download.
    pub content_type: Option<String>,
    /// Container guessed from the first bytes.
    pub sniffed_type: Option<String>,
    /// Hash algorithm (`sha256`).
    pub hash_algo: String,
    /// Hash of the complete file **as it is now**.
    pub hash_value: String,
    /// Length as received; `None` only for rows written before the length was
    /// recorded, which the
    /// migration could not fill.
    pub source_size_bytes: Option<u64>,
    /// Hash algorithm of [`Self::source_hash_value`].
    pub source_hash_algo: Option<String>,
    /// Hash of the bytes as received. Equal to `hash_value` until Uguisu
    /// writes tags; a verification pass never touches it.
    pub source_hash_value: Option<String>,
    /// Modification time when the record was written (seconds since the
    /// epoch); `None` when the platform did not report one.
    pub mtime_unix: Option<i64>,
    /// How the record came to exist.
    pub origin: ArchiveOrigin,
    /// How far a tag write got.
    pub tag_state: TagState,
    /// The mode of the last successful tag write.
    pub tag_mode: Option<TagMode>,
    /// When tags were last written.
    #[serde(with = "time::serde::rfc3339::option")]
    pub tagged_at: Option<OffsetDateTime>,
    /// When the portable sidecar beside the file was last written.
    #[serde(with = "time::serde::rfc3339::option")]
    pub sidecar_written_at: Option<OffsetDateTime>,
    /// The managed tags the file carried before Uguisu first wrote any;
    /// `None` until a tag write captures them (ADR 0012).
    #[serde(default)]
    pub original_tags: Option<OriginalTags>,
    /// When a refresh last found the feed pointing at different audio for
    /// this download (another URL or declared length). The file is kept as
    /// it is; a re-download clears this (ADR 0015).
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub source_changed_at: Option<OffsetDateTime>,
    /// What the last verification found.
    pub verification_state: VerificationState,
    /// Why, from [`reason`].
    pub verification_reason: Option<String>,
    /// When the last verification ran.
    #[serde(with = "time::serde::rfc3339::option")]
    pub verified_at: Option<OffsetDateTime>,
    /// When the artifact was first recorded.
    #[serde(with = "time::serde::rfc3339")]
    pub registered_at: OffsetDateTime,
    /// Row creation.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Classification of an archive failure. Local archive problems get their
/// own kinds: an HTTP or download kind would say nothing about a file that
/// is missing from disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveErrorKind {
    /// No archive record exists for the episode.
    ArchiveNotFound,
    /// The recorded file is gone.
    ArchiveMissing,
    /// The recorded file is there but does not match.
    ArchiveInvalid,
    /// The file's hash differs from the record.
    HashMismatch,
    /// The file's length differs from the record.
    SizeMismatch,
    /// A path would leave the archive root, or is not usable on this system.
    PathInvalid,
    /// The target path is taken by another episode or by an unrelated file.
    PathCollision,
    /// The configured template could not be parsed or rendered.
    TemplateInvalid,
    /// A relocation could not complete (cross-device, rename refused).
    RelocationFailed,
    /// The file could not be read while verifying.
    VerificationIo,
    /// The requested policy contradicts the stored one.
    PolicyConflict,
    /// The policy values are not usable.
    PolicyInvalid,
    /// A sidecar exists but does not parse, is too large, or claims a
    /// schema this build cannot read.
    SidecarInvalid,
    /// No sidecar was found where one was expected.
    SidecarMissing,
    /// A manifest does not parse, or repeats a path.
    ManifestInvalid,
    /// A source file matched more than one episode closely enough that
    /// picking one would be a guess.
    ImportAmbiguous,
    /// A source file matched no episode well enough to import.
    ImportUnmatched,
    /// The import source is unusable: outside its declared root, a
    /// symlink leaving it, or not readable.
    ImportSourceInvalid,
    /// Artwork bytes are not an image Uguisu stores, or contradict the
    /// declared media type.
    ArtworkInvalid,
    /// The container carries no tag format Uguisu writes.
    TagsUnsupported,
    /// A tag write was attempted and refused; the file is untouched.
    TagsFailed,
}

impl ArchiveErrorKind {
    /// Every variant.
    pub const ALL: [Self; 21] = [
        Self::ArchiveNotFound,
        Self::ArchiveMissing,
        Self::ArchiveInvalid,
        Self::HashMismatch,
        Self::SizeMismatch,
        Self::PathInvalid,
        Self::PathCollision,
        Self::TemplateInvalid,
        Self::RelocationFailed,
        Self::VerificationIo,
        Self::PolicyConflict,
        Self::PolicyInvalid,
        Self::SidecarInvalid,
        Self::SidecarMissing,
        Self::ManifestInvalid,
        Self::ImportAmbiguous,
        Self::ImportUnmatched,
        Self::ImportSourceInvalid,
        Self::ArtworkInvalid,
        Self::TagsUnsupported,
        Self::TagsFailed,
    ];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArchiveNotFound => "archive_not_found",
            Self::ArchiveMissing => "archive_missing",
            Self::ArchiveInvalid => "archive_invalid",
            Self::HashMismatch => "hash_mismatch",
            Self::SizeMismatch => "size_mismatch",
            Self::PathInvalid => "path_invalid",
            Self::PathCollision => "path_collision",
            Self::TemplateInvalid => "template_invalid",
            Self::RelocationFailed => "relocation_failed",
            Self::VerificationIo => "verification_io",
            Self::PolicyConflict => "policy_conflict",
            Self::PolicyInvalid => "policy_invalid",
            Self::SidecarInvalid => "sidecar_invalid",
            Self::SidecarMissing => "sidecar_missing",
            Self::ManifestInvalid => "manifest_invalid",
            Self::ImportAmbiguous => "import_ambiguous",
            Self::ImportUnmatched => "import_unmatched",
            Self::ImportSourceInvalid => "import_source_invalid",
            Self::ArtworkInvalid => "artwork_invalid",
            Self::TagsUnsupported => "tags_unsupported",
            Self::TagsFailed => "tags_failed",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for ArchiveErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which sanitization rules a rendered path obeys (ADR 0009).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PathProfile {
    /// Windows rules: reserved device names, `<>:"|?*`, no trailing dot or space.
    Windows,
    /// POSIX rules: only `/` and NUL are impossible.
    Posix,
    /// The intersection of both, so an archive can move between systems.
    #[default]
    Portable,
}

impl PathProfile {
    /// Every variant.
    pub const ALL: [Self; 3] = [Self::Windows, Self::Posix, Self::Portable];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Posix => "posix",
            Self::Portable => "portable",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for PathProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a podcast's episodes are queued automatically.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    /// Only explicit commands queue episodes (the default, ADR 0023).
    #[default]
    Manual,
    /// Newly discovered episodes are queued by the policy.
    Auto,
}

impl PolicyMode {
    /// Every variant.
    pub const ALL: [Self; 2] = [Self::Manual, Self::Auto];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Whether the policy may queue on its own.
    #[must_use]
    pub const fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl std::fmt::Display for PolicyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The automatic download rules for one podcast, as stored.
///
/// A row exists only where a user overrode the global defaults; the engine
/// merges it with [`ArchiveConfig`](crate::config::ArchiveConfig) into the
/// effective policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ArchivePolicy {
    /// The podcast this applies to.
    pub podcast_id: PodcastId,
    /// Manual or automatic.
    pub mode: PolicyMode,
    /// At most this many of the podcast's episodes may be waiting to be
    /// archived at once (queued, retrying, downloading or finalizing);
    /// `None` uses the global default, `0` means no limit.
    pub max_backlog: Option<u32>,
    /// Episodes published longer ago than this are left alone; `None`
    /// uses the global default.
    pub max_age_days: Option<u32>,
    /// Priority for automatically queued jobs.
    pub priority: Option<Priority>,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// What the policy decided for one episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    /// Queue it at this priority.
    Queue {
        /// Priority for the job.
        priority: Priority,
    },
    /// Leave it alone, for this reason.
    Skip {
        /// One of [`policy_reason`].
        reason: String,
    },
}

impl PolicyDecision {
    /// Whether the decision queues the episode.
    #[must_use]
    pub const fn queues(&self) -> bool {
        matches!(self, Self::Queue { .. })
    }

    /// The reason a skip carries, if any.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Queue { .. } => None,
            Self::Skip { reason } => Some(reason.as_str()),
        }
    }
}

/// Why the policy left an episode alone: a closed vocabulary, like the
/// verification reasons.
pub mod policy_reason {
    /// The podcast (or the installation) is in manual mode.
    pub const DISABLED: &str = "disabled";
    /// The episode already has a file.
    pub const ALREADY_ARCHIVED: &str = "already_archived";
    /// A download job already exists.
    pub const ALREADY_QUEUED: &str = "already_queued";
    /// The episode has no media to download.
    pub const NO_ENCLOSURE: &str = "no_enclosure";
    /// The episode is a candidate duplicate of another one.
    pub const DUPLICATE: &str = "duplicate";
    /// The episode was marked skipped.
    pub const SKIPPED: &str = "skipped";
    /// Older than the configured age limit.
    pub const TOO_OLD: &str = "too_old";
    /// Beyond the configured backlog limit.
    pub const BACKLOG_EXCEEDED: &str = "backlog_exceeded";
    /// The episode is gone from the feed.
    pub const REMOVED_FROM_FEED: &str = "removed_from_feed";
}

/// How an archive record came to exist.
///
/// This is provenance, not evidence: it says where the bytes came from,
/// never whether they are still correct. Only a verification pass decides
/// that.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveOrigin {
    /// Uguisu downloaded the file itself.
    #[default]
    Download,
    /// The file was copied in from a foreign archive.
    Import,
    /// The record was reconstructed from a sidecar after the index was lost.
    Rebuild,
}

impl ArchiveOrigin {
    /// Every variant.
    pub const ALL: [Self; 3] = [Self::Download, Self::Import, Self::Rebuild];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Import => "import",
            Self::Rebuild => "rebuild",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for ArchiveOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which managed fields a tag write may change.
///
/// Two modes, and both preserve every tag Uguisu does not manage. ADR 0012
/// sketched five; the other three (`overwrite`, `existing_wins`, `custom`)
/// stay unbuilt rather than half-built, which ADR 0026 records.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TagMode {
    /// Write a managed field only where the file has none. A value the
    /// publisher (or the user) already wrote is never replaced.
    #[default]
    FillMissing,
    /// Make the managed fields match Uguisu. A field Uguisu has no value
    /// for is still left alone: `sync` never empties anything.
    Sync,
}

impl TagMode {
    /// Every variant.
    pub const ALL: [Self; 2] = [Self::FillMissing, Self::Sync];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FillMissing => "fill_missing",
            Self::Sync => "sync",
        }
    }

    /// Parses the stable string form. `fill-missing` is accepted because
    /// that is how the CLI flag reads.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fill-missing" => Some(Self::FillMissing),
            other => Self::ALL.iter().copied().find(|k| k.as_str() == other),
        }
    }
}

impl std::fmt::Display for TagMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How far a tag write got.
///
/// `Pending` is written **before** the media file is touched, which is the
/// whole point of the column: after a crash between the atomic replace and
/// the record update, "Uguisu retagged this" and "someone tampered with
/// this" are indistinguishable from the bytes alone. A marker a crash
/// cannot forge tells them apart, and recovery is bounded to the rows that
/// carry it. Without it, an interrupted retag would look exactly like
/// corruption for ever.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TagState {
    /// Uguisu has never written tags to this file.
    #[default]
    Untagged,
    /// A write is in flight, or was interrupted by a crash.
    Pending,
    /// Tags were written and the record matches the bytes on disk.
    Written,
    /// The container carries no tag format Uguisu writes. Not an error.
    Unsupported,
    /// The write was attempted and refused; the media file is untouched.
    Failed,
}

impl TagState {
    /// Every variant.
    pub const ALL: [Self; 5] = [
        Self::Untagged,
        Self::Pending,
        Self::Written,
        Self::Unsupported,
        Self::Failed,
    ];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Untagged => "untagged",
            Self::Pending => "pending",
            Self::Written => "written",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Whether a recovery pass has to look at this row.
    #[must_use]
    pub const fn is_in_flight(self) -> bool {
        matches!(self, Self::Pending)
    }
}

impl std::fmt::Display for TagState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An image container Uguisu stores as podcast artwork.
///
/// Deliberately short: these three are what podcast feeds serve and what
/// every tag format can carry. A format that is not on this list is
/// refused, never guessed at — the point of the list is that the bytes
/// were recognised, not that the server was believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkFormat {
    /// JPEG.
    Jpeg,
    /// PNG.
    Png,
    /// WebP.
    Webp,
}

impl ArtworkFormat {
    /// Every variant.
    pub const ALL: [Self; 3] = [Self::Jpeg, Self::Png, Self::Webp];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// The media type Uguisu records and embeds.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }

    /// The file extension, without the dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }
}

impl std::fmt::Display for ArtworkFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One line of a `sha256sum -c` manifest.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
pub struct ManifestEntry {
    /// Path relative to the **media root**, POSIX separators — byte-identical
    /// to `ArchiveFile::relative_path`, so nothing has to be rewritten when
    /// the template changes.
    pub relative_path: String,
    /// Lowercase hex digest.
    pub hash_value: String,
}

/// The state of one podcast's manifest file.
///
/// The manifest is derived data: `stale` is set inside the very
/// transaction that changes an artifact, so a crash can only ever leave
/// "marked stale but actually fresh" — never the reverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ArchiveManifest {
    /// The podcast whose artifacts it lists.
    pub podcast_id: PodcastId,
    /// Where the file lives, relative to the media root.
    pub relative_path: String,
    /// How many artifacts the last write listed.
    pub entries: u64,
    /// SHA-256 of the manifest text itself, once written.
    pub hash_value: Option<String>,
    /// Whether the index has changed since the last write.
    pub stale: bool,
    /// When the manifest was last written.
    #[serde(with = "time::serde::rfc3339::option")]
    pub generated_at: Option<OffsetDateTime>,
    /// Since when it has been stale.
    #[serde(with = "time::serde::rfc3339::option")]
    pub stale_since: Option<OffsetDateTime>,
    /// Last change to this row.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A stored artwork file for a podcast.
///
/// Artwork is content-addressed, so fetching a replacement can never
/// destroy the previous one; exactly one row per podcast carries
/// `is_current`. It lives in its own table because `ArchiveFile` is for
/// episode media and its `episode_id` is unique.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PodcastArtwork {
    /// Identifier.
    pub id: ArtworkId,
    /// The podcast it belongs to.
    pub podcast_id: PodcastId,
    /// Where it was fetched from.
    pub source_url: Option<Url>,
    /// Path relative to the media root, POSIX separators.
    pub relative_path: String,
    /// The container, as recognised from the bytes.
    pub format: ArtworkFormat,
    /// `Content-Type` as served (kept for the record; never trusted alone).
    pub content_type: Option<String>,
    /// Length in bytes.
    pub size_bytes: u64,
    /// Hash algorithm (`sha256`).
    pub hash_algo: String,
    /// Hash of the file; also its name on disk.
    pub hash_value: String,
    /// `ETag` of the response, for the next conditional request.
    pub etag: Option<String>,
    /// `Last-Modified` of the response.
    pub last_modified: Option<String>,
    /// Whether this is the artwork Uguisu currently uses.
    pub is_current: bool,
    /// When it was fetched.
    #[serde(with = "time::serde::rfc3339")]
    pub retrieved_at: OffsetDateTime,
    /// Row creation.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// The portable record written next to every archived media file
/// (`<media file>.json`, ADR 0007 and ADR 0024).
///
/// This is what makes the archive self-describing: copy one episode out of
/// the archive and its sidecar travels with it; lose the database entirely
/// and `archive reconcile --rebuild` reads the sidecars back.
///
/// **A sidecar is metadata, never evidence.** It records what Uguisu knew
/// when it was written. A rebuild therefore restores a record as
/// [`VerificationState::Unchecked`] with reason [`reason::REBUILT`]; only a
/// real verification pass may ever write `verified`. A file that has been
/// edited since still has a perfectly well-formed sidecar.
///
/// Unknown fields are **ignored** rather than rejected, so a sidecar
/// written by a newer Uguisu still reads here; a newer `schema` is refused
/// by name, because that is a statement the reader cannot interpret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Sidecar {
    /// Document schema; [`Sidecar::SCHEMA`] is what this build writes.
    pub schema: u32,
    /// What wrote it, for a human reading the file.
    pub generator: String,
    /// When it was written.
    #[serde(with = "time::serde::rfc3339")]
    pub written_at: OffsetDateTime,
    /// The podcast.
    pub podcast: SidecarPodcast,
    /// The episode.
    pub episode: SidecarEpisode,
    /// The file as Uguisu last recorded it.
    pub archive: SidecarArchive,
    /// Where the bytes came from.
    #[serde(default)]
    pub source: Option<SidecarSource>,
}

impl Sidecar {
    /// The schema this build writes and is able to read.
    pub const SCHEMA: u32 = 1;
    /// The `generator` value this build writes.
    pub const GENERATOR: &'static str = "uguisu";
}

/// The podcast half of a [`Sidecar`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SidecarPodcast {
    /// Podcast identifier.
    pub id: PodcastId,
    /// Title as Uguisu knows it.
    pub title: String,
    /// Author, when the feed named one.
    #[serde(default)]
    pub author: Option<String>,
    /// Publisher / owner.
    #[serde(default)]
    pub publisher: Option<String>,
    /// Feed URL of the current source.
    #[serde(default)]
    pub feed_url: Option<Url>,
    /// BCP 47 language tag.
    #[serde(default)]
    pub language: Option<String>,
    /// Categories, in feed order.
    #[serde(default)]
    pub categories: Vec<String>,
}

/// The episode half of a [`Sidecar`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SidecarEpisode {
    /// Episode identifier.
    pub id: EpisodeId,
    /// The stable identity key (`guid:…`, `enclosure:…`, …) — what a
    /// rebuild matches on when the identifier itself is unknown.
    pub identity_key: String,
    /// Where that key came from.
    #[serde(default)]
    pub identity_source: Option<String>,
    /// Title.
    pub title: String,
    /// Publication time, when the feed carried a usable one.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
    /// Season number.
    #[serde(default)]
    pub season: Option<u32>,
    /// Episode number within the season.
    #[serde(default)]
    pub number: Option<u32>,
    /// Duration in seconds.
    #[serde(default)]
    pub duration_secs: Option<u32>,
    /// Plain-text description, as stored (already truncated).
    #[serde(default)]
    pub description_text: Option<String>,
    /// Feed GUID.
    #[serde(default)]
    pub guid: Option<String>,
    /// Episode web page.
    #[serde(default)]
    pub link: Option<Url>,
    /// The enclosure the bytes came from.
    #[serde(default)]
    pub enclosure_url: Option<Url>,
    /// Its declared media type.
    #[serde(default)]
    pub enclosure_type: Option<String>,
    /// Its declared length.
    #[serde(default)]
    pub enclosure_length_bytes: Option<u64>,
    /// Episode artwork as the feed declared it; Uguisu does not download it.
    #[serde(default)]
    pub artwork_url: Option<Url>,
    /// Chapter documents as the feed declared them, not downloaded.
    #[serde(default)]
    pub chapters: Vec<ChaptersRef>,
    /// Transcripts as the feed declared them, not downloaded.
    #[serde(default)]
    pub transcripts: Vec<TranscriptRef>,
}

/// The file half of a [`Sidecar`]: what the record says about the bytes on
/// disk *now*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SidecarArchive {
    /// Path relative to the media root when the sidecar was written. A
    /// reader must not rely on it: the sidecar's own location is the truth
    /// after a user has moved things.
    pub relative_path: String,
    /// Length in bytes.
    pub size_bytes: u64,
    /// Hash algorithm.
    pub hash_algo: String,
    /// Hash of the whole file.
    pub hash_value: String,
    /// `Content-Type` as served.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Container guessed from the first bytes.
    #[serde(default)]
    pub sniffed_type: Option<String>,
    /// How the record came to exist.
    #[serde(default)]
    pub origin: ArchiveOrigin,
    /// Whether Uguisu has written tags, and in which mode.
    #[serde(default)]
    pub tag_state: TagState,
    /// The mode of the last successful tag write.
    #[serde(default)]
    pub tag_mode: Option<TagMode>,
    /// When tags were last written.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub tagged_at: Option<OffsetDateTime>,
    /// When the artifact was first recorded.
    #[serde(with = "time::serde::rfc3339")]
    pub registered_at: OffsetDateTime,
    /// The tags the file carried before Uguisu first wrote any.
    #[serde(default)]
    pub original_tags: Option<OriginalTags>,
}

/// The managed tags a file carried before Uguisu first wrote any (ADR 0012).
///
/// Captured once, before the first tag write, and kept for the life of one
/// download. A file Uguisu had already tagged when this was introduced never
/// gets one: its tags are Uguisu's, and recording them as the original would
/// be false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OriginalTags {
    /// Values by managed field name (`title`, `album`, …); a field the file
    /// did not carry is absent.
    pub values: std::collections::BTreeMap<String, String>,
    /// The embedded cover, described rather than copied.
    #[serde(default)]
    pub cover: Option<OriginalCover>,
    /// When the tags were read.
    #[serde(with = "time::serde::rfc3339")]
    pub captured_at: OffsetDateTime,
}

/// An embedded cover image, as [`OriginalTags`] records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OriginalCover {
    /// Its MIME type.
    pub mime: String,
    /// Its length in bytes.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of its bytes.
    pub sha256: String,
}

/// What was received, as opposed to what is on disk now.
///
/// After a tag write the two differ: `SidecarArchive::hash_value` follows
/// the file, this does not. It is immutable **for the life of one
/// download** — a re-download of the same episode replaces it, because it
/// then describes different bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SidecarSource {
    /// Hash algorithm.
    pub hash_algo: String,
    /// Hash of the bytes as received.
    pub hash_value: String,
    /// Length as received.
    pub size_bytes: u64,
    /// Where they came from: an enclosure URL, or an import's source path.
    #[serde(default)]
    pub origin_detail: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn enums_round_trip_through_their_stable_strings() {
        for s in VerificationState::ALL {
            assert_eq!(VerificationState::parse(s.as_str()), Some(s));
        }
        for d in VerifyDepth::ALL {
            assert_eq!(VerifyDepth::parse(d.as_str()), Some(d));
        }
        for k in ArchiveErrorKind::ALL {
            assert_eq!(ArchiveErrorKind::parse(k.as_str()), Some(k));
        }
        for p in PathProfile::ALL {
            assert_eq!(PathProfile::parse(p.as_str()), Some(p));
        }
        for m in PolicyMode::ALL {
            assert_eq!(PolicyMode::parse(m.as_str()), Some(m));
        }
        for o in ArchiveOrigin::ALL {
            assert_eq!(ArchiveOrigin::parse(o.as_str()), Some(o));
        }
        for t in TagState::ALL {
            assert_eq!(TagState::parse(t.as_str()), Some(t));
        }
        for m in TagMode::ALL {
            assert_eq!(TagMode::parse(m.as_str()), Some(m));
        }
        for a in ArtworkFormat::ALL {
            assert_eq!(ArtworkFormat::parse(a.as_str()), Some(a));
        }
        // The CLI spells the mode with a dash; the database with an
        // underscore. Both read back as the same value.
        assert_eq!(TagMode::parse("fill-missing"), Some(TagMode::FillMissing));
        assert_eq!(TagMode::parse("overwrite"), None);
        assert_eq!(ArtworkFormat::parse("gif"), None);
        assert_eq!(TagState::parse("tagged"), None);
        assert_eq!(VerificationState::parse("gone"), None);
        assert_eq!(PolicyMode::parse("automatic"), None);
    }

    #[test]
    fn defaults_are_the_conservative_ones() {
        assert_eq!(VerificationState::default(), VerificationState::Unchecked);
        assert_eq!(VerifyDepth::default(), VerifyDepth::Light);
        assert_eq!(PathProfile::default(), PathProfile::Portable);
        assert_eq!(PolicyMode::default(), PolicyMode::Manual);
        assert!(!PolicyMode::default().is_auto());
    }

    #[test]
    fn verification_states_say_what_needs_attention() {
        assert!(VerificationState::Missing.is_problem());
        assert!(VerificationState::Invalid.is_problem());
        assert!(!VerificationState::Verified.is_problem());
        assert!(!VerificationState::Unchecked.is_problem());
        assert!(VerifyDepth::Full.hashes());
        assert!(!VerifyDepth::Light.hashes());
        assert!(!VerifyDepth::Existence.hashes());
    }

    #[test]
    fn asset_defaults_do_nothing() {
        assert_eq!(ArchiveOrigin::default(), ArchiveOrigin::Download);
        assert_eq!(TagState::default(), TagState::Untagged);
        assert_eq!(TagMode::default(), TagMode::FillMissing);
        assert!(TagState::Pending.is_in_flight());
        assert!(
            !TagState::Written.is_in_flight(),
            "only an interrupted write needs recovery"
        );
    }

    #[test]
    fn artwork_formats_carry_type_and_extension() {
        assert_eq!(ArtworkFormat::Jpeg.mime(), "image/jpeg");
        assert_eq!(ArtworkFormat::Jpeg.extension(), "jpg");
        assert_eq!(ArtworkFormat::Png.mime(), "image/png");
        assert_eq!(ArtworkFormat::Webp.extension(), "webp");
    }

    fn sample_sidecar() -> Sidecar {
        let at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        Sidecar {
            schema: Sidecar::SCHEMA,
            generator: Sidecar::GENERATOR.to_owned(),
            written_at: at,
            podcast: SidecarPodcast {
                id: PodcastId::new(),
                title: "Show".to_owned(),
                author: Some("Autorin".to_owned()),
                publisher: None,
                feed_url: Some(Url::parse("https://feeds.example/a.xml").unwrap()),
                language: Some("de".to_owned()),
                categories: vec!["Technology".to_owned()],
            },
            episode: SidecarEpisode {
                id: EpisodeId::new(),
                identity_key: "guid:x".to_owned(),
                identity_source: Some("guid".to_owned()),
                title: "Folge 1".to_owned(),
                published_at: Some(at),
                season: Some(1),
                number: Some(2),
                duration_secs: Some(1800),
                description_text: None,
                guid: Some("x".to_owned()),
                link: None,
                enclosure_url: Some(Url::parse("https://cdn.example/a.mp3").unwrap()),
                enclosure_type: Some("audio/mpeg".to_owned()),
                enclosure_length_bytes: Some(1024),
                artwork_url: Some(Url::parse("https://cdn.example/folge-1.jpg").unwrap()),
                chapters: vec![ChaptersRef {
                    url: "https://cdn.example/folge-1.json".to_owned(),
                    mime_type: Some("application/json+chapters".to_owned()),
                }],
                transcripts: vec![TranscriptRef {
                    url: "https://cdn.example/folge-1.vtt".to_owned(),
                    mime_type: Some("text/vtt".to_owned()),
                    language: Some("de".to_owned()),
                    rel: Some("captions".to_owned()),
                }],
            },
            archive: SidecarArchive {
                relative_path: "Show/2023/2023-11-14 - Folge 1.mp3".to_owned(),
                size_bytes: 1024,
                hash_algo: "sha256".to_owned(),
                hash_value: "ab".to_owned(),
                content_type: Some("audio/mpeg".to_owned()),
                sniffed_type: Some("mp3".to_owned()),
                origin: ArchiveOrigin::Download,
                tag_state: TagState::Untagged,
                tag_mode: None,
                tagged_at: None,
                registered_at: at,
                original_tags: None,
            },
            source: Some(SidecarSource {
                hash_algo: "sha256".to_owned(),
                hash_value: "ab".to_owned(),
                size_bytes: 1024,
                origin_detail: Some("https://cdn.example/a.mp3".to_owned()),
            }),
        }
    }

    #[test]
    fn a_sidecar_keeps_its_wire_shape() {
        let s = sample_sidecar();
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["generator"], "uguisu");
        assert_eq!(v["written_at"], "2023-11-14T22:13:20Z");
        assert_eq!(v["podcast"]["title"], "Show");
        assert_eq!(v["episode"]["identity_key"], "guid:x");
        assert_eq!(
            v["episode"]["artwork_url"],
            "https://cdn.example/folge-1.jpg"
        );
        assert_eq!(
            v["episode"]["chapters"][0]["mime_type"],
            "application/json+chapters"
        );
        assert_eq!(v["episode"]["transcripts"][0]["rel"], "captions");
        assert_eq!(v["archive"]["hash_algo"], "sha256");
        assert_eq!(v["archive"]["origin"], "download");
        assert_eq!(v["archive"]["tag_state"], "untagged");
        assert_eq!(v["source"]["hash_value"], "ab");
        let back: Sidecar = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn a_newer_sidecar_still_reads() {
        // Forward compatibility is the whole point of the document: an
        // unknown field is ignored, an absent optional field defaults.
        // Only the schema number itself is a statement this build may not
        // reinterpret, and refusing it is `sidecar::parse`'s job.
        let json = serde_json::json!({
            "schema": 1,
            "generator": "uguisu/9.9.9",
            "written_at": "2023-11-14T22:13:20Z",
            "chapters": [{"start_ms": 0, "title": "Intro"}],
            "podcast": {"id": PodcastId::new().to_string(), "title": "Show"},
            "episode": {
                "id": EpisodeId::new().to_string(),
                "identity_key": "guid:x",
                "title": "Folge 1",
                "soundbites": [{"start_secs": 12, "duration_secs": 30}]
            },
            "archive": {
                "relative_path": "Show/a.mp3",
                "size_bytes": 1,
                "hash_algo": "sha256",
                "hash_value": "ab",
                "registered_at": "2023-11-14T22:13:20Z"
            }
        });
        let s: Sidecar = serde_json::from_value(json).unwrap();
        assert_eq!(s.podcast.categories, Vec::<String>::new());
        assert_eq!(s.episode.season, None);
        // What a sidecar from before the feed's references were kept lacks.
        assert!(s.episode.artwork_url.is_none());
        assert!(s.episode.chapters.is_empty());
        assert!(s.episode.transcripts.is_empty());
        assert_eq!(s.archive.origin, ArchiveOrigin::Download);
        assert_eq!(s.archive.tag_state, TagState::Untagged);
        assert!(s.source.is_none(), "an absent source is not an error");
    }

    #[test]
    fn error_kinds_are_distinct_and_named() {
        // 21 kinds, every string distinct: a caller that matches on the
        // string must never see two kinds collapse into one.
        let names: std::collections::BTreeSet<&str> =
            ArchiveErrorKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(names.len(), ArchiveErrorKind::ALL.len());
        assert!(names.contains("import_ambiguous"));
        assert!(names.contains("tags_unsupported"));
        assert_eq!(
            ArchiveErrorKind::SidecarInvalid.to_string(),
            "sidecar_invalid"
        );
    }

    #[test]
    fn a_rebuilt_record_is_never_evidence() {
        // The reason vocabulary has to keep these apart: `rebuilt` means
        // "a sidecar said so", `hash_match` means "the bytes were read".
        assert_ne!(reason::REBUILT, reason::HASH_MATCH);
        assert_eq!(reason::REBUILT, "rebuilt");
        assert_eq!(reason::IMPORTED, "imported");
        assert_eq!(reason::TAGGED, "tagged");
        assert_eq!(reason::RETAG_RECOVERED, "retag_recovered");
    }

    #[test]
    fn a_decision_queues_or_explains() {
        let q = PolicyDecision::Queue {
            priority: Priority::Normal,
        };
        assert!(q.queues());
        assert_eq!(q.reason(), None);
        let s = PolicyDecision::Skip {
            reason: policy_reason::TOO_OLD.to_owned(),
        };
        assert!(!s.queues());
        assert_eq!(s.reason(), Some("too_old"));
        // The tag is what the wire and the CLI match on.
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"decision":"skip","reason":"too_old"}"#);
    }
}
