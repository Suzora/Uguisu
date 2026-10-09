//! Download vocabulary: job states, priorities, error classification and
//! the persisted job/attempt records (`docs/DOWNLOAD_ENGINE.md`, ADR 0018).
//!
//! The enums keep the stable string forms that are stored in the database
//! and shown on the wire (`as_str`/`parse`), like the model enums.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

use crate::ids::{AttemptId, EnclosureId, EpisodeId, JobId, PodcastId};

/// Persisted state of a download job (ADR 0018).
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
pub enum DownloadState {
    /// Waiting for a worker.
    #[default]
    Queued,
    /// A worker holds the job and transfers bytes.
    Downloading,
    /// Every byte is on disk and validated; the rename is in progress.
    Finalizing,
    /// A transient failure; eligible again at `next_attempt_at`.
    Retrying,
    /// Stopped by the user, by a global pause or by a full disk; keeps its `.part`.
    Paused,
    /// The final file exists at `target_path`.
    Completed,
    /// Gave up; a user retry re-queues it.
    Failed,
    /// Cancelled by the user; a user retry re-queues it.
    Cancelled,
}

impl DownloadState {
    /// Every variant.
    pub const ALL: [Self; 8] = [
        Self::Queued,
        Self::Downloading,
        Self::Finalizing,
        Self::Retrying,
        Self::Paused,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Finalizing => "finalizing",
            Self::Retrying => "retrying",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Whether no transition leaves this state (only `completed`).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed)
    }

    /// Whether a worker currently owns the job.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Downloading | Self::Finalizing)
    }

    /// Whether the job still wants to be downloaded (not done, not stopped).
    #[must_use]
    pub const fn is_pending(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Downloading | Self::Finalizing | Self::Retrying
        )
    }
}

impl std::fmt::Display for DownloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Queue priority; higher runs first, FIFO within a level.
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
pub enum Priority {
    /// Background work.
    Low,
    /// The default.
    #[default]
    Normal,
    /// Ahead of everything else.
    High,
}

impl Priority {
    /// Every variant, ascending.
    pub const ALL: [Self; 3] = [Self::Low, Self::Normal, Self::High];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.as_str() == s)
    }

    /// The stored numeric form (0 low, 1 normal, 2 high).
    #[must_use]
    pub const fn as_i64(self) -> i64 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::High => 2,
        }
    }

    /// Parses the stored numeric form.
    #[must_use]
    pub const fn from_i64(n: i64) -> Option<Self> {
        match n {
            0 => Some(Self::Low),
            1 => Some(Self::Normal),
            2 => Some(Self::High),
            _ => None,
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classification of a failed download attempt (`docs/DOWNLOAD_ENGINE.md`
/// "Retry taxonomy"). Whether a kind is retried is decided by
/// [`DownloadErrorKind::is_retryable`]; the worker may still refuse a retry
/// when the attempt budget is spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadErrorKind {
    /// Connection reset, transport failure, early close.
    Network,
    /// Headers or the next chunk did not arrive in time.
    Timeout,
    /// Host did not resolve.
    Dns,
    /// TLS handshake or certificate failure.
    Tls,
    /// An HTTP status that is neither one of the dedicated kinds nor success.
    Http,
    /// 429.
    RateLimited,
    /// 404 or 410.
    NotFound,
    /// 401.
    Unauthorized,
    /// 403.
    Forbidden,
    /// A resume was attempted but the server answered with a full body.
    RangeUnsupported,
    /// The `Content-Range` answer does not match the request.
    RangeInvalid,
    /// Fewer or more bytes than the declared length.
    ContentLengthMismatch,
    /// No space left on the media file system.
    DiskFull,
    /// The media directory or file is not writable.
    PermissionDenied,
    /// Any other file system error.
    Io,
    /// The body failed validation (empty, oversized, malformed range answer).
    Validation,
    /// Cancelled by the caller or shutdown.
    Cancelled,
    /// The database failed while the job ran.
    Storage,
    /// The URL or a redirect was refused by the network policy.
    PolicyBlocked,
}

impl DownloadErrorKind {
    /// Every variant.
    pub const ALL: [Self; 19] = [
        Self::Network,
        Self::Timeout,
        Self::Dns,
        Self::Tls,
        Self::Http,
        Self::RateLimited,
        Self::NotFound,
        Self::Unauthorized,
        Self::Forbidden,
        Self::RangeUnsupported,
        Self::RangeInvalid,
        Self::ContentLengthMismatch,
        Self::DiskFull,
        Self::PermissionDenied,
        Self::Io,
        Self::Validation,
        Self::Cancelled,
        Self::Storage,
        Self::PolicyBlocked,
    ];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Dns => "dns",
            Self::Tls => "tls",
            Self::Http => "http",
            Self::RateLimited => "rate_limited",
            Self::NotFound => "not_found",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::RangeUnsupported => "range_unsupported",
            Self::RangeInvalid => "range_invalid",
            Self::ContentLengthMismatch => "content_length_mismatch",
            Self::DiskFull => "disk_full",
            Self::PermissionDenied => "permission_denied",
            Self::Io => "io",
            Self::Validation => "validation",
            Self::Cancelled => "cancelled",
            Self::Storage => "storage",
            Self::PolicyBlocked => "policy_blocked",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Whether a later attempt can reasonably succeed without user action.
    /// `DiskFull` is not retried but pauses the queue instead.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Network
                | Self::Timeout
                | Self::Dns
                | Self::Http
                | Self::RateLimited
                | Self::ContentLengthMismatch
        )
    }

    /// Whether the failure was caused by the network or the remote host
    /// (for exit-code mapping), as opposed to local validation or I/O.
    #[must_use]
    pub const fn is_network(self) -> bool {
        matches!(
            self,
            Self::Network
                | Self::Timeout
                | Self::Dns
                | Self::Tls
                | Self::Http
                | Self::RateLimited
                | Self::NotFound
                | Self::Unauthorized
                | Self::Forbidden
        )
    }
}

