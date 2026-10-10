//! Writing metadata tags into archived media (ADR 0012, ADR 0026).
//!
//! Uguisu never modifies the only copy of a file. A tag write copies the
//! artifact into its own scratch directory, tags the copy, re-reads and
//! re-hashes it, and only then renames it over the original. Until that rename
//! the artifact is untouched; after it the record moves with the bytes in the
//! same transaction.
//!
//! The hard part is the crash. Between the atomic replace and the record
//! update, "Uguisu retagged this" and "somebody else changed it" are
//! indistinguishable from the bytes alone. So a marker goes down **first**, in
//! its own committed transaction, before a single byte moves:
//! `tag_state = pending`. A crash cannot forge it, and recovery is bounded to
//! the rows carrying it by a partial index that is empty whenever nothing is
//! in flight.
//!
//! This is the one place where a hash is re-read to fit a file rather than the
//! other way round, and it is deliberately narrow: only a row that said
//! `pending` before the write began, recorded as [`reason::RETAG_RECOVERED`]
//! rather than as a verification.
//!
//! Tagging never happens on its own. A command does it, or nothing does.

use std::path::Path;

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uguisu_archive::layout;
use uguisu_archive::path::{RelativePath, resolve_checked};
use uguisu_archive::verify::hash_file;
use uguisu_core::UguisuError;
use uguisu_core::archive::{
    ArchiveErrorKind, ArchiveFile, OriginalCover, OriginalTags, TagMode, TagState,
    VerificationState, VerifyDepth, reason,
};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::{ArchiveFileId, EpisodeId};
use uguisu_core::model::{Episode, Podcast};
use uguisu_download::SpaceProbe;
use uguisu_metadata::{Artwork, Field, OutcomeState, TagSet};
use uguisu_storage::archive_files::{self, TagWrite};
use uguisu_storage::{events, manifests};

use crate::Engine;
use crate::archive::archive_error;

/// Rows a recovery pass looks at in one go.
const RECOVERY_BATCH: u32 = 100;

/// What a tag write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagResult {
    /// The episode.
    pub episode_id: EpisodeId,
    /// Path relative to the media root.
    pub path: String,
    /// How it ended, as `uguisu_metadata::OutcomeState` spells it.
    pub state: &'static str,
    /// The mode that ran.
    pub mode: TagMode,
    /// The fields that changed.
    pub fields: Vec<String>,
    /// Fields this container cannot hold; they stay in the sidecar.
    pub not_embeddable: Vec<String>,
    /// The file's hash afterwards.
    pub hash_value: String,
    /// Whether cover art was embedded.
    pub cover_written: bool,
    /// Anything a person needs to read.
    pub detail: Option<String>,
}

impl Engine {
    /// Writes Uguisu's metadata into one episode's media file.
    pub async fn write_episode_tags(
        &self,
        episode_id: EpisodeId,
        mode: TagMode,
    ) -> Result<TagResult, UguisuError> {
        let _gate = self.tag_gate().await;
        let (podcast, episode, file) = self.archive_subject(episode_id).await?;
        let missing = || {
            archive_error(
                ArchiveErrorKind::ArchiveNotFound,
                format!("episode {episode_id} has no archive file"),
            )
        };
        let file = file.ok_or_else(missing)?;

        // The file must be what the record says before it is rewritten.
        // Tagging a file that is already wrong would replace a detectable
        // problem with an undetectable one. The check settles a write that
        // was interrupted first, so the record it hands on is the current one.
        let verified = self
            .verify_current_held(file, VerifyDepth::Full)
            .await?
            .ok_or_else(missing)?;
        let file = verified.file;
        if verified.state != VerificationState::Verified {
            return Err(archive_error(
                ArchiveErrorKind::TagsFailed,
                format!(
                    "{} is {} ({}); tags are only written to a file that matches its record",
                    file.relative_path, verified.state, verified.reason
                ),
            ));
        }

        let desired = self.tag_set(&podcast, &episode).await?;
        if desired.is_empty() {
            return self
                .skipped(
                    &file,
                    mode,
                    "nothing_to_write",
                    "Uguisu has no values for this episode",
                )
                .await;
        }
        self.run_tag_write(&file, mode, &desired).await
    }

