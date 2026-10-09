//! Putting a missing archived file back from a copy elsewhere (ADR 0060).
//!
//! A record keeps the hash of the bytes it describes, so the bytes are the
//! identity: a file in the source folder restores a record only when its
//! SHA-256 is the record's. There is no scoring and no guess. Like an
//! import, the source is only read, the target is never overwritten and
//! nothing is deleted; unlike an import, the record already exists, and the
//! file goes back to the path it names.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use uguisu_archive::layout;
use uguisu_archive::path::{RelativePath, resolve_checked};
use uguisu_archive::scan::{self, ScanOptions, ScannedFile};
use uguisu_archive::verify::hash_file;
use uguisu_core::UguisuError;
use uguisu_core::archive::{ArchiveErrorKind, ArchiveFile, VerificationState, VerifyDepth};
use uguisu_core::ids::{ArchiveFileId, EpisodeId, PodcastId};
use uguisu_storage::archive_files::{self, ArchiveFilter};

use crate::Engine;
use crate::archive::{archive_error, sync_dir};
use crate::import::{blocking, copy_and_hash};

/// What a restore decided about one missing record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreAction {
    /// A file with the record's exact bytes was found and is copied back.
    Restore,
    /// The record's own bytes are at its path again; applying verifies them.
    Returned,
    /// Only the bytes Uguisu received before its own tag write were found:
    /// reported, not used, because the record describes the tagged file.
    SourceOnly,
    /// Something now occupies the record's path.
    Taken,
    /// The file in the folder changed between hashing and copying; the
    /// record stays missing.
    Changed,
    /// Nothing in the folder has the record's bytes.
    NotFound,
    /// The record's path could not be checked or the copy failed; the
    /// detail says why, and the record stays missing.
    Failed,
}

impl RestoreAction {
    /// Stable string form, as the CLI and the API spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Restore => "restore",
            Self::Returned => "returned",
            Self::SourceOnly => "source_only",
            Self::Taken => "taken",
            Self::Changed => "changed",
            Self::NotFound => "not_found",
            Self::Failed => "failed",
        }
    }
}

/// The item, marked failed for `why`.
fn failed(mut item: RestoreItem, why: String) -> RestoreItem {
    item.action = RestoreAction::Failed;
    item.detail = Some(why);
    item
}

/// One missing record and what a restore does about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreItem {
    /// The record's episode.
    pub episode_id: EpisodeId,
    /// The record's podcast.
    pub podcast_id: PodcastId,
    /// Where the record says the file belongs, relative to the media root.
    pub target_path: String,
    /// What would happen, or did.
    pub action: RestoreAction,
    /// The file in the folder that holds the bytes, relative to the folder.
    pub source_path: Option<String>,
    /// Anything a person needs to read.
    pub detail: Option<String>,
}

/// What a restore would do, or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// Whether files were copied.
    pub applied: bool,
    /// The folder that was read.
    pub source_root: String,
    /// Files in the folder that were looked at.
    pub scanned: u64,
    /// One line per missing record, in record order.
    pub items: Vec<RestoreItem>,
}

/// What a restore is asked to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreOptions {
    /// Whether files are copied. The default is a dry run.
    pub apply: bool,
    /// Only this podcast's missing records.
    pub podcast: Option<PodcastId>,
}