impl std::fmt::Display for DownloadErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How an attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// The file was finalized.
    Completed,
    /// The job failed for good.
    Failed,
    /// A retry was scheduled.
    RetryScheduled,
    /// Cancelled by the user.
    Cancelled,
    /// Paused (user, global pause, disk full).
    Paused,
    /// The process stopped before the attempt ended (shutdown or crash).
    Interrupted,
}

impl AttemptOutcome {
    /// Every variant.
    pub const ALL: [Self; 6] = [
        Self::Completed,
        Self::Failed,
        Self::RetryScheduled,
        Self::Cancelled,
        Self::Paused,
        Self::Interrupted,
    ];

    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::RetryScheduled => "retry_scheduled",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
            Self::Interrupted => "interrupted",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

/// Why every download is paused (`download_control`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PauseAllReason {
    /// `download pause-all`.
    User,
    /// A worker hit a full disk; resumes only on `resume-all`.
    DiskFull,
}

impl PauseAllReason {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::DiskFull => "disk_full",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "disk_full" => Some(Self::DiskFull),
            _ => None,
        }
    }
}

/// A download job (`download_jobs`); exactly one per episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DownloadJob {
    /// Identifier; also the `.part` file name.
    pub id: JobId,
    /// The episode being downloaded (unique).
    pub episode_id: EpisodeId,
    /// Its podcast.
    pub podcast_id: PodcastId,
    /// The enclosure row the URL was taken from (may be gone after a refresh).
    pub enclosure_id: Option<EnclosureId>,
    /// The URL being fetched.
    pub source_url: Url,
    /// `scheme://host:port` for per-host limits.
    pub host_key: String,
    /// State.
    pub state: DownloadState,
    /// Why the job is in that state (stable vocabulary, `docs/DOWNLOAD_ENGINE.md`).
    pub state_reason: Option<String>,
    /// Queue priority.
    pub priority: Priority,
    /// Attempts started so far.
    pub attempt_count: u32,
    /// Attempt budget.
    pub max_attempts: u32,
    /// When a `retrying` job becomes eligible.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_attempt_at: Option<OffsetDateTime>,
    /// Acknowledged bytes in the `.part` file.
    pub bytes_downloaded: u64,
    /// Complete length when known.
    pub total_bytes: Option<u64>,
    /// `.part` path relative to the media directory (POSIX separators).
    pub part_path: String,
    /// Final path relative to the media directory (POSIX separators).
    pub target_path: String,
    /// `Content-Type` as served (recorded, never enforced).
    pub content_type: Option<String>,
    /// Container guessed from the first bytes (recorded, never enforced).
    pub sniffed_type: Option<String>,
    /// `ETag` of the resource being resumed.
    pub etag: Option<String>,
    /// `Last-Modified` of the resource being resumed.
    pub last_modified: Option<String>,
    /// Whether the server supports byte ranges (`None` = unknown).
    pub accept_ranges: Option<bool>,
    /// Hash algorithm (`sha256`).
    pub hash_algo: String,
    /// Hash of the complete file, set when finalization starts.
    pub hash_value: Option<String>,
    /// Last HTTP status seen.
    pub last_http_status: Option<u16>,
    /// Classification of the last failure.
    pub last_error_kind: Option<DownloadErrorKind>,
    /// Detail of the last failure (no headers, no secrets).
    pub last_error_detail: Option<String>,
    /// When a worker last claimed the job.
    #[serde(with = "time::serde::rfc3339::option")]
    pub claimed_at: Option<OffsetDateTime>,
    /// When progress was last persisted.
    #[serde(with = "time::serde::rfc3339::option")]
    pub progress_at: Option<OffsetDateTime>,
    /// First claim.
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    /// Completion, failure or cancellation time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    /// Creation time (queue order within a priority).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl DownloadJob {
    /// Completion as a fraction when the total is known.
    #[must_use]
    pub fn percentage(&self) -> Option<f32> {
        percentage(self.bytes_downloaded, self.total_bytes)
    }
}

