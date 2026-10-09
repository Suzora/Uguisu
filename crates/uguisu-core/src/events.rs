//! Domain events (ADR 0010).
//!
//! An [`Event`] describes something that happened; it never instructs a
//! consumer. `episode.discovered` means "the episode exists in the feed",
//! not "download it" — policies decide that later (`docs/FEED_ENGINE.md`).
//! The wire shape is stable and versioned by `schema`.

// The schema derive builds one array holding every variant's schema, and
// `EventKind` has enough variants to make that array large. It is built once,
// when the OpenAPI document is generated, and nothing else in this file
// allocates an array at all.
#![allow(clippy::large_stack_arrays)]

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

use crate::archive::{ArtworkFormat, TagMode, VerifyDepth};
use crate::download::{DownloadErrorKind, PauseAllReason, Priority};
use crate::feed::{EpisodeCounts, FetchErrorKind, NotModifiedReason};
use crate::ids::{
    ArchiveFileId, ArtworkId, EpisodeId, EventId, FetchId, JobId, PodcastId, SourceId,
};
use crate::model::{DuplicateResolution, ReplacementReason};

/// Event kinds with their payloads. Serialized with a `kind` tag holding the
/// dotted event name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind")]
pub enum EventKind {
    /// A podcast was added to the library.
    #[serde(rename = "podcast.added")]
    PodcastAdded {
        /// Title.
        title: String,
        /// Feed URL of the current source.
        feed_url: Url,
        /// Source id.
        source_id: SourceId,
    },
    /// A person removed a podcast from the library; its files stay
    /// (ADR 0055).
    #[serde(rename = "podcast.removed")]
    PodcastRemoved {
        /// Title.
        title: String,
        /// Feed URL of the source that was current.
        feed_url: Option<Url>,
        /// Episodes the library held for it.
        episodes: u64,
        /// Archived files it had, all of them still on disk.
        files: u64,
    },
    /// A refresh started.
    #[serde(rename = "podcast.feed.refresh.started")]
    FeedRefreshStarted {
        /// Source id.
        source_id: SourceId,
        /// Feed URL.
        feed_url: Url,
        /// Whether validators were sent.
        conditional: bool,
    },
    /// A refresh fetched and processed the feed.
    #[serde(rename = "podcast.feed.refresh.completed")]
    FeedRefreshCompleted {
        /// Source id.
        source_id: SourceId,
        /// Fetch log id.
        fetch_id: FetchId,
        /// Counts.
        episodes: EpisodeCounts,
        /// Whether podcast metadata changed.
        podcast_changed: bool,
        /// Whether the document was truncated.
        truncated: bool,
    },
    /// A refresh failed; the previous state is retained.
    #[serde(rename = "podcast.feed.refresh.failed")]
    FeedRefreshFailed {
        /// Source id.
        source_id: SourceId,
        /// Fetch log id.
        fetch_id: FetchId,
        /// Classification.
        error_kind: FetchErrorKind,
        /// Detail.
        detail: String,
        /// HTTP status when one was received.
        http_status: Option<u16>,
        /// Failures in a row.
        consecutive_failures: u32,
    },
    /// The feed was unchanged.
    #[serde(rename = "podcast.feed.not_modified")]
    FeedNotModified {
        /// Source id.
        source_id: SourceId,
        /// Fetch log id.
        fetch_id: FetchId,
        /// How that was determined.
        reason: NotModifiedReason,
    },
    /// Podcast-level metadata changed.
    #[serde(rename = "podcast.metadata.updated")]
    PodcastMetadataUpdated {
        /// Changed field names.
        fields: Vec<String>,
    },
    /// An episode exists in the feed and was stored for the first time.
    #[serde(rename = "episode.discovered")]
    EpisodeDiscovered {
        /// Title.
        title: String,
        /// Identity key.
        identity_key: String,
        /// Primary enclosure URL, when present.
        enclosure_url: Option<Url>,
        /// Publication time.
        #[serde(with = "time::serde::rfc3339::option")]
        published_at: Option<OffsetDateTime>,
    },
    /// A stored episode's fields changed.
    #[serde(rename = "episode.updated")]
    EpisodeUpdated {
        /// Changed field names.
        fields: Vec<String>,
    },
    /// A stored episode is missing from the feed for the configured streak.
    #[serde(rename = "episode.removal_detected")]
    EpisodeRemovalDetected {
        /// Consecutive fetches without the item.
        missing_streak: u32,
    },
    /// An item looked like an existing episode but carried a contradicting
    /// identity; stored as a candidate duplicate (ADR 0014).
    #[serde(rename = "episode.identity_ambiguous")]
    EpisodeIdentityAmbiguous {
        /// The episode it probably duplicates.
        duplicate_of: EpisodeId,
        /// Match reasons.
        reasons: Vec<String>,
    },
    /// A person resolved a candidate duplicate (ADR 0051). The envelope's
    /// episode is the one that remains.
    #[serde(rename = "episode.duplicate_resolved")]
    EpisodeDuplicateResolved {
        /// The candidate.
        candidate: EpisodeId,
        /// The episode it was a candidate duplicate of.
        original: EpisodeId,
        /// How it was resolved.
        resolution: DuplicateResolution,
    },
    /// The feed announced a new URL that could not be verified yet.
    #[serde(rename = "feed.url.change_detected")]
    FeedUrlChangeDetected {
        /// Source id.
        source_id: SourceId,
        /// Announced URL.
        announced: Url,
        /// How it was announced.
        via: ReplacementReason,
        /// Why it was not adopted.
        reason: String,
    },
    /// The current source was migrated to a verified new URL.
    #[serde(rename = "feed.url.changed")]
    FeedUrlChanged {
        /// Previous source.
        from_source_id: SourceId,
        /// New source.
        to_source_id: SourceId,
        /// Previous URL.
        from: Url,
        /// New URL.
        to: Url,
        /// How it was announced.
        via: ReplacementReason,
    },
    /// A download job was created or re-queued.
    #[serde(rename = "download.queued")]
    DownloadQueued {
        /// Job id.
        job_id: JobId,
        /// URL to fetch.
        enclosure_url: Url,
        /// Priority.
        priority: Priority,
        /// `true` when an existing job was queued again: a failed or cancelled
        /// one, or a completed one by a redownload.
        requeued: bool,
    },
    /// A worker claimed the job and the request is on its way.
    #[serde(rename = "download.started")]
    DownloadStarted {
        /// Job id.
        job_id: JobId,
        /// 1-based attempt number.
        attempt: u32,
        /// Bytes already on disk that the attempt resumes from.
        resumed_from: u64,
        /// Complete length when known before the request.
        total_bytes: Option<u64>,
        /// URL being fetched.
        url: Url,
    },
    /// Throttled progress; published live, never stored.
    #[serde(rename = "download.progress")]
    DownloadProgress {
        /// Job id.
        job_id: JobId,
        /// Bytes on disk.
        bytes_downloaded: u64,
        /// Complete length when known.
        total_bytes: Option<u64>,
        /// Completion in percent when the total is known.
        percentage: Option<f32>,
        /// Smoothed rate.
        speed_bps: u64,
        /// Remaining seconds at that rate, when the total is known.
        eta_secs: Option<u64>,
    },
    /// The job was paused (its `.part` is kept).
    #[serde(rename = "download.paused")]
    DownloadPaused {
        /// Job id.
        job_id: JobId,
        /// Why (`user`, `paused_all`, `disk_full`).
        reason: String,
        /// Bytes kept.
        bytes_downloaded: u64,
    },
    /// A paused job was re-queued.
    #[serde(rename = "download.resumed")]
    DownloadResumed {
        /// Job id.
        job_id: JobId,
    },
    /// A transient failure; the job waits for its next attempt.
    #[serde(rename = "download.retry_scheduled")]
    DownloadRetryScheduled {
        /// Job id.
        job_id: JobId,
        /// The attempt that failed.
        attempt: u32,
        /// Classification.
        error_kind: DownloadErrorKind,
        /// Detail.
        detail: String,
        /// HTTP status when one was received.
        http_status: Option<u16>,
        /// When the next attempt may start.
        #[serde(with = "time::serde::rfc3339")]
        next_attempt_at: OffsetDateTime,
    },
    /// The file was validated and moved to its final path.
    #[serde(rename = "download.completed")]
    DownloadCompleted {
        /// Job id.
        job_id: JobId,
        /// Final path relative to the media directory (POSIX separators).
        path: String,
        /// File size.
        size_bytes: u64,
        /// Hash algorithm.
        hash_algo: String,
        /// Hash of the file.
        hash_value: String,
        /// `Content-Type` as served.
        content_type: Option<String>,
        /// Container guessed from the first bytes.
        sniffed_type: Option<String>,
        /// Attempts it took.
        attempts: u32,
        /// Wall-clock time of the last attempt.
        duration_ms: u64,
    },
    /// The job gave up.
    #[serde(rename = "download.failed")]
    DownloadFailed {
        /// Job id.
        job_id: JobId,
        /// Why (`max_attempts`, `not_retryable`, `validation`, `target_exists`, …).
        reason: String,
        /// Classification of the last error.
        error_kind: DownloadErrorKind,
        /// Detail.
        detail: String,
        /// HTTP status when one was received.
        http_status: Option<u16>,
        /// Attempts made.
        attempts: u32,
    },
    /// The job was cancelled by the user.
    #[serde(rename = "download.cancelled")]
    DownloadCancelled {
        /// Job id.
        job_id: JobId,
        /// Bytes kept in the `.part`.
        bytes_downloaded: u64,
    },
    /// Every download was paused.
    #[serde(rename = "download.paused_all")]
    DownloadPausedAll {
        /// Why.
        reason: PauseAllReason,
    },
    /// Downloads resume after a global pause.
    #[serde(rename = "download.resumed_all")]
    DownloadResumedAll {},
    /// A finished download was recorded as an archive artifact.
    #[serde(rename = "archive.registered")]
    ArchiveRegistered {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Path relative to the media root.
        path: String,
        /// File size.
        size_bytes: u64,
        /// Hash algorithm.
        hash_algo: String,
        /// Hash of the file.
        hash_value: String,
    },
    /// A verification pass found the artifact intact.
    #[serde(rename = "archive.verified")]
    ArchiveVerified {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Path relative to the media root.
        path: String,
        /// How hard the check looked.
        depth: VerifyDepth,
        /// Why it passed (`size_match`, `hash_match`).
        reason: String,
    },
    /// The recorded file is gone. The record is kept.
    #[serde(rename = "archive.missing")]
    ArchiveMissing {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Path that was checked.
        path: String,
    },
    /// Something is at the path, but it is not the recorded artifact.
    #[serde(rename = "archive.invalid")]
    ArchiveInvalid {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Path that was checked.
        path: String,
        /// How hard the check looked.
        depth: VerifyDepth,
        /// Why it failed (`size_mismatch`, `hash_mismatch`, `not_a_file`, …).
        reason: String,
    },
    /// An artifact was moved to a new path.
    #[serde(rename = "archive.relocated")]
    ArchiveRelocated {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Where it was.
        from: String,
        /// Where it is now.
        to: String,
    },
    /// A refresh found the feed pointing at different audio for an archived
    /// episode: another URL or another declared length. The file is kept.
    #[serde(rename = "archive.source_changed")]
    ArchiveSourceChanged {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// The archived file.
        path: String,
        /// The primary enclosure's URL before the refresh.
        old_url: String,
        /// Its URL now.
        new_url: String,
        /// Its declared length before, when the feed gave one.
        old_length: Option<u64>,
        /// Its declared length now.
        new_length: Option<u64>,
    },
    /// The archive policy queued a discovered episode.
    #[serde(rename = "archive.policy_queued")]
    ArchivePolicyQueued {
        /// The job the policy created or re-used.
        job_id: JobId,
        /// Policy vocabulary version, so a consumer can tell the rules apart.
        policy_version: u32,
        /// Priority the job was queued at.
        priority: Priority,
    },
    /// The archive policy left a discovered episode alone.
    #[serde(rename = "archive.policy_skipped")]
    ArchivePolicySkipped {
        /// Policy vocabulary version.
        policy_version: u32,
        /// Why, from `archive::policy_reason`.
        reason: String,
    },
    /// The portable sidecar next to an artifact was written.
    #[serde(rename = "archive.sidecar.written")]
    ArchiveSidecarWritten {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Sidecar path relative to the media root.
        path: String,
    },
    /// A sidecar was found but could not be read. It is left on disk.
    #[serde(rename = "archive.sidecar.invalid")]
    ArchiveSidecarInvalid {
        /// Sidecar path relative to the media root.
        path: String,
        /// Why (`malformed`, `too_large`, `unsupported_schema`, …).
        reason: String,
    },
    /// A podcast's manifest was rewritten from the index.
    #[serde(rename = "archive.manifest.written")]
    ArchiveManifestWritten {
        /// Manifest path relative to the media root.
        path: String,
        /// How many artifacts it lists.
        entries: u64,
    },
    /// A manifest check disagreed with the files on disk.
    #[serde(rename = "archive.manifest.mismatch")]
    ArchiveManifestMismatch {
        /// Manifest path relative to the media root.
        path: String,
        /// Listed files whose bytes differ.
        changed: u64,
        /// Listed files that are gone.
        missing: u64,
        /// Files present under the podcast that the manifest does not list.
        added: u64,
    },
    /// A rebuild pass finished. Counts only: a list of every scanned path
    /// would be the memory bug this engine exists to avoid.
    #[serde(rename = "archive.rebuild.completed")]
    ArchiveRebuildCompleted {
        /// Whether records were written (`false` is a dry run).
        applied: bool,
        /// Sidecars read.
        scanned: u64,
        /// Records restored.
        rebuilt: u64,
        /// Records that already agreed with their sidecar.
        unchanged: u64,
        /// Sidecars that contradict an existing record, or each other.
        conflicts: u64,
        /// Sidecars naming an episode this library does not have.
        unknown_episode: u64,
    },
    /// One file was copied in from a foreign archive and registered.
    #[serde(rename = "archive.imported")]
    ArchiveImported {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Where it landed, relative to the media root.
        path: String,
        /// Matching confidence in hundredths (100 = certain).
        confidence: u32,
        /// Why it matched (`scored`, `embedded_guid`, `source_database`).
        matched_by: String,
    },
    /// One source file was not imported, and why.
    #[serde(rename = "archive.import.skipped")]
    ArchiveImportSkipped {
        /// How it was classified (`ambiguous`, `unmatched`, `already_present`, …).
        state: String,
        /// Detail.
        reason: String,
    },
    /// An import run finished.
    #[serde(rename = "archive.import.completed")]
    ArchiveImportCompleted {
        /// Whether files were copied (`false` is a dry run).
        applied: bool,
        /// The layout that read the source tree.
        format: String,
        /// Files examined.
        scanned: u64,
        /// Files imported.
        imported: u64,
        /// Files that matched more than one episode too closely to choose.
        ambiguous: u64,
        /// Files that matched nothing well enough.
        unmatched: u64,
        /// Files Uguisu already has, byte for byte.
        already_present: u64,
    },
    /// Uguisu wrote metadata tags into an artifact. The record moved with
    /// the bytes; the `source_*` provenance did not.
    #[serde(rename = "archive.tagged")]
    ArchiveTagged {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Path relative to the media root.
        path: String,
        /// Which mode wrote them.
        mode: TagMode,
        /// The managed fields that changed.
        fields: Vec<String>,
        /// The file's new size.
        size_bytes: u64,
        /// The file's new hash.
        hash_value: String,
    },
    /// An artifact was left untagged, and why.
    #[serde(rename = "archive.tags.skipped")]
    ArchiveTagsSkipped {
        /// Archive record id.
        archive_file_id: ArchiveFileId,
        /// Why (`unsupported`, `nothing_to_write`, `not_verified`, …).
        reason: String,
    },
    /// Podcast artwork was fetched and stored.
    #[serde(rename = "podcast.artwork.fetched")]
    PodcastArtworkFetched {
        /// Artwork record id.
        artwork_id: ArtworkId,
        /// Path relative to the media root.
        path: String,
        /// The container, as recognised from the bytes.
        format: ArtworkFormat,
        /// Length in bytes.
        size_bytes: u64,
        /// Hash of the file.
        hash_value: String,
    },
    /// The server said the stored artwork is still current.
    #[serde(rename = "podcast.artwork.unchanged")]
    PodcastArtworkUnchanged {
        /// The artwork that was revalidated.
        artwork_id: ArtworkId,
    },
    /// Artwork could not be fetched or was refused. Nothing was stored and
    /// the previous artwork is untouched.
    #[serde(rename = "podcast.artwork.failed")]
    PodcastArtworkFailed {
        /// Where it was fetched from.
        url: Url,
        /// Why: the HTTP client's error kind (`policy`, `dns`, `connect`,
        /// `timeout`, `body_too_large`, …), `http_<status>`, or why the image
        /// was refused.
        reason: String,
    },
    /// The refresh scheduler completed a pass. Live-only, like download
    /// progress: a tick every few minutes for the life of the process is a
    /// heartbeat for a dashboard, not a fact worth keeping for a year.
    #[serde(rename = "scheduler.tick")]
    SchedulerTick {
        /// Podcasts the pass found due.
        due: u64,
        /// Refreshes it started (bounded by the free concurrency).
        started: u64,
        /// How long it intends to sleep before looking again.
        sleep_ms: u64,
    },
    /// Automatic refreshing stopped. The downloads queue is separate: a
    /// paused scheduler still lets transfers run, and a paused queue still
    /// lets feeds be refreshed.
    #[serde(rename = "scheduler.paused")]
    SchedulerPaused {
        /// Why, as the operator or the engine said it.
        reason: String,
    },
    /// Automatic refreshing resumed.
    #[serde(rename = "scheduler.resumed")]
    SchedulerResumed {},
    /// A stored setting was written or cleared.
    ///
    /// The value is deliberately absent: the `settings` table already
    /// holds the current one with `updated_at` and `updated_by`, and an
    /// event log is copied into support tickets and issue reports, where a
    /// base URL or a path template is nobody else's business.
    #[serde(rename = "settings.changed")]
    SettingsChanged {
        /// The `UGUISU_*` key.
        key: String,
        /// Whether the row was removed rather than written.
        removed: bool,
        /// Whether the change needs a restart to take effect.
        restart_required: bool,
    },
    /// A stored setting was not applied. The row is kept exactly as it was
    /// written — this says it is being ignored, and why (ADR 0028).
    #[serde(rename = "settings.rejected")]
    SettingsRejected {
        /// The `UGUISU_*` key, or an unknown one.
        key: String,
        /// The parser's message, or why the key is refused.
        message: String,
    },
    /// The local search index was (re)built.
    #[serde(rename = "search.reindexed")]
    SearchReindexed {
        /// Podcasts indexed.
        podcasts: u64,
        /// Episodes indexed.
        episodes: u64,
        /// How long it took.
        duration_ms: u64,
        /// Whether the index was rebuilt from scratch.
        full: bool,
    },
}

