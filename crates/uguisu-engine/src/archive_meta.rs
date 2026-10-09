//! Sidecars and manifests: keeping the archive able to describe itself
//! (ADR 0024).
//!
//! Two artefacts, two very different lifetimes.
//!
//! A **sidecar** belongs to one file and is written once per change to it.
//! It is small, it sits next to its media, and it is what a rebuild reads
//! when the database is gone. It is rendered from the record, so there is
//! exactly one place a sidecar can be wrong.
//!
//! A **manifest** belongs to a podcast and would be rewritten by every
//! download if it were written eagerly — five hundred episodes of one show
//! would mean five hundred rewrites of a growing file. So a change marks
//! the manifest stale *inside the transaction that made the change*, and
//! the file is written later: when the queue goes idle, on `close`, or on
//! request. The flag and the fact commit together, so a crash can only
//! ever leave "marked stale but actually fresh" — never a manifest that
//! silently disagrees with the archive.
//!
//! Neither is on the path of a download. A manifest that was not written
//! is a stale row and a line in `archive manifest status`; it is never a
//! failed download.

use std::collections::BTreeMap;

use time::OffsetDateTime;
use uguisu_archive::manifest::{self, ManifestDiff};
use uguisu_archive::path::{RelativePath, resolve_checked};
use uguisu_archive::sidecar;
use uguisu_archive::verify::hash_file;
use uguisu_core::UguisuError;
use uguisu_core::archive::{
    ArchiveErrorKind, ArchiveFile, ArchiveManifest, ManifestEntry, Sidecar, SidecarArchive,
    SidecarEpisode, SidecarPodcast, SidecarSource,
};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_core::model::{Episode, Podcast};
use uguisu_storage::{
    SqliteConnection, archive_files, episodes, events, manifests, podcasts, sources,
};

use crate::Engine;
use crate::archive::archive_error;

/// Rows read per page while rendering a manifest.
const MANIFEST_PAGE: u32 = 500;

/// Manifests written in one flush.
const FLUSH_BATCH: u32 = 64;

/// Sidecars written in one catch-up pass.
const SIDECAR_BATCH: u32 = 200;

/// Where a sidecar went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarWritten {
    /// The episode it describes.
    pub episode_id: EpisodeId,
    /// Path relative to the media root.
    pub path: String,
}

/// What writing a manifest produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestWritten {
    /// The podcast.
    pub podcast_id: PodcastId,
    /// Path relative to the media root.
    pub path: String,
    /// How many artifacts it lists.
    pub entries: u64,
    /// Whether the row could be marked current, or a change landed while
    /// the file was being written and left it stale for the next flush.
    pub cleared: bool,
}

/// What comparing a manifest against the archive found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestCheck {
    /// The podcast.
    pub podcast_id: PodcastId,
    /// Path relative to the media root.
    pub path: String,
    /// Whether the index has changed since the file was written.
    pub stale: bool,
    /// Whether the files were re-read, or only the index consulted.
    pub rehashed: bool,
    /// What was found.
    pub diff: ManifestDiff,
}

/// Marks a podcast's manifest as out of date.
///
/// Call inside the transaction that changed the artifact — registration,
/// relocation, import or a tag write — so the two commit together.
pub async fn mark_stale(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    now: OffsetDateTime,
) -> Result<(), UguisuError> {
    let path = manifest::path_for(podcast_id);
    manifests::mark_stale(conn, podcast_id, path.as_str(), now).await?;
    Ok(())
}