impl Engine {
    /// Looks in `source` for the bytes of every missing record and says
    /// what restoring them would do, or, with `apply`, does it.
    ///
    /// # Errors
    /// `import_source_invalid` when the folder is unusable or overlaps the
    /// media directory; storage errors otherwise. A record whose file cannot
    /// be checked or copied back is a `failed` line in the report, not an
    /// error.
    pub async fn restore(
        &self,
        source: &Path,
        options: &RestoreOptions,
    ) -> Result<RestoreReport, UguisuError> {
        let root = self.import_root(source)?;
        let media_root = self.media_root()?;
        let missing = self.missing_records(options.podcast).await?;

        // Only files whose size is some record's size can hold its bytes, so
        // nothing else is hashed; the extensions are the records' own, which
        // also finds a `.bin` or `.weba` that no media list names.
        let mut extensions: Vec<String> = missing
            .iter()
            .filter_map(|f| {
                f.relative_path
                    .rsplit_once('.')
                    .map(|(_, e)| e.to_ascii_lowercase())
            })
            .collect();
        extensions.sort_unstable();
        extensions.dedup();
        let sizes: std::collections::HashSet<u64> = missing
            .iter()
            .flat_map(|f| [Some(f.size_bytes), f.source_size_bytes])
            .flatten()
            .collect();
        let scan_root = root.clone();
        let (scanned, by_size) = blocking(move || {
            let options = ScanOptions {
                read_head: false,
                extensions,
                ..ScanOptions::default()
            };
            let mut scanned = 0u64;
            let mut by_size: HashMap<u64, Vec<ScannedFile>> = HashMap::new();
            for file in scan::walk(&scan_root, &options).flatten() {
                scanned += 1;
                if sizes.contains(&file.size_bytes) {
                    by_size.entry(file.size_bytes).or_default().push(file);
                }
            }
            (scanned, by_size)
        })
        .await?;

        let mut hashes: HashMap<PathBuf, String> = HashMap::new();
        let mut items = Vec::with_capacity(missing.len());
        for record in &missing {
            let item = self
                .restore_one(&media_root, record, &by_size, &mut hashes, options.apply)
                .await?;
            items.push(item);
        }
        let report = RestoreReport {
            applied: options.apply,
            source_root: root.display().to_string(),
            scanned,
            items,
        };
        let restored = report
            .items
            .iter()
            .filter(|i| i.action == RestoreAction::Restore)
            .count();
        tracing::info!(
            applied = report.applied,
            scanned = report.scanned,
            missing = report.items.len(),
            restored,
            "archive restore finished"
        );
        Ok(report)
    }

    /// Every record in state `missing`, in record order.
    async fn missing_records(
        &self,
        podcast: Option<PodcastId>,
    ) -> Result<Vec<ArchiveFile>, UguisuError> {
        let filter = ArchiveFilter {
            state: Some(VerificationState::Missing),
            podcast_id: podcast,
            ..ArchiveFilter::default()
        };
        let mut out = Vec::new();
        let mut after: Option<(OffsetDateTime, ArchiveFileId)> = None;
        loop {
            let mut reader = self.storage().reader().await?;
            let batch = archive_files::list(&mut reader, &filter, after, 500).await?;
            drop(reader);
            let Some(last) = batch.last() else { break };
            after = Some((last.created_at, last.id));
            let full = batch.len() >= 500;
            out.extend(batch);
            if !full {
                break;
            }
        }
        Ok(out)
    }

