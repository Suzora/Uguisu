//! `archive reconcile --rebuild`: putting the index back together from what is
//! on disk (ADR 0025).
//!
//! The database is an index over the archive, not the archive itself. Lose it
//! and the media files and their sidecars are still there; this reads them
//! back into records.
//!
//! The rule that shapes everything here: **a sidecar is metadata, not
//! evidence.** It says what Uguisu knew when it was written, not what the
//! bytes are now. So a rebuilt record is always
//! [`VerificationState::Unchecked`] with reason [`reason::REBUILT`], only a
//! real `archive verify` may write `verified`, and a record that already says
//! `verified` is never overwritten by a document claiming otherwise — that is
//! a conflict, and it is reported.
//!
//! Whatever a sidecar says, a rebuild will not invent an episode, download
//! anything, move or delete a file, or overwrite an unrelated record.
//! Everything it cannot resolve is counted and named, up to a bound.
//!
//! Resolution goes through two doors, because after a total loss the
//! identifiers in a sidecar no longer exist: podcast by identifier, else by
//! the feed URL the document records; episode by identifier, else by its
//! stable identity key.

use time::OffsetDateTime;
use uguisu_archive::layout;
use uguisu_archive::manifest::{Findings, MAX_SAMPLE};
use uguisu_archive::path::{RelativePath, resolve_checked};
use uguisu_archive::scan::{self, ScanOptions};
use uguisu_archive::sidecar;
use uguisu_core::UguisuError;
use uguisu_core::archive::{
    ArchiveErrorKind, ArchiveFile, ArchiveOrigin, Sidecar, VerificationState, reason,
};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::{ArchiveFileId, EpisodeId, PodcastId};
use uguisu_core::model::{Episode, Podcast};
use uguisu_storage::{archive_files, episodes, events, podcasts, sources};

use crate::Engine;
use crate::archive::archive_error;

/// Records written per transaction.
const REBUILD_BATCH: usize = 200;

/// What a rebuild was asked to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebuildOptions {
    /// Whether records are written. The default is a dry run, because a
    /// rebuild reads a tree Uguisu did not necessarily write.
    pub apply: bool,
    /// Restrict to one podcast's documents.
    pub podcast: Option<PodcastId>,
}

/// What a rebuild found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebuildReport {
    /// Whether records were written.
    pub applied: bool,
    /// Sidecars read.
    pub scanned: u64,
    /// Records that would be, or were, restored.
    pub rebuilt: u64,
    /// Records that already agree with their document.
    pub unchanged: u64,
    /// Documents that contradict a record Uguisu has checked, or each
    /// other. Never resolved by guessing.
    pub conflicts: Findings,
    /// Documents naming an episode this library does not have.
    pub unknown_episode: Findings,
    /// Documents that could not be read.
    pub malformed: Findings,
    /// Documents whose media file is not there.
    pub missing_media: Findings,
    /// Entries the scan could not look at (a link, an unreadable
    /// directory).
    pub unreadable: Findings,
}

impl RebuildReport {
    /// Whether anything needs a person to look at it.
    #[must_use]
    pub const fn has_findings(&self) -> bool {
        !self.conflicts.is_empty()
            || !self.unknown_episode.is_empty()
            || !self.malformed.is_empty()
            || !self.missing_media.is_empty()
            || !self.unreadable.is_empty()
    }
}

/// A finding group that counts everything and names the first few.
pub(crate) fn note(findings: &mut Findings, path: &str) {
    findings.count += 1;
    if findings.sample.len() < MAX_SAMPLE {
        findings.sample.push(path.to_owned());
    }
}

/// One record a rebuild is ready to write.
struct Pending {
    file: ArchiveFile,
    podcast_id: PodcastId,
    episode_id: EpisodeId,
}

