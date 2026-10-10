//! Archive service: turning a finished transfer into an archive artifact,
//! checking that artifacts are still there and still correct, moving them
//! when the template changes, and repairing the record after a crash
//! (`docs/ARCHIVE_ENGINE.md`, ADR 0021).
//!
//! Two rules shape everything here.
//!
//! **Nothing is deleted to restore consistency.** A file whose record is
//! gone is reported, not removed; a record whose file is gone keeps the
//! hash, so the artifact can be recovered by putting the file back or by
//! downloading it again on request. Verification never writes to the file
//! it is checking.
//!
//! **The download's completion transaction is not touched.** Registration
//! happens *after* it, in its own transaction, and hashing and filesystem
//! work happen outside any transaction. A crash between the two leaves a
//! completed job without an archive record, which is exactly what the
//! next start registers.

use uguisu_archive::collision::{self, Holder, Occupancy};
use uguisu_archive::path::RelativePath;
use uguisu_archive::policy::{self, EffectivePolicy, Position};
use uguisu_archive::template::{Context, Template};
use uguisu_archive::verify::Expectation;
use uguisu_archive::{path as archive_path, verify as archive_verify};
use uguisu_core::archive::{
    ArchiveErrorKind, ArchiveFile, ArchiveOrigin, ArchivePolicy, PolicyDecision, TagState,
    VerificationState, VerifyDepth, policy_reason, reason,
};
use uguisu_core::download::DownloadState;
use uguisu_core::ids::{ArchiveFileId, EpisodeId, PodcastId};
use uguisu_core::model::{ArchiveState, Episode, Podcast};
use uguisu_core::page;
use uguisu_core::{Event, EventKind, UguisuError};
use uguisu_download::{DestinationRequest, DestinationResolver};
use uguisu_storage::archive_files::VerificationUpdate;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
pub use uguisu_storage::archive_files::ArchiveFilter;
use uguisu_storage::{archive_files, archive_policies, downloads, episodes, events, podcasts};

use crate::Engine;

/// How many artifacts one scan batch reads and commits. A scan is never
/// one transaction: a full verification of a large archive would otherwise
/// hold the writer for minutes and block every other command.
pub const SCAN_BATCH: u32 = 200;

/// One page of archive records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ArchivePage {
    /// The records.
    pub files: Vec<ArchiveFile>,
    /// Cursor for the next page (`None` when this is the last one).
    pub next_after: Option<ArchiveFileId>,
}

/// How long the archive watcher waits for the next event before deciding
/// the queue has gone quiet and writing the manifests it owes.
const MANIFEST_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(5);

/// What a verification run found, in one place.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VerifySummary {
    /// Artifacts checked.
    pub checked: u64,
    /// Artifacts found intact.
    pub verified: u64,
    /// Artifacts whose file is gone.
    pub missing: u64,
    /// Artifacts whose file is there but wrong.
    pub invalid: u64,
    /// Artifacts that could not be checked (permissions, I/O).
    pub unchecked: u64,
    /// How deep the run looked.
    pub depth: VerifyDepth,
}

impl VerifySummary {
    fn record(&mut self, state: VerificationState) {
        self.checked += 1;
        match state {
            VerificationState::Verified => self.verified += 1,
            VerificationState::Missing => self.missing += 1,
            VerificationState::Invalid => self.invalid += 1,
            VerificationState::Unchecked => self.unchecked += 1,
        }
    }

    /// Whether anything needs the user's attention.
    #[must_use]
    pub const fn has_problems(&self) -> bool {
        self.missing > 0 || self.invalid > 0
    }
}

/// One artifact with the verification that was just run on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VerifiedFile {
    /// The record as it now stands.
    pub file: ArchiveFile,
    /// What the check found.
    pub state: VerificationState,
    /// Why, from `uguisu_core::archive::reason`.
    pub reason: String,
    /// How deep the check looked.
    pub depth: VerifyDepth,
    /// Detail when the check could not complete.
    pub detail: Option<String>,
}

/// What the template would produce for an episode, without writing
/// anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PathPreview {
    /// The episode.
    pub episode_id: EpisodeId,
    /// The template that was rendered.
    pub template: String,
    /// The path the template produces, before collisions are considered.
    pub rendered: String,
    /// The path the episode would actually get, after collision handling.
    pub resolved: String,
    /// The path it currently has, when it is archived.
    pub current: Option<String>,
    /// Whether [`Engine::relocate`] would move the file.
    pub would_move: bool,
    /// The disambiguating suffix, when one was needed.
    pub suffix: Option<String>,
}

/// What a relocation did, or would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Relocation {
    /// The episode.
    pub episode_id: EpisodeId,
    /// Where the artifact was.
    pub from: String,
    /// Where it is now (or would be).
    pub to: String,
    /// `false` for a dry run, or when the path did not change.
    pub moved: bool,
}

/// What startup reconciliation repaired.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ArchiveReconcileReport {
    /// Tag writes that were in flight when the process stopped.
    #[serde(default)]
    pub tagging_settled: u64,
    /// Completed downloads that had no archive record and now do.
    pub registered: u64,
    /// Completed downloads whose file could not be registered.
    pub unregisterable: u64,
    /// Artifacts checked for existence.
    pub checked: u64,
    /// Artifacts whose file turned out to be gone.
    pub missing: u64,
    /// Artifacts whose file turned out not to be a file.
    pub invalid: u64,
}

/// Errors this service raises, all under [`UguisuError::Archive`].
pub(crate) fn archive_error(kind: ArchiveErrorKind, detail: impl Into<String>) -> UguisuError {
    UguisuError::Archive {
        kind,
        detail: detail.into(),
    }
}

/// The database's answer to "who holds this path", plus the disk's.
///
/// Both are needed: the database knows which episode owns a path, and the
/// filesystem knows about files Uguisu never wrote — an imported archive,
/// a leftover from an older layout, a user's own file. Neither is
/// overwritten.
pub(crate) struct DbAndDisk<'a> {
    pub(crate) owners: &'a std::collections::HashMap<String, EpisodeId>,
    pub(crate) root: &'a Path,
}