impl Engine {
    /// Builds the sidecar document for one artifact.
    ///
    /// `feed_url` matters more than it looks: after a total loss of the
    /// database the podcast is added again and gets a fresh identifier, so
    /// the feed URL and the episode's identity key are the only things in
    /// the document that still point at anything. A rebuild resolves
    /// through them.
    fn sidecar_for(
        file: &ArchiveFile,
        episode: &Episode,
        podcast: &Podcast,
        feed_url: Option<url::Url>,
    ) -> Sidecar {
        let enclosure = episode.primary_enclosure();
        Sidecar {
            schema: Sidecar::SCHEMA,
            generator: format!("{}/{}", Sidecar::GENERATOR, uguisu_core::VERSION),
            // Second precision, like every other timestamp Uguisu writes:
            // two writes of an unchanged record then produce byte-identical
            // documents, which is what lets a caller skip a rewrite.
            written_at: OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap_or_else(|_| OffsetDateTime::now_utc()),
            podcast: SidecarPodcast {
                id: podcast.id,
                title: podcast.title.clone(),
                author: podcast.author.clone(),
                publisher: podcast.publisher.clone(),
                feed_url,
                language: podcast.language.clone(),
                categories: podcast.categories.clone(),
            },
            episode: SidecarEpisode {
                id: episode.id,
                identity_key: episode.identity.key.clone(),
                identity_source: Some(episode.identity.source.as_str().to_owned()),
                title: episode.title.clone(),
                published_at: episode.published_at,
                season: episode.season,
                number: episode.episode_number,
                duration_secs: episode.duration_secs,
                description_text: episode.description_text.clone(),
                guid: episode.guid.clone(),
                link: episode.link.clone(),
                enclosure_url: enclosure.map(|e| e.url.clone()),
                enclosure_type: enclosure.and_then(|e| e.mime_type.clone()),
                enclosure_length_bytes: enclosure.and_then(|e| e.length_bytes),
                artwork_url: episode.artwork_url.clone(),
                chapters: episode.extras.chapters.clone(),
                transcripts: episode.extras.transcripts.clone(),
            },
            archive: SidecarArchive {
                relative_path: file.relative_path.clone(),
                size_bytes: file.size_bytes,
                hash_algo: file.hash_algo.clone(),
                hash_value: file.hash_value.clone(),
                content_type: file.content_type.clone(),
                sniffed_type: file.sniffed_type.clone(),
                origin: file.origin,
                tag_state: file.tag_state,
                tag_mode: file.tag_mode,
                tagged_at: file.tagged_at,
                registered_at: file.registered_at,
                original_tags: file.original_tags.clone(),
            },
            source: file
                .source_hash_value
                .clone()
                .map(|hash_value| SidecarSource {
                    hash_algo: file
                        .source_hash_algo
                        .clone()
                        .unwrap_or_else(|| file.hash_algo.clone()),
                    hash_value,
                    size_bytes: file.source_size_bytes.unwrap_or(file.size_bytes),
                    origin_detail: enclosure.map(|e| e.url.to_string()),
                }),
        }
    }

    /// Writes the portable sidecar beside one episode's media file.
    ///
    /// Returns `Ok(None)` when sidecars are switched off, or when the
    /// episode has no archive record — neither is a failure. Writing the
    /// same record twice produces the same bytes, so this is safe to
    /// repeat.
    pub async fn write_sidecar(
        &self,
        episode_id: EpisodeId,
    ) -> Result<Option<SidecarWritten>, UguisuError> {
        if !self.config().archive.sidecars {
            return Ok(None);
        }
        let mut reader = self.storage().reader().await?;
        let Some(file) = archive_files::get_by_episode(&mut reader, episode_id).await? else {
            return Ok(None);
        };
        let (Some(episode), Some(podcast)) = (
            episodes::get(&mut reader, episode_id).await?,
            podcasts::get(&mut reader, file.podcast_id).await?,
        ) else {
            return Ok(None);
        };
        drop(reader);
        self.write_sidecar_for(&file, &episode, &podcast).await
    }