impl EventKind {
    /// Whether the event is live-only (never persisted): true for
    /// download progress and for the scheduler's heartbeat.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::DownloadProgress { .. } | Self::SchedulerTick { .. }
        )
    }

    /// The dotted event name (also the `kind` column).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::PodcastAdded { .. } => "podcast.added",
            Self::PodcastRemoved { .. } => "podcast.removed",
            Self::FeedRefreshStarted { .. } => "podcast.feed.refresh.started",
            Self::FeedRefreshCompleted { .. } => "podcast.feed.refresh.completed",
            Self::FeedRefreshFailed { .. } => "podcast.feed.refresh.failed",
            Self::FeedNotModified { .. } => "podcast.feed.not_modified",
            Self::PodcastMetadataUpdated { .. } => "podcast.metadata.updated",
            Self::EpisodeDiscovered { .. } => "episode.discovered",
            Self::EpisodeUpdated { .. } => "episode.updated",
            Self::EpisodeRemovalDetected { .. } => "episode.removal_detected",
            Self::EpisodeIdentityAmbiguous { .. } => "episode.identity_ambiguous",
            Self::EpisodeDuplicateResolved { .. } => "episode.duplicate_resolved",
            Self::FeedUrlChangeDetected { .. } => "feed.url.change_detected",
            Self::FeedUrlChanged { .. } => "feed.url.changed",
            Self::DownloadQueued { .. } => "download.queued",
            Self::DownloadStarted { .. } => "download.started",
            Self::DownloadProgress { .. } => "download.progress",
            Self::DownloadPaused { .. } => "download.paused",
            Self::DownloadResumed { .. } => "download.resumed",
            Self::DownloadRetryScheduled { .. } => "download.retry_scheduled",
            Self::DownloadCompleted { .. } => "download.completed",
            Self::DownloadFailed { .. } => "download.failed",
            Self::DownloadCancelled { .. } => "download.cancelled",
            Self::DownloadPausedAll { .. } => "download.paused_all",
            Self::DownloadResumedAll {} => "download.resumed_all",
            Self::ArchiveRegistered { .. } => "archive.registered",
            Self::ArchiveVerified { .. } => "archive.verified",
            Self::ArchiveMissing { .. } => "archive.missing",
            Self::ArchiveInvalid { .. } => "archive.invalid",
            Self::ArchiveRelocated { .. } => "archive.relocated",
            Self::ArchiveSourceChanged { .. } => "archive.source_changed",
            Self::ArchivePolicyQueued { .. } => "archive.policy_queued",
            Self::ArchivePolicySkipped { .. } => "archive.policy_skipped",
            Self::ArchiveSidecarWritten { .. } => "archive.sidecar.written",
            Self::ArchiveSidecarInvalid { .. } => "archive.sidecar.invalid",
            Self::ArchiveManifestWritten { .. } => "archive.manifest.written",
            Self::ArchiveManifestMismatch { .. } => "archive.manifest.mismatch",
            Self::ArchiveRebuildCompleted { .. } => "archive.rebuild.completed",
            Self::ArchiveImported { .. } => "archive.imported",
            Self::ArchiveImportSkipped { .. } => "archive.import.skipped",
            Self::ArchiveImportCompleted { .. } => "archive.import.completed",
            Self::ArchiveTagged { .. } => "archive.tagged",
            Self::ArchiveTagsSkipped { .. } => "archive.tags.skipped",
            Self::PodcastArtworkFetched { .. } => "podcast.artwork.fetched",
            Self::PodcastArtworkUnchanged { .. } => "podcast.artwork.unchanged",
            Self::PodcastArtworkFailed { .. } => "podcast.artwork.failed",
            Self::SchedulerTick { .. } => "scheduler.tick",
            Self::SchedulerPaused { .. } => "scheduler.paused",
            Self::SchedulerResumed {} => "scheduler.resumed",
            Self::SettingsChanged { .. } => "settings.changed",
            Self::SettingsRejected { .. } => "settings.rejected",
            Self::SearchReindexed { .. } => "search.reindexed",
        }
    }
}