    /// The tags an episode's media file currently carries.
    ///
    /// Only the fields Uguisu manages: the rest of the file's tags are
    /// none of its business, and a caller that cannot see them cannot
    /// accidentally ask for them to be dropped.
    pub async fn read_episode_tags(&self, episode_id: EpisodeId) -> Result<TagSet, UguisuError> {
        let (_, _, file) = self.archive_subject(episode_id).await?;
        let file = file.ok_or_else(|| {
            archive_error(
                ArchiveErrorKind::ArchiveNotFound,
                format!("episode {episode_id} has no archive file"),
            )
        })?;
        let root = self.media_root()?;
        let relative = RelativePath::parse(&file.relative_path)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let full = resolve_checked(&root, &relative)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        uguisu_metadata::read_tags(&full)
            .map_err(|e| archive_error(ArchiveErrorKind::TagsFailed, e.to_string()))
    }

    /// The values Uguisu would write for one episode.
    async fn tag_set(&self, podcast: &Podcast, episode: &Episode) -> Result<TagSet, UguisuError> {
        let mut set = TagSet::default();
        set.set(Field::Title, episode.title.clone());
        set.set(Field::Album, podcast.title.clone());
        set.set_opt(
            Field::Artist,
            episode.author.clone().or_else(|| podcast.author.clone()),
        );
        set.set_opt(
            Field::AlbumArtist,
            podcast.author.clone().or_else(|| podcast.publisher.clone()),
        );
        set.set_opt(Field::Publisher, podcast.publisher.clone());
        set.set_opt(Field::Description, episode.description_text.clone());
        set.set_opt(Field::Genre, podcast.categories.first().cloned());
        set.set_opt(Field::Language, podcast.language.clone());
        set.set_opt(Field::Copyright, podcast.copyright.clone());
        set.set_opt(Field::EpisodeGuid, episode.guid.clone());
        if let Some(published) = episode.published_at {
            // The date alone: a full RFC 3339 timestamp comes back without
            // its `Z`, and a value that does not round-trip would be
            // rewritten on every `sync` - moving the archive's hash each
            // time for no change at all.
            set.set(Field::RecordingDate, published.date().to_string());
        }
        if let Some(number) = episode.episode_number {
            set.set(Field::TrackNumber, number.to_string());
        }
        if let Some(season) = episode.season {
            set.set(Field::DiscNumber, season.to_string());
        }
        let mut reader = self.storage().reader().await?;
        let current = uguisu_storage::artwork::current(&mut reader, podcast.id).await?;
        drop(reader);
        if let Some(art) = current {
            let root = self.media_root()?;
            if let Ok(relative) = RelativePath::parse(&art.relative_path)
                && let Ok(full) = resolve_checked(&root, &relative)
                && let Ok(bytes) = std::fs::read(&full)
            {
                set.cover = Some(Artwork {
                    mime: art.format.mime().to_owned(),
                    bytes,
                });
            }
        }
        Ok(set)
    }