impl Occupancy for DbAndDisk<'_> {
    fn holder(&self, path: &RelativePath, episode: EpisodeId) -> Holder {
        if let Some(owner) = self.owners.get(path.as_str()) {
            return if *owner == episode {
                Holder::SameEpisode
            } else {
                Holder::OtherEpisode
            };
        }
        match archive_path::resolve(self.root, path) {
            // `symlink_metadata`: a dangling symlink is still something in
            // the way, and a link is never followed to decide occupancy.
            Ok(full) => match std::fs::symlink_metadata(&full) {
                Ok(_) => Holder::ForeignFile,
                Err(_) => Holder::Free,
            },
            Err(_) => Holder::ForeignFile,
        }
    }
}

impl Engine {
    /// The archive root: every path this service resolves stays under it.
    pub(crate) fn media_root(&self) -> Result<PathBuf, UguisuError> {
        self.config()
            .data
            .media_dir()
            .map_err(|e| UguisuError::Config(e.to_string()))
    }

    /// The configured template, parsed.
    pub(crate) fn template(&self) -> Result<Template, UguisuError> {
        Template::parse(&self.config().archive.template).map_err(|e| {
            archive_error(
                ArchiveErrorKind::TemplateInvalid,
                format!("{}: {e}", self.config().archive.template),
            )
        })
    }

    /// Records a finished download as an archive artifact.
    ///
    /// Idempotent: registering an episode that already has a record
    /// updates that record rather than creating a second one (the unique
    /// `episode_id` makes the database enforce it), so a retried
    /// registration after a crash converges instead of duplicating.
    ///
    /// The file is not moved: the path the download wrote is the path that
    /// is recorded. Moving to a newly configured template is
    /// [`relocate`](Self::relocate)'s job, and only on request.
    #[allow(clippy::too_many_lines)] // read, check, build and write one record
    pub async fn register_archive_file(
        &self,
        episode_id: EpisodeId,
    ) -> Result<ArchiveFile, UguisuError> {
        let mut reader = self.storage().reader().await?;
        let job = downloads::get_job_by_episode(&mut reader, episode_id)
            .await?
            .ok_or_else(|| {
                archive_error(
                    ArchiveErrorKind::ArchiveNotFound,
                    format!("episode {episode_id} has no download job"),
                )
            })?;
        let existing = archive_files::get_by_episode(&mut reader, episode_id).await?;
        drop(reader);

        if job.state != DownloadState::Completed {
            return Err(archive_error(
                ArchiveErrorKind::ArchiveNotFound,
                format!("episode {episode_id}: the download is {}", job.state),
            ));
        }
        let hash = job.hash_value.clone().ok_or_else(|| {
            archive_error(
                ArchiveErrorKind::ArchiveInvalid,
                format!("episode {episode_id}: the completed job recorded no hash"),
            )
        })?;
        let relative = RelativePath::parse(&job.target_path)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let root = self.media_root()?;
        let full = archive_path::resolve_checked(&root, &relative)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let meta = std::fs::metadata(&full).map_err(|e| {
            archive_error(
                ArchiveErrorKind::ArchiveMissing,
                format!("{}: {e}", relative.as_str()),
            )
        })?;
        // A redownload to a new path moves the record only off a path that
        // holds nothing: a file that came back there is the record's, and the
        // new one stays where it landed, unowned, for a person to decide
        // (ADR 0060). Only `NotFound` means gone.
        if let Some(old) = existing
            .as_ref()
            .filter(|f| f.relative_path != relative.as_str())
        {
            let back = RelativePath::parse(&old.relative_path)
                .ok()
                .and_then(|p| archive_path::resolve_checked(&root, &p).ok())
                .is_none_or(|at| {
                    !matches!(std::fs::symlink_metadata(at), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
                });
            if back {
                return Err(archive_error(
                    ArchiveErrorKind::PathCollision,
                    format!(
                        "{} is in place again; the download at {} is left for you",
                        old.relative_path,
                        relative.as_str()
                    ),
                ));
            }
        }

        let now = OffsetDateTime::now_utc();
        let file = ArchiveFile {
            // A re-registration keeps the record's identity and the time it
            // was first archived; everything else is taken from the job.
            id: existing.as_ref().map_or_else(ArchiveFileId::new, |f| f.id),
            episode_id,
            podcast_id: job.podcast_id,
            relative_path: relative.as_str().to_owned(),
            size_bytes: meta.len(),
            content_type: job.content_type.clone(),
            sniffed_type: job.sniffed_type.clone(),
            hash_algo: job.hash_algo.clone(),
            hash_value: hash.clone(),
            // Provenance: what the transfer produced. `hash_value` follows
            // the bytes from here on (a tag write moves it); this does not,
            // which is what later tells "Uguisu rewrote this file" apart
            // from "something else did".
            source_size_bytes: Some(meta.len()),
            source_hash_algo: Some(job.hash_algo.clone()),
            source_hash_value: Some(hash),
            mtime_unix: mtime_of(&meta),
            origin: ArchiveOrigin::Download,
            // Fresh bytes: untagged, and with no sidecar until one is
            // written for them.
            tag_state: TagState::Untagged,
            tag_mode: None,
            tagged_at: None,
            sidecar_written_at: None,
            original_tags: None,
            source_changed_at: None,
            // The bytes were hashed while they streamed in, so a fresh
            // download is trusted until something asks otherwise.
            verification_state: VerificationState::Unchecked,
            verification_reason: Some(reason::REGISTERED.to_owned()),
            verified_at: None,
            registered_at: existing.as_ref().map_or(now, |f| f.registered_at),
            created_at: existing.as_ref().map_or(now, |f| f.created_at),
            updated_at: now,
        };

        let event = Event::now(
            Some(job.podcast_id),
            Some(episode_id),
            EventKind::ArchiveRegistered {
                archive_file_id: file.id,
                path: file.relative_path.clone(),
                size_bytes: file.size_bytes,
                hash_algo: file.hash_algo.clone(),
                hash_value: file.hash_value.clone(),
            },
        );
        let mut tx = self.storage().begin().await?;
        archive_files::upsert(&mut tx, &file).await?;
        // The manifest is derived data: marking it stale here means the
        // flag and the fact it describes commit together, so a crash can
        // only leave "stale but actually fresh" and never the reverse.
        crate::archive_meta::mark_stale(&mut tx, file.podcast_id, now).await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        tracing::info!(episode = %episode_id, path = %file.relative_path, size = file.size_bytes, "archive file registered");
        Ok(file)
    }

    /// Registers a finished download and verifies it straight away, as the
    /// configuration asks.
    ///
    /// This is what runs after a download completes. It is deliberately
    /// *not* part of the completion transaction: hashing a large file
    /// inside the writer transaction would block every other command for
    /// as long as the hash takes.
    pub async fn archive_completed_download(
        &self,
        episode_id: EpisodeId,
    ) -> Result<VerifiedFile, UguisuError> {
        let file = self.register_archive_file(episode_id).await?;
        let cfg = &self.config().archive;
        let depth = if cfg.verify_on_completion {
            cfg.verify_depth
        } else {
            // Still confirm the file is where the record says, which costs
            // one stat and catches a finalization that did not land.
            VerifyDepth::Existence
        };
        let verified = self.verify_archive_file(&file, depth).await?;
        // The sidecar is written last and from the record, so it describes
        // bytes that have already been registered and checked. A sidecar
        // that could not be written is a degraded rebuild source, never a
        // failed download - the media file and its record are both
        // already durable by this point.
        if let Err(e) = self.write_sidecar(episode_id).await {
            tracing::warn!(episode = %episode_id, error = %e, "sidecar not written");
        }
        Ok(verified)
    }

    /// The artifact of one episode.
    pub async fn archive_file(
        &self,
        episode_id: EpisodeId,
    ) -> Result<Option<ArchiveFile>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(archive_files::get_by_episode(&mut reader, episode_id).await?)
    }