    /// Writes a sidecar from records already in hand.
    async fn write_sidecar_for(
        &self,
        file: &ArchiveFile,
        episode: &Episode,
        podcast: &Podcast,
    ) -> Result<Option<SidecarWritten>, UguisuError> {
        let root = self.media_root()?;
        let media = RelativePath::parse(&file.relative_path)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let feed_url = {
            let mut reader = self.storage().reader().await?;
            sources::current(&mut reader, podcast.id)
                .await?
                .map(|s| s.feed_url)
        };
        let document = Self::sidecar_for(file, episode, podcast, feed_url);
        // Outside any transaction: this writes and fsyncs a file, and the
        // writer connection is a pool of one.
        let written = sidecar::write_atomic(&root, &media, &document)
            .map_err(|e| archive_error(e.kind(), e.to_string()))?;

        let now = OffsetDateTime::now_utc();
        let event = Event::now(
            Some(file.podcast_id),
            Some(file.episode_id),
            EventKind::ArchiveSidecarWritten {
                archive_file_id: file.id,
                path: written.as_str().to_owned(),
            },
        );
        let mut tx = self.storage().begin().await?;
        archive_files::set_sidecar_written(&mut tx, file.id, now).await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        Ok(Some(SidecarWritten {
            episode_id: file.episode_id,
            path: written.as_str().to_owned(),
        }))
    }