/// A queue row with the names a reader needs.
///
/// Flattened on the wire, so a client that knew only `DownloadJob` still reads
/// every field it knew and gains four. `DownloadJob` itself is untouched: it
/// is the persisted record, and an episode's title is not part of it — it
/// belongs to the episode, and duplicating it into the job would make the two
/// able to disagree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobSummary {
    /// The job.
    #[serde(flatten)]
    pub job: DownloadJob,
    /// The episode's title.
    pub episode_title: String,
    /// Its podcast's title.
    pub podcast_title: String,
    /// When the episode was published, when the feed said so.
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
    /// Transferred percentage, when the total length is known.
    pub percentage: Option<f32>,
}

/// One attempt of a job (`download_attempts`, append-only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DownloadAttempt {
    /// Identifier.
    pub id: AttemptId,
    /// The job.
    pub job_id: JobId,
    /// 1-based attempt number.
    pub attempt_no: u32,
    /// Start.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// End, when the attempt ended.
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    /// URL used.
    pub source_url: Url,
    /// Offset the attempt started at (0 = fresh).
    pub range_start: u64,
    /// HTTP status of the final hop.
    pub http_status: Option<u16>,
    /// Bytes received in this attempt.
    pub bytes_received: u64,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Average rate.
    pub avg_rate_bps: Option<u64>,
    /// How it ended.
    pub outcome: Option<AttemptOutcome>,
    /// Failure classification.
    pub error_kind: Option<DownloadErrorKind>,
    /// Failure detail.
    pub error_detail: Option<String>,
    /// Scheduled retry, when one was scheduled.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_attempt_at: Option<OffsetDateTime>,
}

/// Global queue control (`download_control`, one row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DownloadControl {
    /// Whether no job is claimed.
    pub paused: bool,
    /// Why.
    pub paused_reason: Option<PauseAllReason>,
    /// Since when.
    #[serde(with = "time::serde::rfc3339::option")]
    pub paused_at: Option<OffsetDateTime>,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Live progress of a running job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProgressSnapshot {
    /// Bytes on disk including the resumed prefix.
    pub bytes_downloaded: u64,
    /// Complete length when known.
    pub total_bytes: Option<u64>,
    /// Completion when the total is known.
    pub percentage: Option<f32>,
    /// Smoothed transfer rate (exponential moving average of 1 s samples).
    pub speed_bps: u64,
    /// Remaining seconds at the smoothed rate, when the total is known.
    pub eta_secs: Option<u64>,
    /// When the snapshot was taken.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// Completion as a fraction of `total` in percent, `None` when unknown.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn percentage(done: u64, total: Option<u64>) -> Option<f32> {
    let total = total?;
    if total == 0 {
        return Some(100.0);
    }
    Some(((done.min(total) as f64) * 100.0 / total as f64) as f32)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn enums_round_trip_and_serialize_snake_case() {
        for s in DownloadState::ALL {
            assert_eq!(DownloadState::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
        for k in DownloadErrorKind::ALL {
            assert_eq!(DownloadErrorKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
        for o in AttemptOutcome::ALL {
            assert_eq!(AttemptOutcome::parse(o.as_str()), Some(o));
        }
        for p in Priority::ALL {
            assert_eq!(Priority::parse(p.as_str()), Some(p));
            assert_eq!(Priority::from_i64(p.as_i64()), Some(p));
        }
        assert_eq!(Priority::from_i64(7), None);
        assert!(Priority::High > Priority::Normal && Priority::Normal > Priority::Low);
        assert_eq!(
            PauseAllReason::parse("disk_full"),
            Some(PauseAllReason::DiskFull)
        );
        assert_eq!(DownloadState::parse("verified"), None);
    }

    #[test]
    fn state_classes() {
        assert!(DownloadState::Completed.is_terminal());
        assert!(!DownloadState::Failed.is_terminal());
        assert!(DownloadState::Downloading.is_active());
        assert!(DownloadState::Finalizing.is_active());
        assert!(DownloadState::Retrying.is_pending());
        assert!(!DownloadState::Paused.is_pending());
    }

    #[test]
    fn retryable_and_network_classes() {
        assert!(DownloadErrorKind::RateLimited.is_retryable());
        assert!(DownloadErrorKind::ContentLengthMismatch.is_retryable());
        assert!(!DownloadErrorKind::NotFound.is_retryable());
        assert!(!DownloadErrorKind::DiskFull.is_retryable());
        assert!(!DownloadErrorKind::PolicyBlocked.is_retryable());
        assert!(DownloadErrorKind::Forbidden.is_network());
        assert!(!DownloadErrorKind::Io.is_network());
    }

    #[test]
    fn percentage_math() {
        assert_eq!(percentage(50, Some(200)), Some(25.0));
        assert_eq!(percentage(5, None), None);
        assert_eq!(percentage(0, Some(0)), Some(100.0));
        assert_eq!(percentage(300, Some(200)), Some(100.0));
    }
}