    /// A page of artifacts, newest first.
    ///
    /// `after` is the id of the last row of the previous page; its sort key is
    /// read here, so a caller never has to know the ordering.
    pub async fn archive_list(
        &self,
        filter: &ArchiveFilter,
        after: Option<ArchiveFileId>,
        limit: u32,
    ) -> Result<ArchivePage, UguisuError> {
        let limit = page::limit(Some(limit), page::DEFAULT, page::MAX)?;
        let mut reader = self.storage().reader().await?;
        let cursor = match after {
            None => None,
            Some(id) => {
                let row = archive_files::get(&mut reader, id)
                    .await?
                    .filter(|f| filter.state.is_none_or(|s| f.verification_state == s))
                    .filter(|f| filter.podcast_id.is_none_or(|p| f.podcast_id == p))
                    .ok_or_else(|| UguisuError::Invalid(format!("unknown cursor {id}")))?;
                Some((row.created_at, row.id))
            }
        };
        let rows =
            archive_files::list(&mut reader, filter, cursor, page::over_fetch(limit)).await?;
        let (files, next_after) = page::truncate(rows, limit, |f| f.id);
        Ok(ArchivePage { files, next_after })
    }

    /// How many artifacts are in each verification state.
    pub async fn archive_counts(
        &self,
    ) -> Result<std::collections::BTreeMap<VerificationState, u64>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(archive_files::count_by_state(&mut reader).await?)
    }

    /// Verifies one episode's artifact.
    pub async fn verify_episode(
        &self,
        episode_id: EpisodeId,
        depth: VerifyDepth,
    ) -> Result<VerifiedFile, UguisuError> {
        // A writer that changed the record between the read and the verdict
        // leaves the verdict unwritten; what was asked about is the record as
        // it is now, so it is read and checked again.
        let mut retries = 3;
        loop {
            let file = self.archive_file(episode_id).await?.ok_or_else(|| {
                archive_error(
                    ArchiveErrorKind::ArchiveNotFound,
                    format!("episode {episode_id} has no archive file"),
                )
            })?;
            match self.verify_archive_file(&file, depth).await {
                Err(UguisuError::Archive {
                    kind: ArchiveErrorKind::ArchiveNotFound,
                    ..
                }) if retries > 0 => retries -= 1,
                verdict => return verdict,
            }
        }
    }

    /// Verifies every artifact a filter selects, in batches.
    ///
    /// Each batch is read, checked outside any transaction and written
    /// back in its own transaction, so a scan of a large archive never
    /// holds the writer and can be interrupted without losing what it has
    /// already learned.
    pub async fn verify_all(
        &self,
        filter: &ArchiveFilter,
        depth: VerifyDepth,
    ) -> Result<VerifySummary, UguisuError> {
        let mut summary = VerifySummary {
            depth,
            ..VerifySummary::default()
        };
        let mut after: Option<(OffsetDateTime, ArchiveFileId)> = None;
        loop {
            let mut reader = self.storage().reader().await?;
            let batch = archive_files::list(&mut reader, filter, after, SCAN_BATCH).await?;
            drop(reader);
            if batch.is_empty() {
                break;
            }
            after = batch.last().map(|f| (f.created_at, f.id));
            let full = batch.len() >= SCAN_BATCH as usize;
            for file in &batch {
                // A record that vanished mid-scan is not a failure of the
                // scan: it is one fewer artifact, and the next batch
                // continues from the same cursor.
                match self.verify_archive_file(file, depth).await {
                    Ok(verified) => summary.record(verified.state),
                    Err(UguisuError::Archive {
                        kind: ArchiveErrorKind::ArchiveNotFound,
                        ..
                    }) => {}
                    Err(e) => return Err(e),
                }
            }
            if !full {
                break;
            }
        }
        tracing::info!(
            checked = summary.checked,
            verified = summary.verified,
            missing = summary.missing,
            invalid = summary.invalid,
            %depth,
            "archive verification finished"
        );
        Ok(summary)
    }

    /// Runs one check and records what it found.
    ///
    /// The record is updated, never removed, and the file is never
    /// touched. The episode's [`ArchiveState`] is projected from the
    /// outcome so the library view agrees with the archive without storing
    /// the same fact twice.
    async fn verify_archive_file(
        &self,
        file: &ArchiveFile,
        depth: VerifyDepth,
    ) -> Result<VerifiedFile, UguisuError> {
        let root = self.media_root()?;
        let expect = Expectation::from_record(file)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let outcome = archive_verify::verify(&root, &expect, depth);
        let now = OffsetDateTime::now_utc();

        // An existence pass that found the file writes nothing: it learned
        // only that the path is occupied, which is not enough to change
        // what the record says. Its job is to find files that vanished.
        // Neither may an inconclusive light pass clear a finding: a file
        // that failed its hash and was touched since is still unexplained.
        let inconclusive = outcome.state == VerificationState::Unchecked
            && (depth == VerifyDepth::Existence
                || file.verification_state == VerificationState::Invalid);
        if inconclusive {
            return Ok(VerifiedFile {
                file: file.clone(),
                state: file.verification_state,
                reason: outcome.reason.to_owned(),
                depth: outcome.depth,
                detail: outcome.detail,
            });
        }

        let event = match outcome.state {
            // Staying intact is no news; announcing it let one pass over a
            // large archive fill the bounded event log (ADR 0021, amended).
            VerificationState::Verified => (file.verification_state != VerificationState::Verified)
                .then(|| EventKind::ArchiveVerified {
                    archive_file_id: file.id,
                    path: file.relative_path.clone(),
                    depth: outcome.depth,
                    reason: outcome.reason.to_owned(),
                }),
            VerificationState::Missing => Some(EventKind::ArchiveMissing {
                archive_file_id: file.id,
                path: file.relative_path.clone(),
            }),
            VerificationState::Invalid => Some(EventKind::ArchiveInvalid {
                archive_file_id: file.id,
                path: file.relative_path.clone(),
                depth: outcome.depth,
                reason: outcome.reason.to_owned(),
            }),
            // "could not check" is not a finding: it says nothing about the
            // artifact, so it is logged rather than announced.
            VerificationState::Unchecked => None,
        }
        .map(|kind| Event::now(Some(file.podcast_id), Some(file.episode_id), kind));

        let mut tx = self.storage().begin().await?;
        let still_there = archive_files::set_verification(
            &mut tx,
            file,
            &VerificationUpdate {
                state: outcome.state,
                reason: Some(outcome.reason.to_owned()),
                at: now,
                // Only a pass that vouched for the bytes may move the mtime
                // a light pass compares against; otherwise the second light
                // pass after an edit would trust the size again.
                mtime_unix: outcome.mtime_unix.filter(|_| outcome.is_ok()),
            },
        )
        .await?;
        if !still_there {
            // The record was removed, moved or rewritten while the file was
            // being checked: the verdict is about a record that is no more,
            // and the next pass checks the one that is.
            tx.rollback()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            return Err(archive_error(
                ArchiveErrorKind::ArchiveNotFound,
                format!("archive record {} changed while it was checked", file.id),
            ));
        }
        if let Some(state) = projected_state(outcome.state) {
            episodes::set_archive_state(&mut tx, file.episode_id, state, None, now).await?;
        }
        if let Some(event) = &event {
            events::insert_all(&mut tx, std::slice::from_ref(event)).await?;
        }
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        if let Some(event) = &event {
            self.bus().publish(std::slice::from_ref(event));
        }
        if outcome.state.is_problem() {
            tracing::warn!(
                episode = %file.episode_id,
                path = %file.relative_path,
                state = %outcome.state,
                reason = outcome.reason,
                detail = ?outcome.detail,
                "archive verification found a problem"
            );
        }

        let mut updated = file.clone();
        updated.verification_state = outcome.state;
        updated.verification_reason = Some(outcome.reason.to_owned());
        updated.verified_at = Some(now);
        updated.updated_at = now;
        if let Some(mtime) = outcome.mtime_unix {
            updated.mtime_unix = Some(mtime);
        }
        Ok(VerifiedFile {
            file: updated,
            state: outcome.state,
            reason: outcome.reason.to_owned(),
            depth: outcome.depth,
            detail: outcome.detail,
        })
    }

    /// What the current template would produce for an episode.
    ///
    /// Creates nothing and reads no media: this is safe to call on a
    /// read-only archive, which is what makes it usable as a preview
    /// before a relocation.
    pub async fn path_preview(&self, episode_id: EpisodeId) -> Result<PathPreview, UguisuError> {
        let (podcast, episode, current) = self.archive_subject(episode_id).await?;
        let template = self.template()?;
        let root = self.media_root()?;
        let extension = current
            .as_ref()
            .and_then(|f| extension_of(&f.relative_path))
            .unwrap_or_else(|| "mp3".to_owned());

        let ctx = Context::new(&podcast, &episode, &extension);
        let profile = self.config().archive.path_profile;
        let rendered = template
            .render(&ctx, profile)
            .map_err(|e| archive_error(ArchiveErrorKind::TemplateInvalid, e.to_string()))?;

        let owners = self
            .path_owners(std::slice::from_ref(&rendered), episode_id)
            .await?;
        let placement = collision::place(
            &rendered,
            episode_id,
            profile,
            &DbAndDisk {
                owners: &owners,
                root: &root,
            },
        )
        .map_err(|e| archive_error(ArchiveErrorKind::PathCollision, e.to_string()))?;

        let current_path = current.as_ref().map(|f| f.relative_path.clone());
        Ok(PathPreview {
            episode_id,
            template: template.source().to_owned(),
            rendered: rendered.as_str().to_owned(),
            would_move: current_path
                .as_ref()
                .is_some_and(|c| c != placement.path.as_str()),
            resolved: placement.path.as_str().to_owned(),
            current: current_path,
            suffix: placement.suffix,
        })
    }

    /// Moves one episode's artifact to the path the current template
    /// produces.
    ///
    /// Only ever on request: a configuration change does not move files
    /// that are already archived. The move is a rename, so it either
    /// happened or it did not — there is no window in which the file
    /// exists at neither path. A target on another filesystem is refused
    /// rather than copied, because a copy would double the archive's size
    /// without saying so.
    pub async fn relocate(
        &self,
        episode_id: EpisodeId,
        dry_run: bool,
    ) -> Result<Relocation, UguisuError> {
        let preview = self.path_preview(episode_id).await?;
        let file = self.archive_file(episode_id).await?.ok_or_else(|| {
            archive_error(
                ArchiveErrorKind::ArchiveNotFound,
                format!("episode {episode_id} has no archive file"),
            )
        })?;
        let from = file.relative_path.clone();
        if from == preview.resolved {
            return Ok(Relocation {
                episode_id,
                from,
                to: preview.resolved,
                moved: false,
            });
        }
        if dry_run {
            return Ok(Relocation {
                episode_id,
                from,
                to: preview.resolved,
                moved: false,
            });
        }

        let root = self.media_root()?;
        let source = RelativePath::parse(&from)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let target = RelativePath::parse(&preview.resolved)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let (source_full, target_full, mtime) = move_file(&root, &source, &target)?;
        let now = OffsetDateTime::now_utc();
        let event = Event::now(
            Some(file.podcast_id),
            Some(episode_id),
            EventKind::ArchiveRelocated {
                archive_file_id: file.id,
                from: from.clone(),
                to: target.as_str().to_owned(),
            },
        );
        let mut tx = self.storage().begin().await?;
        // The unique index on `relative_path` is the real guard: if another
        // artifact took the path since the preview, this fails here and the
        // file is moved back below.
        let updated = archive_files::set_path(&mut tx, file.id, target.as_str(), mtime, now).await;
        let updated = match updated {
            Ok(v) => v,
            Err(e) => {
                let _ = uguisu_download::paths::rename_new(&target_full, &source_full);
                return Err(archive_error(
                    ArchiveErrorKind::PathCollision,
                    format!("{} could not be recorded: {e}", target.as_str()),
                ));
            }
        };
        if !updated {
            let _ = uguisu_download::paths::rename_new(&target_full, &source_full);
            return Err(archive_error(
                ArchiveErrorKind::ArchiveNotFound,
                format!("archive record {} disappeared while moving", file.id),
            ));
        }
        // The job that downloaded the file follows it, or a deep reconcile
        // would look at the old path and mark the episode missing.
        if let Err(e) = downloads::set_target_path(&mut tx, episode_id, target.as_str(), now).await
        {
            let _ = uguisu_download::paths::rename_new(&target_full, &source_full);
            return Err(e.into());
        }
        crate::archive_meta::mark_stale(&mut tx, file.podcast_id, now).await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        // The media moved first and its record followed; the sidecar is
        // rendered fresh at the new path. The one left behind at the old
        // path is reported by `archive orphans` as a stray sidecar; nothing
        // here deletes it.
        if let Err(e) = self.write_sidecar(episode_id).await {
            tracing::warn!(episode = %episode_id, error = %e, "sidecar not written after relocation");
        }
        tracing::info!(episode = %episode_id, %from, to = target.as_str(), "archive file relocated");
        Ok(Relocation {
            episode_id,
            from,
            to: target.as_str().to_owned(),
            moved: true,
        })
    }

    /// Repairs the archive record after an unclean stop, and reports files
    /// that are not where it says, as after a change of media root.
    ///
    /// Shallow (the default): the start's repair, then confirms that
    /// recorded files exist. It never hashes.
    ///
    /// Deep: the same, followed by a light verification of every artifact.
    /// Hashing stays an explicit `archive verify --full`.
    pub async fn reconcile_archive(
        &self,
        deep: bool,
    ) -> Result<ArchiveReconcileReport, UguisuError> {
        let mut report = ArchiveReconcileReport::default();
        self.repair(&mut report).await?;

        let depth = if deep {
            VerifyDepth::Light
        } else {
            VerifyDepth::Existence
        };
        let summary = self.verify_all(&ArchiveFilter::default(), depth).await?;
        report.checked = summary.checked;
        report.missing = summary.missing;
        report.invalid = summary.invalid;
        if report.registered > 0 || summary.has_problems() {
            tracing::warn!(
                registered = report.registered,
                unregisterable = report.unregisterable,
                missing = report.missing,
                invalid = report.invalid,
                "archive reconciled"
            );
        }
        Ok(report)
    }

    /// What every start runs: registers the completed downloads an unclean
    /// stop left without a record, and settles interrupted tag writes. Both
    /// are queries that find nothing on a healthy archive; no file is
    /// looked at, so a start does not grow with the archive (ADR 0021).
    pub(crate) async fn repair_archive(&self) -> Result<(), UguisuError> {
        let mut report = ArchiveReconcileReport::default();
        self.repair(&mut report).await?;
        if report.registered > 0 || report.unregisterable > 0 {
            tracing::warn!(
                registered = report.registered,
                unregisterable = report.unregisterable,
                "unrecorded downloads were archived at start"
            );
        }
        Ok(())
    }

    async fn repair(&self, report: &mut ArchiveReconcileReport) -> Result<(), UguisuError> {
        self.register_unarchived(report).await?;
        // Tag writes a crash interrupted. The query behind this is a
        // partial index that is empty whenever nothing was in flight, so
        // on a healthy archive it costs one lookup and hashes nothing.
        match self.recover_interrupted_tagging().await {
            Ok(0) => {}
            Ok(settled) => {
                report.tagging_settled = settled;
                tracing::warn!(settled, "interrupted tag writes were resolved");
            }
            Err(e) => tracing::warn!(error = %e, "interrupted tag writes could not be resolved"),
        }
        Ok(())
    }

    /// Registers every completed download that has no archive record yet.
    async fn register_unarchived(
        &self,
        report: &mut ArchiveReconcileReport,
    ) -> Result<(), UguisuError> {
        loop {
            let mut reader = self.storage().reader().await?;
            let pending = archive_files::completed_unregistered(&mut reader, SCAN_BATCH).await?;
            drop(reader);
            if pending.is_empty() {
                return Ok(());
            }
            let count = pending.len();
            let before = report.registered;
            for episode_id in pending {
                match self.register_archive_file(episode_id).await {
                    Ok(_) => report.registered += 1,
                    Err(e) => {
                        report.unregisterable += 1;
                        // Not fatal and never destructive: the job stays
                        // completed and the file, wherever it is, stays.
                        tracing::warn!(episode = %episode_id, error = %e, "completed download could not be registered");
                    }
                }
            }
            // A batch that registered nothing would come back unchanged.
            if count < SCAN_BATCH as usize || report.registered == before {
                return Ok(());
            }
        }
    }

    /// The podcast, episode and current record an archive command works on.
    pub(crate) async fn archive_subject(
        &self,
        episode_id: EpisodeId,
    ) -> Result<(Podcast, Episode, Option<ArchiveFile>), UguisuError> {
        let mut reader = self.storage().reader().await?;
        let episode = episodes::get(&mut reader, episode_id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "episode".to_owned(),
                id: episode_id.to_string(),
            })?;
        let podcast = podcasts::get(&mut reader, episode.podcast_id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: episode.podcast_id.to_string(),
            })?;
        let file = archive_files::get_by_episode(&mut reader, episode_id).await?;
        Ok((podcast, episode, file))
    }

    /// Which episode owns each of the candidate paths, and the path the
    /// suffixed form would take.
    pub(crate) async fn path_owners(
        &self,
        candidates: &[RelativePath],
        episode_id: EpisodeId,
    ) -> Result<std::collections::HashMap<String, EpisodeId>, UguisuError> {
        let profile = self.config().archive.path_profile;
        let suffix = collision::suffix_for(episode_id);
        let mut out = std::collections::HashMap::new();
        let mut reader = self.storage().reader().await?;
        for candidate in candidates {
            let mut paths = vec![candidate.clone()];
            if let Ok(suffixed) = collision::with_suffix(candidate, &suffix, profile) {
                paths.push(suffixed);
            }
            for path in paths {
                if let Some(owner) =
                    archive_files::owner_of_path(&mut reader, path.as_str()).await?
                {
                    out.insert(path.as_str().to_owned(), owner);
                }
            }
        }
        Ok(out)
    }
}