/// An event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Event {
    /// Wire schema version.
    pub schema: u32,
    /// Identifier (time-ordered).
    pub id: EventId,
    /// When it happened.
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// Podcast the event is about, when any.
    pub podcast_id: Option<PodcastId>,
    /// Episode the event is about, when any.
    pub episode_id: Option<EpisodeId>,
    /// Kind and payload.
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    /// Current wire schema.
    pub const SCHEMA: u32 = 1;

    /// Creates an event that happened now.
    #[must_use]
    pub fn now(
        podcast_id: Option<PodcastId>,
        episode_id: Option<EpisodeId>,
        kind: EventKind,
    ) -> Self {
        Self {
            schema: Self::SCHEMA,
            id: EventId::new(),
            occurred_at: OffsetDateTime::now_utc(),
            podcast_id,
            episode_id,
            kind,
        }
    }

    /// The dotted event name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.kind.name()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn events_serialize_with_a_flat_kind_tag() {
        let p = PodcastId::new();
        let e = Event::now(
            Some(p),
            None,
            EventKind::PodcastMetadataUpdated {
                fields: vec!["title".into()],
            },
        );
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["kind"], "podcast.metadata.updated");
        assert_eq!(v["podcast_id"], p.to_string());
        assert_eq!(v["fields"][0], "title");
        assert!(v["episode_id"].is_null());
        let back: Event = serde_json::from_value(v).unwrap();
        assert_eq!(back, e);
        assert_eq!(e.name(), "podcast.metadata.updated");
    }

    /// Every kind's dotted name, straight from the derive.
    ///
    /// Serde generates the `kind` tag's vocabulary, and its unknown-variant
    /// error names every entry. Reading it back is what keeps the sample
    /// list honest: a variant added without a sample fails the test *by
    /// name*, instead of quietly going untested — which is how 15 of them
    /// were, until this replaced a hand-written list.
    fn names_serde_knows() -> BTreeSet<String> {
        let err = serde_json::from_value::<EventKind>(serde_json::json!({
            "kind": "no.such.kind"
        }))
        .unwrap_err()
        .to_string();
        // `unknown variant `x`, expected one of `a`, `b`, …`: the quoted
        // runs are every odd segment, and the first one is `x` itself.
        let names: BTreeSet<String> = err
            .split('`')
            .skip(1)
            .step_by(2)
            .skip(1)
            .map(str::to_owned)
            .collect();
        assert!(
            names.len() > 40,
            "serde's message no longer parses as expected: {err}"
        );
        names
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one literal per variant is the point
    fn every_kind_round_trips() {
        let job = JobId::new();
        let kinds = [
            EventKind::EpisodeUpdated { fields: vec![] },
            EventKind::EpisodeRemovalDetected { missing_streak: 2 },
            EventKind::EpisodeIdentityAmbiguous {
                duplicate_of: EpisodeId::new(),
                reasons: vec![],
            },
            EventKind::EpisodeDuplicateResolved {
                candidate: EpisodeId::new(),
                original: EpisodeId::new(),
                resolution: DuplicateResolution::Same,
            },
            EventKind::DownloadQueued {
                job_id: job,
                enclosure_url: Url::parse("https://cdn.example/a.mp3").unwrap(),
                priority: Priority::Normal,
                requeued: false,
            },
            EventKind::DownloadProgress {
                job_id: job,
                bytes_downloaded: 10,
                total_bytes: None,
                percentage: None,
                speed_bps: 1000,
                eta_secs: None,
            },
            EventKind::DownloadCompleted {
                job_id: job,
                path: "01/02.mp3".into(),
                size_bytes: 10,
                hash_algo: "sha256".into(),
                hash_value: "ab".into(),
                content_type: None,
                sniffed_type: None,
                attempts: 1,
                duration_ms: 5,
            },
            EventKind::DownloadPausedAll {
                reason: PauseAllReason::DiskFull,
            },
            EventKind::DownloadResumedAll {},
            EventKind::ArchiveRegistered {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/2024-01-01 - Ep.mp3".into(),
                size_bytes: 10,
                hash_algo: "sha256".into(),
                hash_value: "ab".into(),
            },
            EventKind::ArchiveVerified {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/2024-01-01 - Ep.mp3".into(),
                depth: VerifyDepth::Full,
                reason: "hash_match".into(),
            },
            EventKind::ArchiveMissing {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/gone.mp3".into(),
            },
            EventKind::ArchiveInvalid {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/changed.mp3".into(),
                depth: VerifyDepth::Light,
                reason: "size_mismatch".into(),
            },
            EventKind::ArchiveRelocated {
                archive_file_id: ArchiveFileId::new(),
                from: "01/02.mp3".into(),
                to: "Show/2024/2024-01-01 - Ep.mp3".into(),
            },
            EventKind::ArchiveSourceChanged {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/2024-01-01 - Ep.mp3".into(),
                old_url: "https://cdn.example/ep.mp3".into(),
                new_url: "https://cdn.example/ep-v2.mp3".into(),
                old_length: Some(1000),
                new_length: None,
            },
            EventKind::ArchivePolicyQueued {
                job_id: job,
                policy_version: 1,
                priority: Priority::Normal,
            },
            EventKind::ArchivePolicySkipped {
                policy_version: 1,
                reason: "too_old".into(),
            },
            EventKind::ArchiveSidecarWritten {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/2024-01-01 - Ep.mp3.json".into(),
            },
            EventKind::ArchiveSidecarInvalid {
                path: "Show/2024/broken.mp3.json".into(),
                reason: "unsupported_schema".into(),
            },
            EventKind::ArchiveManifestWritten {
                path: ".uguisu/manifests/01J/manifest.sha256".into(),
                entries: 12,
            },
            EventKind::ArchiveManifestMismatch {
                path: ".uguisu/manifests/01J/manifest.sha256".into(),
                changed: 1,
                missing: 2,
                added: 3,
            },
            EventKind::ArchiveRebuildCompleted {
                applied: false,
                scanned: 10,
                rebuilt: 4,
                unchanged: 5,
                conflicts: 1,
                unknown_episode: 0,
            },
            EventKind::ArchiveImported {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/2024-01-01 - Ep.mp3".into(),
                confidence: 100,
                matched_by: "content_hash".into(),
            },
            EventKind::ArchiveImportSkipped {
                state: "ambiguous".into(),
                reason: "two candidates within the margin".into(),
            },
            EventKind::ArchiveImportCompleted {
                applied: true,
                format: "podgrab".into(),
                scanned: 9,
                imported: 6,
                ambiguous: 1,
                unmatched: 1,
                already_present: 1,
            },
            EventKind::ArchiveTagged {
                archive_file_id: ArchiveFileId::new(),
                path: "Show/2024/2024-01-01 - Ep.mp3".into(),
                mode: TagMode::FillMissing,
                fields: vec!["title".into(), "album".into()],
                size_bytes: 1024,
                hash_value: "cd".into(),
            },
            EventKind::ArchiveTagsSkipped {
                archive_file_id: ArchiveFileId::new(),
                reason: "unsupported".into(),
            },
            EventKind::PodcastArtworkFetched {
                artwork_id: ArtworkId::new(),
                path: ".uguisu/artwork/01J/ab.jpg".into(),
                format: ArtworkFormat::Jpeg,
                size_bytes: 2048,
                hash_value: "ab".into(),
            },
            EventKind::PodcastArtworkUnchanged {
                artwork_id: ArtworkId::new(),
            },
            EventKind::PodcastArtworkFailed {
                url: Url::parse("https://cdn.example/cover.png").unwrap(),
                reason: "artwork_invalid".into(),
            },
            EventKind::PodcastAdded {
                title: "A Show".into(),
                feed_url: Url::parse("https://example.test/feed.xml").unwrap(),
                source_id: SourceId::new(),
            },
            EventKind::PodcastRemoved {
                title: "A Show".into(),
                feed_url: Some(Url::parse("https://example.test/feed.xml").unwrap()),
                episodes: 12,
                files: 3,
            },
            EventKind::FeedRefreshStarted {
                source_id: SourceId::new(),
                feed_url: Url::parse("https://example.test/feed.xml").unwrap(),
                conditional: true,
            },
            EventKind::FeedRefreshCompleted {
                source_id: SourceId::new(),
                fetch_id: FetchId::new(),
                episodes: EpisodeCounts {
                    seen: 3,
                    added: 1,
                    ..EpisodeCounts::default()
                },
                podcast_changed: false,
                truncated: false,
            },
            EventKind::FeedRefreshFailed {
                source_id: SourceId::new(),
                fetch_id: FetchId::new(),
                error_kind: FetchErrorKind::Timeout,
                detail: "no response in 60s".into(),
                http_status: None,
                consecutive_failures: 2,
            },
            EventKind::FeedNotModified {
                source_id: SourceId::new(),
                fetch_id: FetchId::new(),
                reason: NotModifiedReason::Http304,
            },
            EventKind::PodcastMetadataUpdated {
                fields: vec!["title".into()],
            },
            EventKind::EpisodeDiscovered {
                title: "Episode One".into(),
                identity_key: "guid:abc".into(),
                enclosure_url: Some(Url::parse("https://cdn.example/a.mp3").unwrap()),
                published_at: None,
            },
            EventKind::FeedUrlChangeDetected {
                source_id: SourceId::new(),
                announced: Url::parse("https://example.test/new.xml").unwrap(),
                via: ReplacementReason::NewFeedUrl,
                reason: "not verified yet".into(),
            },
            EventKind::FeedUrlChanged {
                from_source_id: SourceId::new(),
                to_source_id: SourceId::new(),
                from: Url::parse("https://example.test/feed.xml").unwrap(),
                to: Url::parse("https://example.test/new.xml").unwrap(),
                via: ReplacementReason::Redirect,
            },
            EventKind::DownloadStarted {
                job_id: job,
                attempt: 1,
                resumed_from: 0,
                total_bytes: Some(1024),
                url: Url::parse("https://cdn.example/a.mp3").unwrap(),
            },
            EventKind::DownloadPaused {
                job_id: job,
                reason: "user".into(),
                bytes_downloaded: 512,
            },
            EventKind::DownloadResumed { job_id: job },
            EventKind::DownloadRetryScheduled {
                job_id: job,
                attempt: 1,
                error_kind: DownloadErrorKind::Network,
                detail: "connection reset".into(),
                http_status: None,
                next_attempt_at: OffsetDateTime::UNIX_EPOCH,
            },
            EventKind::DownloadFailed {
                job_id: job,
                reason: "max_attempts".into(),
                error_kind: DownloadErrorKind::Timeout,
                detail: "gave up".into(),
                http_status: Some(504),
                attempts: 8,
            },
            EventKind::DownloadCancelled {
                job_id: job,
                bytes_downloaded: 4,
            },
            EventKind::SchedulerTick {
                due: 4,
                started: 4,
                sleep_ms: 60_000,
            },
            EventKind::SchedulerPaused {
                reason: "operator".into(),
            },
            EventKind::SchedulerResumed {},
            EventKind::SettingsChanged {
                key: "UGUISU_FEED_REFRESH_CONCURRENCY".into(),
                removed: false,
                restart_required: false,
            },
            EventKind::SettingsRejected {
                key: "UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY".into(),
                message: "must not exceed UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY (3)".into(),
            },
            EventKind::SearchReindexed {
                podcasts: 12,
                episodes: 3400,
                duration_ms: 950,
                full: true,
            },
        ];
        let covered: BTreeSet<String> = kinds.iter().map(|k| k.name().to_owned()).collect();
        assert_eq!(
            covered.len(),
            kinds.len(),
            "the same kind is sampled twice: {covered:?}"
        );
        let known = names_serde_knows();
        assert_eq!(
            known.difference(&covered).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "these kinds have no sample and are never round-tripped"
        );
        assert_eq!(
            covered.difference(&known).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "a sample names a kind serde does not know"
        );
        for k in kinds {
            assert!(k.name().contains('.'));
            let json = serde_json::to_value(&k).unwrap();
            assert_eq!(json["kind"], k.name());
            let back: EventKind = serde_json::from_value(json).unwrap();
            assert_eq!(back, k);
        }
    }

    #[test]
    fn archive_events_carry_their_record() {
        let id = ArchiveFileId::new();
        let e = Event::now(
            Some(PodcastId::new()),
            Some(EpisodeId::new()),
            EventKind::ArchiveInvalid {
                archive_file_id: id,
                path: "Show/2024/changed.mp3".into(),
                depth: VerifyDepth::Full,
                reason: "hash_mismatch".into(),
            },
        );
        assert!(
            !e.kind.is_transient(),
            "no archive event is live-only: each one is a fact worth keeping"
        );
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "archive.invalid");
        assert_eq!(v["archive_file_id"], id.to_string());
        assert_eq!(v["path"], "Show/2024/changed.mp3");
        assert_eq!(v["depth"], "full");
        assert_eq!(v["reason"], "hash_mismatch");
        let back: Event = serde_json::from_value(v).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn only_download_progress_is_transient() {
        let job = JobId::new();
        let e = Event::now(
            Some(PodcastId::new()),
            Some(EpisodeId::new()),
            EventKind::DownloadProgress {
                job_id: job,
                bytes_downloaded: 512,
                total_bytes: Some(1024),
                percentage: Some(50.0),
                speed_bps: 100,
                eta_secs: Some(5),
            },
        );
        assert!(e.kind.is_transient());
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "download.progress");
        assert_eq!(v["job_id"], job.to_string());
        assert_eq!(v["percentage"], 50.0);
        assert!(v.get("timestamp").is_none(), "occurred_at is the timestamp");
        assert!(
            !EventKind::DownloadResumed { job_id: job }.is_transient()
                && !EventKind::PodcastMetadataUpdated { fields: vec![] }.is_transient()
        );
    }
}