impl Engine {
    /// Reconstructs archive records from the sidecars on disk.
    ///
    /// Streams the documents, so the memory it needs does not grow with
    /// the archive, and writes in batches so a long rebuild never holds
    /// the single writer connection.
    pub async fn rebuild_archive(
        &self,
        options: &RebuildOptions,
    ) -> Result<RebuildReport, UguisuError> {
        let root = self.media_root()?;
        let mut report = RebuildReport {
            applied: options.apply,
            ..RebuildReport::default()
        };
        let scan_options = ScanOptions::sidecars();
        let mut pending: Vec<Pending> = Vec::new();

        // `walkdir` is a blocking iterator; collecting the paths first
        // would defeat the point, so the whole pass reads one entry, then
        // awaits the database for it. The scan itself holds only its own
        // stack.
        let entries = scan::walk(&root, &scan_options);
        for entry in entries {
            let file = match entry {
                Ok(f) => f,
                Err(e) => {
                    note(&mut report.unreadable, &e.path().to_string_lossy());
                    continue;
                }
            };
            if !layout::is_sidecar_path(&file.relative) {
                continue;
            }
            report.scanned += 1;

            let document = match sidecar::read(&root, &file.relative) {
                Ok(Some(d)) => d,
                Ok(None) => continue,
                Err(e) => {
                    note(&mut report.malformed, file.relative.as_str());
                    tracing::debug!(path = %file.relative, error = %e, "sidecar not readable");
                    continue;
                }
            };
            match self
                .plan_rebuild(&root, &file.relative, &document, options, &mut report)
                .await?
            {
                Some(item) => pending.push(item),
                None => continue,
            }
            if pending.len() >= REBUILD_BATCH {
                self.commit_rebuild(&mut pending, options, &mut report)
                    .await?;
            }
        }
        self.commit_rebuild(&mut pending, options, &mut report)
            .await?;

        let event = Event::now(
            options.podcast,
            None,
            EventKind::ArchiveRebuildCompleted {
                applied: report.applied,
                scanned: report.scanned,
                rebuilt: report.rebuilt,
                unchanged: report.unchanged,
                conflicts: report.conflicts.count,
                unknown_episode: report.unknown_episode.count,
            },
        );
        let mut tx = self.storage().begin().await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        Ok(report)
    }

    /// Decides what one document means, without writing anything.
    async fn plan_rebuild(
        &self,
        root: &std::path::Path,
        sidecar_path: &RelativePath,
        document: &Sidecar,
        options: &RebuildOptions,
        report: &mut RebuildReport,
    ) -> Result<Option<Pending>, UguisuError> {
        let Some(media) = layout::media_of(sidecar_path) else {
            note(&mut report.malformed, sidecar_path.as_str());
            return Ok(None);
        };
        // The document's own idea of where it lives is not trusted: the
        // sidecar's location on disk is, because that is where it actually
        // is. A path inside the document only has to be usable at all.
        if RelativePath::parse(&document.archive.relative_path).is_err() {
            note(&mut report.malformed, sidecar_path.as_str());
            return Ok(None);
        }
        let full = resolve_checked(root, &media)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        if !full.is_file() {
            // A document with no media is reported, never turned into a
            // record: a record whose file does not exist is exactly the
            // lie a rebuild exists to avoid.
            note(&mut report.missing_media, sidecar_path.as_str());
            return Ok(None);
        }

        let Some((podcast, episode)) = self.resolve_subject(document).await? else {
            note(&mut report.unknown_episode, sidecar_path.as_str());
            return Ok(None);
        };
        if options.podcast.is_some_and(|wanted| wanted != podcast.id) {
            report.scanned -= 1;
            return Ok(None);
        }

        let mut reader = self.storage().reader().await?;
        let existing = archive_files::get_by_episode(&mut reader, episode.id).await?;
        let owner = archive_files::owner_of_path(&mut reader, media.as_str()).await?;
        drop(reader);

        if owner.is_some_and(|other| other != episode.id) {
            // Another episode already owns this path. Two documents
            // claiming one file, or one file claiming another's place:
            // either way a rebuild does not get to choose.
            note(&mut report.conflicts, sidecar_path.as_str());
            return Ok(None);
        }
        if let Some(existing) = &existing {
            if existing.relative_path == media.as_str()
                && existing.hash_value == document.archive.hash_value
            {
                report.unchanged += 1;
                return Ok(None);
            }
            if existing.verification_state == VerificationState::Verified {
                // The record was checked against the bytes; the document
                // was not. A checked finding is never overwritten by a
                // claim.
                note(&mut report.conflicts, sidecar_path.as_str());
                return Ok(None);
            }
        }

        let now = OffsetDateTime::now_utc();
        let file = ArchiveFile {
            id: existing.as_ref().map_or_else(ArchiveFileId::new, |f| f.id),
            episode_id: episode.id,
            podcast_id: podcast.id,
            relative_path: media.as_str().to_owned(),
            size_bytes: document.archive.size_bytes,
            content_type: document.archive.content_type.clone(),
            sniffed_type: document.archive.sniffed_type.clone(),
            hash_algo: document.archive.hash_algo.clone(),
            hash_value: document.archive.hash_value.clone(),
            source_size_bytes: document.source.as_ref().map(|s| s.size_bytes),
            source_hash_algo: document.source.as_ref().map(|s| s.hash_algo.clone()),
            source_hash_value: document.source.as_ref().map(|s| s.hash_value.clone()),
            mtime_unix: None,
            // Never `verified`: the document said so, nobody read the file.
            verification_state: VerificationState::Unchecked,
            verification_reason: Some(reason::REBUILT.to_owned()),
            verified_at: None,
            origin: ArchiveOrigin::Rebuild,
            tag_state: document.archive.tag_state,
            tag_mode: document.archive.tag_mode,
            tagged_at: document.archive.tagged_at,
            // The document is on disk and describes this record, so the
            // restored row starts out with a current sidecar.
            sidecar_written_at: Some(document.written_at),
            original_tags: document.archive.original_tags.clone(),
            source_changed_at: None,
            registered_at: existing
                .as_ref()
                .map_or(document.archive.registered_at, |f| f.registered_at),
            created_at: existing.as_ref().map_or(now, |f| f.created_at),
            updated_at: now,
        };
        report.rebuilt += 1;
        Ok(Some(Pending {
            file,
            podcast_id: podcast.id,
            episode_id: episode.id,
        }))
    }