/// Moves one artifact inside the archive, creating the target's parents.
///
/// The move never replaces a file (`paths::rename_new`), and a crash leaves
/// the file at the new path, the old one, or briefly both, never neither. A
/// target on another filesystem is refused rather than copied — a copy would
/// double the archive's size without saying so, and would leave two files
/// that both look authoritative.
///
/// Returns the two absolute paths and the moved file's modification time.
fn move_file(
    root: &Path,
    source: &RelativePath,
    target: &RelativePath,
) -> Result<(PathBuf, PathBuf, Option<i64>), UguisuError> {
    let source_full = archive_path::resolve_checked(root, source)
        .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
    let target_full = archive_path::resolve_checked(root, target)
        .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
    if !source_full.is_file() {
        return Err(archive_error(
            ArchiveErrorKind::ArchiveMissing,
            format!("{source} is not there to move"),
        ));
    }
    // Refuse rather than overwrite: the collision rule already chose a free
    // path, so anything sitting here appeared in between.
    if std::fs::symlink_metadata(&target_full).is_ok() {
        return Err(archive_error(
            ArchiveErrorKind::PathCollision,
            format!("{target} is already taken"),
        ));
    }
    if let Some(parent) = target_full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            archive_error(
                ArchiveErrorKind::RelocationFailed,
                format!("cannot create {}: {e}", parent.display()),
            )
        })?;
    }
    uguisu_download::paths::rename_new(&source_full, &target_full).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            return archive_error(
                ArchiveErrorKind::PathCollision,
                format!("{target} is already taken"),
            );
        }
        let detail = if e.raw_os_error() == Some(CROSS_DEVICE) {
            format!(
                "{source} and {target} are on different filesystems; \
                 Uguisu moves files, it does not copy them"
            )
        } else {
            format!("{source} -> {target}: {e}")
        };
        archive_error(ArchiveErrorKind::RelocationFailed, detail)
    })?;
    sync_dir(target_full.parent());
    let mtime = std::fs::metadata(&target_full)
        .ok()
        .and_then(|m| mtime_of(&m));
    Ok((source_full, target_full, mtime))
}