    /// Copy, tag, prove, replace, record.
    #[allow(clippy::too_many_lines)] // the order is the guarantee
    async fn run_tag_write(
        &self,
        file: &ArchiveFile,
        mode: TagMode,
        desired: &TagSet,
    ) -> Result<TagResult, UguisuError> {
        let root = self.media_root()?;
        let relative = RelativePath::parse(&file.relative_path)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let media = resolve_checked(&root, &relative)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;

        // Two copies of the artifact exist for the length of this
        // operation. Refusing up front beats filling the disk halfway
        // through and leaving the archive to the queue's disk-full path.
        let probe = uguisu_download::Fs4Probe;
        if let Ok(available) = probe.available(&root)
            && available < file.size_bytes.saturating_mul(2)
        {
            return Err(archive_error(
                ArchiveErrorKind::TagsFailed,
                format!(
                    "tagging {} needs about {} bytes of scratch space and {available} are free",
                    file.relative_path,
                    file.size_bytes * 2
                ),
            ));
        }

        let scratch = layout::tmp_path(&format!("{}.tagtmp", ArchiveFileId::new()));
        let scratch_full = resolve_checked(&root, &scratch)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let _held = layout::Scratch::hold(&scratch_full);
        if let Some(parent) = scratch_full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(parent, &e))?;
        }

        // The marker goes down before a single byte moves, and in its own
        // committed transaction. After a crash it is the only thing that
        // can tell an interrupted write from tampering.
        let now = OffsetDateTime::now_utc();
        // The original tags go down with the marker, before anything is
        // copied (ADR 0012) - but only for a file Uguisu never tagged: a
        // tagged file's current tags are Uguisu's own. A file whose tags
        // cannot be read gets no snapshot; the write reports why.
        let original = (file.original_tags.is_none() && file.tagged_at.is_none())
            .then(|| uguisu_metadata::read_tags(&media).ok())
            .flatten()
            .map(|tags| original_tags_of(&tags, now));
        let mut tx = self.storage().begin().await?;
        archive_files::set_tag_state(&mut tx, file.id, TagState::Pending, now).await?;
        if let Some(original) = &original {
            archive_files::set_original_tags(&mut tx, file.id, original).await?;
        }
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;

        let mut replaced = false;
        let result = self
            .tag_the_copy(file, mode, desired, &media, &scratch_full, &mut replaced)
            .await;
        match result {
            Ok(outcome) => Ok(outcome),
            // The new bytes are in place and the record did not follow.
            // The marker stays: it is what lets the next start's recovery
            // adopt them instead of a later full pass calling Uguisu's own
            // write a hash mismatch.
            Err(e) if replaced => Err(e),
            Err(e) => {
                // The media file has not been touched: the copy is what
                // failed. Clear the marker so a later recovery pass does
                // not go looking for a write that never started.
                let now = OffsetDateTime::now_utc();
                let mut tx = self.storage().begin().await?;
                archive_files::set_tag_state(&mut tx, file.id, TagState::Failed, now).await?;
                tx.commit()
                    .await
                    .map_err(uguisu_storage::StorageError::from)?;
                drop(std::fs::remove_file(&scratch_full));
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_lines)] // copy, tag, prove, replace, record
    async fn tag_the_copy(
        &self,
        file: &ArchiveFile,
        mode: TagMode,
        desired: &TagSet,
        media: &Path,
        scratch: &Path,
        replaced: &mut bool,
    ) -> Result<TagResult, UguisuError> {
        std::fs::copy(media, scratch).map_err(|e| io_error(scratch, &e))?;
        let outcome = uguisu_metadata::write_tags(scratch, desired, mode).map_err(|e| {
            // A file that is not a container Uguisu recognises is a fact
            // about the file, not a fault in the server: it is reported
            // the same way a WAV is, rather than as an internal error.
            let kind = match e {
                uguisu_metadata::MetadataError::Unreadable { .. } => {
                    ArchiveErrorKind::TagsUnsupported
                }
                _ => ArchiveErrorKind::TagsFailed,
            };
            archive_error(kind, e.to_string())
        })?;

        if outcome.state != OutcomeState::Written {
            // Unsupported, or nothing to change. Either way the artifact
            // is untouched and the copy has served its purpose.
            drop(std::fs::remove_file(scratch));
            // `file` is the record before `pending`: a file Uguisu already
            // tagged and that needed nothing more is still tagged.
            let state = if outcome.state == OutcomeState::Unsupported {
                TagState::Unsupported
            } else if file.tag_state == TagState::Written {
                TagState::Written
            } else {
                TagState::Untagged
            };
            let mut result = self
                .skipped(
                    file,
                    mode,
                    outcome.state.as_str(),
                    outcome.detail.unwrap_or(""),
                )
                .await?;
            result.not_embeddable = outcome
                .not_embeddable
                .iter()
                .map(|f| f.as_str().to_owned())
                .collect();
            let now = OffsetDateTime::now_utc();
            let mut tx = self.storage().begin().await?;
            archive_files::set_tag_state(&mut tx, file.id, state, now).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            return Ok(result);
        }

        // Prove the replacement before it replaces anything: it has to
        // re-parse as the container it claims to be, and its hash and size
        // have to be readable.
        uguisu_metadata::read_tags(scratch).map_err(|e| {
            archive_error(
                ArchiveErrorKind::TagsFailed,
                format!("the tagged copy did not read back: {e}"),
            )
        })?;
        let hash = hash_file(scratch).map_err(|e| io_error(scratch, &e))?;
        let meta = std::fs::metadata(scratch).map_err(|e| io_error(scratch, &e))?;

        std::fs::rename(scratch, media).map_err(|e| io_error(media, &e))?;
        *replaced = true;
        if let Some(parent) = media.parent()
            && let Ok(dir) = std::fs::File::open(parent)
        {
            drop(dir.sync_all());
        }

        let now = OffsetDateTime::now_utc();
        let fields: Vec<String> = outcome
            .written
            .iter()
            .map(|f| f.as_str().to_owned())
            .collect();
        let event = Event::now(
            Some(file.podcast_id),
            Some(file.episode_id),
            EventKind::ArchiveTagged {
                archive_file_id: file.id,
                path: file.relative_path.clone(),
                mode,
                fields: fields.clone(),
                size_bytes: meta.len(),
                hash_value: hash.clone(),
            },
        );
        let mut tx = self.storage().begin().await?;
        archive_files::set_tagged(
            &mut tx,
            file.id,
            &TagWrite {
                mode,
                size_bytes: meta.len(),
                hash_value: hash.clone(),
                mtime_unix: mtime_of(&meta),
                verification_state: VerificationState::Verified,
                reason: Some(reason::TAGGED.to_owned()),
                at: now,
            },
        )
        .await?;
        manifests::mark_stale(
            &mut tx,
            file.podcast_id,
            uguisu_archive::manifest::path_for(file.podcast_id).as_str(),
            now,
        )
        .await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));

        // The sidecar describes the bytes, so it moves with them.
        if let Err(e) = self.write_sidecar(file.episode_id).await {
            tracing::warn!(episode = %file.episode_id, error = %e, "sidecar not rewritten after tagging");
        }
        Ok(TagResult {
            episode_id: file.episode_id,
            path: file.relative_path.clone(),
            state: OutcomeState::Written.as_str(),
            mode,
            fields,
            not_embeddable: outcome
                .not_embeddable
                .iter()
                .map(|f| f.as_str().to_owned())
                .collect(),
            hash_value: hash,
            cover_written: outcome.cover_written,
            detail: None,
        })
    }

    /// Records that nothing was written, and why.
    async fn skipped(
        &self,
        file: &ArchiveFile,
        mode: TagMode,
        state: &'static str,
        detail: &str,
    ) -> Result<TagResult, UguisuError> {
        let event = Event::now(
            Some(file.podcast_id),
            Some(file.episode_id),
            EventKind::ArchiveTagsSkipped {
                archive_file_id: file.id,
                reason: state.to_owned(),
            },
        );
        let mut tx = self.storage().begin().await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        Ok(TagResult {
            episode_id: file.episode_id,
            path: file.relative_path.clone(),
            state,
            mode,
            fields: Vec::new(),
            not_embeddable: Vec::new(),
            hash_value: file.hash_value.clone(),
            cover_written: false,
            detail: (!detail.is_empty()).then(|| detail.to_owned()),
        })
    }

    /// Finishes tag writes a crash or a failure after the rename interrupted.
    ///
    /// Backed by a partial index that is empty whenever nothing is in
    /// flight, so asking costs nothing on a large archive. Returns how
    /// many rows it settled.
    pub async fn recover_interrupted_tagging(&self) -> Result<u64, UguisuError> {
        let mut reader = self.storage().reader().await?;
        let pending = archive_files::tagging_interrupted(&mut reader, RECOVERY_BATCH).await?;
        drop(reader);
        let mut settled = 0;
        for id in pending {
            let _gate = self.tag_gate().await;
            let mut reader = self.storage().reader().await?;
            let file = archive_files::get(&mut reader, id).await?;
            drop(reader);
            // A write that finished while this waited for the gate left
            // nothing to settle.
            let Some(file) = file.filter(|f| f.tag_state.is_in_flight()) else {
                continue;
            };
            if self.settle_tag_write(&file).await?.is_some() {
                settled += 1;
            }
        }
        Ok(settled)
    }

    /// Settles the interrupted tag write `file` records, from the bytes at
    /// its path, and returns the record as it is afterwards; `None` when
    /// those bytes could not be read. The caller holds the tag gate.
    pub(crate) async fn settle_tag_write(
        &self,
        file: &ArchiveFile,
    ) -> Result<Option<ArchiveFile>, UguisuError> {
        let root = self.media_root()?;
        let Ok(relative) = RelativePath::parse(&file.relative_path) else {
            return Ok(None);
        };
        let Ok(full) = resolve_checked(&root, &relative) else {
            return Ok(None);
        };
        // A file that is gone holds no bytes to adopt: the record stays
        // what it says, and a verification reports the file missing.
        let hash = match hash_file(&full) {
            Ok(hash) => Some(hash),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(
                    episode_id = %file.episode_id,
                    path = %file.relative_path,
                    error = %e,
                    "an interrupted tag write could not be settled"
                );
                return Ok(None);
            }
        };
        let id = file.id;
        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        match hash {
            None => {
                archive_files::set_tag_state(&mut tx, id, untouched(file), now).await?;
            }
            // The replacement never landed. The artifact is exactly what the
            // record says, so there is nothing to adopt.
            Some(hash) if hash == file.hash_value => {
                archive_files::set_tag_state(&mut tx, id, untouched(file), now).await?;
            }
            Some(hash) => {
                // The replacement did land and the record had not caught
                // up. The marker was committed before the first byte
                // moved, so this cannot be someone else's edit - and it is
                // recorded as a recovered retag, never as a verification.
                let meta = std::fs::metadata(&full).ok();
                archive_files::set_tagged(
                    &mut tx,
                    id,
                    &TagWrite {
                        mode: file.tag_mode.unwrap_or_default(),
                        size_bytes: meta
                            .as_ref()
                            .map_or(file.size_bytes, std::fs::Metadata::len),
                        hash_value: hash,
                        mtime_unix: meta.as_ref().and_then(mtime_of),
                        verification_state: VerificationState::Unchecked,
                        reason: Some(reason::RETAG_RECOVERED.to_owned()),
                        at: now,
                    },
                )
                .await?;
                manifests::mark_stale(
                    &mut tx,
                    file.podcast_id,
                    uguisu_archive::manifest::path_for(file.podcast_id).as_str(),
                    now,
                )
                .await?;
            }
        }
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        let mut reader = self.storage().reader().await?;
        Ok(archive_files::get(&mut reader, id).await?)
    }
}

/// The tag state of a file whose interrupted write did not change it. Only
/// a completed write sets `tagged_at`, so it says whether these bytes were
/// already Uguisu's tags.
fn untouched(file: &ArchiveFile) -> TagState {
    if file.tagged_at.is_some() {
        TagState::Written
    } else {
        TagState::Untagged
    }
}

fn original_tags_of(tags: &TagSet, at: OffsetDateTime) -> OriginalTags {
    OriginalTags {
        values: tags
            .values
            .iter()
            .map(|(field, value)| (field.as_str().to_owned(), value.clone()))
            .collect(),
        cover: tags.cover.as_ref().map(|art| OriginalCover {
            mime: art.mime.clone(),
            size_bytes: u64::try_from(art.bytes.len()).unwrap_or(u64::MAX),
            sha256: hex::encode(Sha256::digest(&art.bytes)),
        }),
        captured_at: at,
    }
}

fn io_error(path: &Path, e: &std::io::Error) -> UguisuError {
    archive_error(
        ArchiveErrorKind::TagsFailed,
        format!("{}: {e}", path.display()),
    )
}

fn mtime_of(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}