    /// Finds the podcast and episode a document is about.
    ///
    /// Identifier first, then the durable keys: a library rebuilt from
    /// scratch has new identifiers everywhere, and the feed URL and the
    /// episode's identity key are what survive that.
    async fn resolve_subject(
        &self,
        document: &Sidecar,
    ) -> Result<Option<(Podcast, Episode)>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        let podcast = match podcasts::get(&mut reader, document.podcast.id).await? {
            Some(p) => Some(p),
            None => match &document.podcast.feed_url {
                Some(url) => {
                    let found =
                        sources::find_current_by_urls(&mut reader, &[url.to_string()]).await?;
                    match found.as_slice() {
                        [source] => podcasts::get(&mut reader, source.podcast_id).await?,
                        // No source, or more than one: nothing to choose
                        // between, so nothing is chosen.
                        _ => None,
                    }
                }
                None => None,
            },
        };
        let Some(podcast) = podcast else {
            return Ok(None);
        };
        let episode = match episodes::get(&mut reader, document.episode.id).await? {
            Some(e) if e.podcast_id == podcast.id => Some(e),
            _ => {
                episodes::find_by_identity(&mut reader, podcast.id, &document.episode.identity_key)
                    .await?
            }
        };
        Ok(episode.map(|e| (podcast, e)))
    }

    /// Writes a batch of restored records, or drops it on a dry run.
    async fn commit_rebuild(
        &self,
        pending: &mut Vec<Pending>,
        options: &RebuildOptions,
        report: &mut RebuildReport,
    ) -> Result<(), UguisuError> {
        if pending.is_empty() {
            return Ok(());
        }
        if !options.apply {
            pending.clear();
            return Ok(());
        }
        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        for item in pending.iter() {
            if let Err(e) = archive_files::upsert(&mut tx, &item.file).await {
                // A unique-constraint failure means the archive moved under
                // the rebuild. Report it and keep the rest of the batch.
                note(&mut report.conflicts, &item.file.relative_path);
                report.rebuilt = report.rebuilt.saturating_sub(1);
                tracing::debug!(episode = %item.episode_id, error = %e, "record not restored");
                continue;
            }
            crate::archive_meta::mark_stale(&mut tx, item.podcast_id, now).await?;
        }
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        pending.clear();
        Ok(())
    }
}