/// `EXDEV`: the rename crossed a filesystem boundary.
const CROSS_DEVICE: i32 = 18;

/// The episode state a verification outcome implies.
///
/// A file that could not be checked leaves the episode alone: saying
/// nothing is better than saying something wrong.
const fn projected_state(state: VerificationState) -> Option<ArchiveState> {
    match state {
        VerificationState::Verified => Some(ArchiveState::Archived),
        VerificationState::Missing => Some(ArchiveState::Missing),
        VerificationState::Invalid => Some(ArchiveState::Modified),
        VerificationState::Unchecked => None,
    }
}

/// The extension of a stored path, without the dot.
fn extension_of(relative_path: &str) -> Option<String> {
    let name = relative_path.rsplit('/').next()?;
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => Some(ext.to_owned()),
        _ => None,
    }
}

/// Modification time in whole seconds since the epoch.
fn mtime_of(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

/// Flushes a directory entry so the rename survives a power loss.
///
/// Best effort: Windows has no directory handle to sync, and a filesystem
/// that refuses the call has already made its own durability promise.
pub(crate) fn sync_dir(dir: Option<&Path>) {
    #[cfg(unix)]
    if let Some(dir) = dir
        && let Ok(handle) = std::fs::File::open(dir)
    {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

impl Engine {
    /// Starts the task that archives finished downloads (idempotent).
    ///
    /// It listens on the event bus rather than hooking into the download
    /// worker, so the completion transaction keeps its exact
    /// shape and the archive work — a `stat`, possibly a hash — happens
    /// outside it. If the process dies before the watcher gets to an
    /// event, the next start registers the job; nothing is lost, it is
    /// only late. If the
    /// watcher falls behind the bounded bus and skips events, it registers
    /// every unarchived download itself once the bus is quiet.
    pub fn start_archive_watcher(&self) {
        let mut slot = self
            .inner
            .archive_watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
        let engine = self.clone();
        let cancel = self.inner.shutdown.clone();
        let mut subscription = self.subscribe();
        *slot = Some(tokio::spawn(async move {
            // Manifests are written when the work stops, not per episode:
            // five hundred downloads of one show would otherwise mean five
            // hundred rewrites of a growing file. `dirty` is what keeps an
            // idle process idle - with nothing to flush, the loop blocks on
            // the bus instead of waking on a timer.
            let mut dirty = false;
            // A skipped event may have been a completion, so a lag is
            // answered with the same sweep a start runs, once it is quiet.
            let mut skipped = false;
            loop {
                let received = if dirty {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        event = tokio::time::timeout(MANIFEST_DEBOUNCE, subscription.recv()) => event,
                    }
                } else {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        event = subscription.recv() => Ok(event),
                    }
                };
                if subscription.take_skipped() > 0 {
                    skipped = true;
                    dirty = true;
                }
                let Ok(event) = received else {
                    // Nothing has happened for a while: the queue has gone
                    // quiet, so this is the cheap moment to write.
                    if std::mem::take(&mut skipped) {
                        let mut report = ArchiveReconcileReport::default();
                        if let Err(e) = engine.register_unarchived(&mut report).await {
                            tracing::warn!(error = %e, "skipped downloads could not be archived");
                        } else {
                            tracing::info!(
                                registered = report.registered,
                                "archived what the watcher skipped"
                            );
                        }
                    }
                    engine.flush_archive_metadata().await;
                    dirty = false;
                    continue;
                };
                let Some(event) = event else { break };
                if !matches!(event.kind, EventKind::DownloadCompleted { .. }) {
                    continue;
                }
                let Some(episode_id) = event.episode_id else {
                    continue;
                };
                if let Err(e) = engine.archive_completed_download(episode_id).await {
                    // Never fatal: the download itself succeeded and the
                    // bytes are on disk. Reconciliation retries.
                    tracing::warn!(episode = %episode_id, error = %e, "finished download could not be archived");
                }
                dirty = true;
            }
            tracing::debug!("archive watcher stopped");
        }));
    }
}

impl Engine {
    /// Starts confirming that every recorded file still exists, as a server
    /// does once it is up (idempotent).
    ///
    /// A `stat` per file: about 12 s for 100 000 files on NTFS, which a
    /// start must not wait for (ADR 0021). It finds what a changed media
    /// root or a deleted file did while nothing was running; a file that
    /// is there is not written to.
    pub fn start_archive_check(&self) {
        let mut slot = self
            .inner
            .archive_check
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
        let engine = self.clone();
        let cancel = self.inner.shutdown.clone();
        *slot = Some(tokio::spawn(async move {
            let everything = ArchiveFilter::default();
            // Dropping the pass at an await rolls back the write in flight.
            let summary = tokio::select! {
                () = cancel.cancelled() => return,
                summary = engine.verify_all(&everything, VerifyDepth::Existence) => summary,
            };
            match summary {
                Ok(s) if s.has_problems() => tracing::warn!(
                    missing = s.missing,
                    invalid = s.invalid,
                    "the archive check found files that are not where their records say"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "the archive check could not finish"),
            }
        }));
    }

    /// Stops waiting for the archive check, bounded by `grace`.
    pub(crate) async fn stop_archive_check(&self, grace: std::time::Duration) {
        let task = self
            .inner
            .archive_check
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(task) = task else { return };
        if tokio::time::timeout(grace, task).await.is_err() {
            tracing::warn!("archive check did not stop in time");
        }
    }
}

/// Names a finished download's path from the configured archive template
/// (ADR 0022).
///
/// Held by the download service, which knows nothing about templates: it
/// asks, checks the answer against the media root and the other jobs, and
/// keeps the identifier layout when anything is off. That is why
/// this resolver may return `None` freely — a template that cannot be
/// rendered is reported, never fatal, and never blocks a download.
#[derive(Debug)]
pub struct TemplateDestinations {
    template: Option<Template>,
    profile: uguisu_core::archive::PathProfile,
}

impl TemplateDestinations {
    /// Parses the configured template once. An unparseable template is
    /// logged here and turns every answer into the identifier layout,
    /// rather than failing each enqueue in turn.
    #[must_use]
    pub fn new(config: &uguisu_core::config::ArchiveConfig) -> Self {
        let template = match Template::parse(&config.template) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::error!(template = %config.template, error = %e, "archive template is not usable; downloads keep the identifier layout");
                None
            }
        };
        Self {
            template,
            profile: config.path_profile,
        }
    }
}