    /// Reads the sidecar beside an episode's media file, if there is one.
    pub async fn read_sidecar(
        &self,
        episode_id: EpisodeId,
    ) -> Result<Option<Sidecar>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        let Some(file) = archive_files::get_by_episode(&mut reader, episode_id).await? else {
            return Err(archive_error(
                ArchiveErrorKind::ArchiveNotFound,
                format!("episode {episode_id} has no archive file"),
            ));
        };
        drop(reader);
        let root = self.media_root()?;
        let media = RelativePath::parse(&file.relative_path)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        sidecar::read(&root, &sidecar::path_for(&media))
            .map_err(|e| archive_error(e.kind(), e.to_string()))
    }

    /// Writes the sidecars of artifacts that have none yet.
    ///
    /// Work for an idle moment, never for startup: a missing sidecar
    /// degrades a future rebuild, it does not break the archive.
    pub async fn write_pending_sidecars(&self, limit: u32) -> Result<u64, UguisuError> {
        if !self.config().archive.sidecars {
            return Ok(0);
        }
        let mut written = 0;
        let mut reader = self.storage().reader().await?;
        let pending =
            archive_files::without_sidecar(&mut reader, None, limit.clamp(1, SIDECAR_BATCH))
                .await?;
        drop(reader);
        for file in pending {
            let mut reader = self.storage().reader().await?;
            let episode = episodes::get(&mut reader, file.episode_id).await?;
            let podcast = podcasts::get(&mut reader, file.podcast_id).await?;
            drop(reader);
            let (Some(episode), Some(podcast)) = (episode, podcast) else {
                continue;
            };
            match self.write_sidecar_for(&file, &episode, &podcast).await {
                Ok(Some(_)) => written += 1,
                Ok(None) => {}
                Err(e) => {
                    // One unwritable sidecar must not stop the pass: the
                    // rest of the archive still benefits.
                    tracing::warn!(episode = %file.episode_id, error = %e, "sidecar not written");
                }
            }
        }
        Ok(written)
    }

    /// Renders and writes one podcast's manifest from the index.
    ///
    /// Rows are paged and written as they arrive, so a podcast with ten
    /// thousand episodes costs one page of memory rather than ten
    /// thousand. Nothing is re-hashed: the file reports what the index
    /// believes, and says so in its own header.
    pub async fn write_manifest(
        &self,
        podcast_id: PodcastId,
    ) -> Result<ManifestWritten, UguisuError> {
        let marked_at = {
            let mut reader = self.storage().reader().await?;
            manifests::get(&mut reader, podcast_id)
                .await?
                .map_or_else(OffsetDateTime::now_utc, |m| m.updated_at)
        };
        let root = self.media_root()?;
        let now = OffsetDateTime::now_utc();
        let mut writer = manifest::Writer::create(&root, podcast_id, now)
            .map_err(|e| archive_error(e.kind(), e.to_string()))?;

        let mut reader = self.storage().reader().await?;
        let mut after: Option<String> = None;
        loop {
            let page = archive_files::list_for_podcast(
                &mut reader,
                podcast_id,
                after.as_deref(),
                MANIFEST_PAGE,
            )
            .await?;
            let Some(last) = page.last() else { break };
            after = Some(last.relative_path.clone());
            for file in &page {
                writer
                    .push(&ManifestEntry {
                        relative_path: file.relative_path.clone(),
                        hash_value: file.hash_value.clone(),
                    })
                    .map_err(|e| archive_error(e.kind(), e.to_string()))?;
            }
        }
        drop(reader);
        let written = writer
            .finish()
            .map_err(|e| archive_error(e.kind(), e.to_string()))?;

        let event = Event::now(
            Some(podcast_id),
            None,
            EventKind::ArchiveManifestWritten {
                path: written.relative_path.as_str().to_owned(),
                entries: written.entries,
            },
        );
        let mut tx = self.storage().begin().await?;
        // Guarded on `marked_at`: a registration that landed while the file
        // was being rendered leaves the manifest stale for the next flush
        // rather than being lost.
        let cleared = manifests::mark_written(
            &mut tx,
            podcast_id,
            written.relative_path.as_str(),
            written.entries,
            &written.hash_value,
            marked_at,
            OffsetDateTime::now_utc(),
        )
        .await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        Ok(ManifestWritten {
            podcast_id,
            path: written.relative_path.as_str().to_owned(),
            entries: written.entries,
            cleared,
        })
    }

    /// Writes every manifest the index has moved past.
    ///
    /// Bounded per call; anything left over is still marked stale and is
    /// picked up by the next flush.
    pub async fn write_stale_manifests(&self) -> Result<Vec<ManifestWritten>, UguisuError> {
        if !self.config().archive.manifests {
            return Ok(Vec::new());
        }
        let mut reader = self.storage().reader().await?;
        let stale = manifests::stale(&mut reader, FLUSH_BATCH).await?;
        drop(reader);
        let mut out = Vec::new();
        for row in stale {
            match self.write_manifest(row.podcast_id).await {
                Ok(written) => out.push(written),
                Err(e) => {
                    // The row stays stale, so nothing is lost; the next
                    // flush tries again.
                    tracing::warn!(podcast = %row.podcast_id, error = %e, "manifest not written");
                }
            }
        }
        Ok(out)
    }

    /// Writes what the archive owes: stale manifests first, then a batch
    /// of missing sidecars.
    ///
    /// Never fails: everything it does is derived data that the next pass
    /// can redo, and a warning is more use than an error nobody can act
    /// on. Manifests go first because a stale one is visible in
    /// `archive manifest status`, while a missing sidecar is not visible
    /// until a rebuild needs it.
    pub async fn flush_archive_metadata(&self) {
        match self.write_stale_manifests().await {
            Ok(written) if !written.is_empty() => {
                tracing::debug!(count = written.len(), "manifests written");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "stale manifests could not be written"),
        }
        match self.write_pending_sidecars(SIDECAR_BATCH).await {
            Ok(n) if n > 0 => tracing::debug!(count = n, "sidecars written"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "pending sidecars could not be written"),
        }
    }

    /// Writes every stale manifest, or gives up when the deadline passes.
    ///
    /// This is what makes "after a clean close, every manifest is correct"
    /// true. It loops because a flush is bounded per call, and it is
    /// bounded in total because a shutdown that will not finish must not
    /// hold the process open - whatever is left stays marked stale and the
    /// next start's flush picks it up.
    pub(crate) async fn flush_manifests_until(&self, deadline: std::time::Instant) {
        while std::time::Instant::now() < deadline {
            match self.write_stale_manifests().await {
                Ok(written) if written.is_empty() => return,
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "stale manifests could not be written on close");
                    return;
                }
            }
        }
        tracing::warn!("manifests were still stale at shutdown; the next start will write them");
    }

    /// The manifest state of every podcast that has one.
    pub async fn manifest_status(&self) -> Result<Vec<ArchiveManifest>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(manifests::list(&mut reader).await?)
    }

    /// Compares a podcast's manifest against the archive.
    ///
    /// With `rehash`, each listed file is read and hashed, which is the
    /// independent check; without it, the manifest is compared against the
    /// index, which is cheap and answers "has this file fallen behind".
    /// Either way the work is bounded by one podcast.
    pub async fn verify_manifest(
        &self,
        podcast_id: PodcastId,
        rehash: bool,
    ) -> Result<ManifestCheck, UguisuError> {
        let root = self.media_root()?;
        let relative = manifest::path_for(podcast_id);
        let full = resolve_checked(&root, &relative)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let text = manifest::read_file(&full).map_err(|e| {
            archive_error(
                ArchiveErrorKind::ManifestInvalid,
                format!("{}: {e}", relative.as_str()),
            )
        })?;
        let listed = manifest::parse(&text).map_err(|e| archive_error(e.kind(), e.to_string()))?;

        // What the index holds for this podcast, by path.
        let mut indexed: BTreeMap<String, String> = BTreeMap::new();
        let mut reader = self.storage().reader().await?;
        let mut after: Option<String> = None;
        loop {
            let page = archive_files::list_for_podcast(
                &mut reader,
                podcast_id,
                after.as_deref(),
                MANIFEST_PAGE,
            )
            .await?;
            let Some(last) = page.last() else { break };
            after = Some(last.relative_path.clone());
            for file in page {
                indexed.insert(file.relative_path, file.hash_value);
            }
        }
        let stale = manifests::get(&mut reader, podcast_id)
            .await?
            .is_some_and(|m| m.stale);
        drop(reader);

        let mut found: BTreeMap<String, Option<String>> = BTreeMap::new();
        let listed_paths: std::collections::BTreeSet<&str> =
            listed.iter().map(|e| e.relative_path.as_str()).collect();
        for entry in &listed {
            let observed = if rehash {
                let Ok(path) = RelativePath::parse(&entry.relative_path) else {
                    continue;
                };
                match resolve_checked(&root, &path) {
                    Ok(full) if full.is_file() => match hash_file(&full) {
                        Ok(hash) => Some(Some(hash)),
                        // Present but unreadable: a finding, not a miss.
                        Err(_) => Some(None),
                    },
                    Ok(_) => None,
                    Err(_) => Some(None),
                }
            } else {
                indexed.get(&entry.relative_path).cloned().map(Some)
            };
            if let Some(value) = observed {
                found.insert(entry.relative_path.clone(), value);
            }
        }
        // Artifacts the index holds that the manifest does not list: the
        // file has fallen behind the archive. Only unlisted paths, because
        // for a listed one the check above has already decided - and a row
        // that still names a file somebody deleted must not be allowed to
        // put it back into the comparison.
        for (path, hash) in indexed {
            if !listed_paths.contains(path.as_str()) {
                found.insert(path, Some(hash));
            }
        }

        let diff = manifest::compare(&listed, &found);
        if !diff.is_clean() {
            let event = Event::now(
                Some(podcast_id),
                None,
                EventKind::ArchiveManifestMismatch {
                    path: relative.as_str().to_owned(),
                    changed: diff.changed.count,
                    missing: diff.missing.count,
                    added: diff.added.count,
                },
            );
            let mut tx = self.storage().begin().await?;
            events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            self.bus().publish(std::slice::from_ref(&event));
        }
        Ok(ManifestCheck {
            podcast_id,
            path: relative.as_str().to_owned(),
            stale,
            rehashed: rehash,
            diff,
        })
    }
}