    async fn restore_one(
        &self,
        media_root: &Path,
        record: &ArchiveFile,
        by_size: &HashMap<u64, Vec<ScannedFile>>,
        hashes: &mut HashMap<PathBuf, String>,
        apply: bool,
    ) -> Result<RestoreItem, UguisuError> {
        let mut item = RestoreItem {
            episode_id: record.episode_id,
            podcast_id: record.podcast_id,
            target_path: record.relative_path.clone(),
            action: RestoreAction::NotFound,
            source_path: None,
            detail: None,
        };
        let resolved = RelativePath::parse(&record.relative_path)
            .map_err(|e| e.to_string())
            .and_then(|p| resolve_checked(media_root, &p).map_err(|e| e.to_string()));
        let target = match resolved {
            Ok(target) => target,
            Err(e) => {
                return Ok(failed(
                    item,
                    format!("the record's path does not resolve inside the media directory: {e}"),
                ));
            }
        };
        match std::fs::symlink_metadata(&target) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Ok(failed(
                    item,
                    format!("the record's path cannot be checked: {e}"),
                ));
            }
            Ok(meta) => {
                // Only the record's own bytes in a plain file count as the file
                // coming back; anything else at its path, a link included, is
                // somebody's, and stays.
                let at = target.clone();
                let same = meta.is_file()
                    && meta.len() == record.size_bytes
                    && blocking(move || hash_file(&at))
                        .await?
                        .is_ok_and(|h| h.eq_ignore_ascii_case(&record.hash_value));
                if !same {
                    item.action = RestoreAction::Taken;
                    item.detail = Some("something else is at the record's path".to_owned());
                    return Ok(item);
                }
                item.action = RestoreAction::Returned;
                item.detail = Some("the file is at its path again".to_owned());
                return Ok(if apply {
                    self.verified(record, item).await
                } else {
                    item
                });
            }
        }

        let mut source_only = None;
        let mut sizes = vec![record.size_bytes];
        sizes.extend(record.source_size_bytes.filter(|s| *s != record.size_bytes));
        for size in sizes {
            for candidate in by_size.get(&size).into_iter().flatten() {
                let hash = if let Some(h) = hashes.get(&candidate.absolute) {
                    h.clone()
                } else {
                    let path = candidate.absolute.clone();
                    let Ok(h) = blocking(move || hash_file(&path)).await? else {
                        continue;
                    };
                    hashes.insert(candidate.absolute.clone(), h.clone());
                    h
                };
                if size == record.size_bytes && hash.eq_ignore_ascii_case(&record.hash_value) {
                    item.source_path = Some(candidate.relative.as_str().to_owned());
                    item.action = RestoreAction::Restore;
                    if apply {
                        return self
                            .land_exact(media_root, record, &target, candidate, item)
                            .await;
                    }
                    return Ok(item);
                }
                if record
                    .source_hash_value
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&hash))
                {
                    source_only = Some(candidate.relative.as_str().to_owned());
                }
            }
        }
        if let Some(path) = source_only {
            item.action = RestoreAction::SourceOnly;
            item.source_path = Some(path);
            item.detail = Some(
                "these are the bytes as received, before Uguisu wrote its tags; the record describes the tagged file"
                    .to_owned(),
            );
        }
        Ok(item)
    }

    /// Copies one source file back to its record's path and verifies it.
    async fn land_exact(
        &self,
        media_root: &Path,
        record: &ArchiveFile,
        target: &Path,
        source: &ScannedFile,
        mut item: RestoreItem,
    ) -> Result<RestoreItem, UguisuError> {
        let scratch = layout::tmp_path(&format!("{}.restore", ArchiveFileId::new()));
        let scratch_full = resolve_checked(media_root, &scratch)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let from = source.absolute.clone();
        let to = target.to_path_buf();
        let expected = record.hash_value.clone();
        let landed = blocking(move || -> Result<(), (RestoreAction, String)> {
            let io = |e: std::io::Error| (RestoreAction::Failed, e.to_string());
            let _held = layout::Scratch::hold(&scratch_full);
            if let Some(parent) = scratch_full.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            let (hash, _) = copy_and_hash(&from, &scratch_full).map_err(io)?;
            if !hash.eq_ignore_ascii_case(&expected) {
                // The source changed since it was hashed; the scratch copy
                // stays to be reported, the record stays missing.
                return Err((
                    RestoreAction::Changed,
                    "the file in the folder changed while it was copied".to_owned(),
                ));
            }
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            match uguisu_download::paths::rename_new(&scratch_full, &to) {
                Ok(()) => {
                    sync_dir(to.parent());
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err((
                    RestoreAction::Taken,
                    "the record's path was taken while the file was being copied".to_owned(),
                )),
                Err(e) => Err(io(e)),
            }
        })
        .await?;
        match landed {
            Ok(()) => Ok(self.verified(record, item).await),
            Err((action, detail)) => {
                item.action = action;
                item.detail = Some(detail);
                Ok(item)
            }
        }
    }

    /// Verifies a file that is back at its record's path in full, and says so
    /// on the item when the check does not find the record's bytes.
    async fn verified(&self, record: &ArchiveFile, item: RestoreItem) -> RestoreItem {
        match self
            .verify_episode(record.episode_id, VerifyDepth::Full)
            .await
        {
            Ok(checked) if checked.state == VerificationState::Verified => item,
            Ok(checked) => failed(
                item,
                format!("it is back, but a full check found it {}", checked.state),
            ),
            Err(e) => failed(
                item,
                format!("it is back, but it could not be checked: {e}"),
            ),
        }
    }
}