impl DestinationResolver for TemplateDestinations {
    fn target(&self, request: &DestinationRequest<'_>) -> Option<String> {
        let template = self.template.as_ref()?;
        let ctx = Context::new(request.podcast, request.episode, request.extension);
        match template.render(&ctx, self.profile) {
            Ok(path) => Some(path.as_str().to_owned()),
            Err(e) => {
                tracing::warn!(episode = %request.episode.id, error = %e, "template did not render; keeping the identifier layout");
                None
            }
        }
    }
}

/// Version of the policy vocabulary carried on every policy event, so a
/// consumer can tell the rules of one release from another's.
pub const POLICY_VERSION: u32 = 1;

/// What one policy evaluation did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PolicyOutcome {
    /// Episodes the policy queued.
    pub queued: u64,
    /// Episodes it left alone.
    pub skipped: u64,
    /// How many were skipped for each reason.
    pub reasons: std::collections::BTreeMap<String, u64>,
}

impl Engine {
    /// The policy in force for one podcast.
    pub async fn effective_policy(
        &self,
        podcast_id: PodcastId,
    ) -> Result<EffectivePolicy, UguisuError> {
        let mut reader = self.storage().reader().await?;
        let stored = archive_policies::get(&mut reader, podcast_id).await?;
        Ok(EffectivePolicy::resolve(
            &self.config().archive,
            stored.as_ref(),
        ))
    }

    /// Stores (or replaces) one podcast's policy.
    pub async fn set_policy(&self, policy: &ArchivePolicy) -> Result<(), UguisuError> {
        let mut tx = self.storage().begin().await?;
        archive_policies::set(&mut tx, policy).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        Ok(())
    }

    /// Removes one podcast's policy, so the global defaults apply again.
    pub async fn clear_policy(&self, podcast_id: PodcastId) -> Result<bool, UguisuError> {
        let mut tx = self.storage().begin().await?;
        let removed = archive_policies::delete(&mut tx, podcast_id).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        Ok(removed)
    }

    /// Every stored per-podcast policy.
    pub async fn policies(&self) -> Result<Vec<ArchivePolicy>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(archive_policies::list(&mut reader).await?)
    }

    /// Runs the archive policy over episodes a refresh just discovered.
    ///
    /// Off unless it is turned on. The decision is
    /// [`uguisu_archive::policy::decide`]'s, a pure function; the only
    /// thing done here is asking the queue to take what it chose, through
    /// the same `enqueue_episode` a user command uses. The queue's unique
    /// `episode_id` is the real boundary, so re-running this over the same
    /// episodes cannot produce a second job.
    ///
    /// Episodes are considered newest first, so a backlog limit admits the
    /// newest ones rather than whichever came back from the database
    /// first. The limit counts everything already outstanding for the
    /// podcast, not only what this run queued — otherwise every refresh
    /// would top the queue up again and the limit would mean nothing.
    pub async fn apply_policy(
        &self,
        podcast_id: PodcastId,
        discovered: &[EpisodeId],
    ) -> Result<PolicyOutcome, UguisuError> {
        let mut outcome = PolicyOutcome::default();
        if discovered.is_empty() {
            return Ok(outcome);
        }
        let policy = self.effective_policy(podcast_id).await?;

        let mut reader = self.storage().reader().await?;
        let mut episodes = Vec::with_capacity(discovered.len());
        for id in discovered {
            if let Some(episode) = episodes::get(&mut reader, *id).await? {
                episodes.push(episode);
            }
        }
        drop(reader);
        episodes.sort_by(|a, b| b.sort_at.cmp(&a.sort_at).then(b.id.cmp(&a.id)));

        let now = OffsetDateTime::now_utc();
        let mut events = Vec::new();
        let outstanding = {
            let mut reader = self.storage().reader().await?;
            downloads::outstanding_for_podcast(&mut reader, podcast_id).await?
        };
        for episode in &episodes {
            let position = self
                .policy_position(episode, outstanding + outcome.queued)
                .await?;
            match policy::decide(&policy, episode, position, now) {
                PolicyDecision::Queue { priority } => {
                    match self.downloads().enqueue_episode(episode.id, priority).await {
                        Ok(enqueued) => {
                            outcome.queued += 1;
                            events.push(Event::now(
                                Some(podcast_id),
                                Some(episode.id),
                                EventKind::ArchivePolicyQueued {
                                    job_id: enqueued.job().id,
                                    policy_version: POLICY_VERSION,
                                    priority,
                                },
                            ));
                        }
                        Err(e) => {
                            // A policy must never be able to fail a refresh:
                            // the feed is already stored, and the episode
                            // will be considered again next time.
                            tracing::warn!(episode = %episode.id, error = %e, "policy could not queue an episode");
                            outcome.skipped += 1;
                            *outcome
                                .reasons
                                .entry("enqueue_failed".to_owned())
                                .or_default() += 1;
                        }
                    }
                }
                PolicyDecision::Skip { reason } => {
                    outcome.skipped += 1;
                    *outcome.reasons.entry(reason.clone()).or_default() += 1;
                    // `disabled` is the normal state of a default
                    // installation: announcing it for every episode of
                    // every refresh would be noise, not information.
                    if reason != policy_reason::DISABLED {
                        events.push(Event::now(
                            Some(podcast_id),
                            Some(episode.id),
                            EventKind::ArchivePolicySkipped {
                                policy_version: POLICY_VERSION,
                                reason,
                            },
                        ));
                    }
                }
            }
        }

        if !events.is_empty() {
            let mut tx = self.storage().begin().await?;
            events::insert_all(&mut tx, &events).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            self.bus().publish(&events);
        }
        if outcome.queued > 0 {
            tracing::info!(
                podcast = %podcast_id,
                queued = outcome.queued,
                skipped = outcome.skipped,
                "archive policy queued episodes"
            );
        }
        Ok(outcome)
    }

    /// What the queue and the archive already know about an episode.
    ///
    /// `outstanding` is how many of the podcast's episodes are already
    /// waiting to be archived, this evaluation's included.
    async fn policy_position(
        &self,
        episode: &Episode,
        outstanding: u64,
    ) -> Result<Position, UguisuError> {
        let mut reader = self.storage().reader().await?;
        let has_job = downloads::get_job_by_episode(&mut reader, episode.id)
            .await?
            .is_some();
        let has_archive_file = archive_files::get_by_episode(&mut reader, episode.id)
            .await?
            .is_some();
        Ok(Position {
            has_job,
            has_archive_file,
            queued_so_far: u32::try_from(outstanding).unwrap_or(u32::MAX),
        })
    }
}
