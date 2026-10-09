//! Copying a foreign archive in (ADR 0025).
//!
//! Four rules, and the whole module is arranged around them.
//!
//! **The source is never modified.** Files are copied, never moved; the
//! source tree is opened read-only and nothing in it is written, renamed
//! or removed. There is no `--move`, and adding one later would be a
//! separate, separately documented decision.
//!
//! **An ambiguous file is never imported.** A file placed under the wrong
//! episode is a quiet, permanent error that nobody notices until they play
//! it. An unmatched file is a line in a report. The matcher decides, and
//! it refuses rather than guesses.
//!
//! **Nothing is overwritten.** A target that already holds the same bytes
//! is `already_present`; one that holds different bytes is a reported
//! collision. Neither writes. An episode that is already archived keeps
//! its record whatever an import finds (ADR 0050).
//!
//! **Nothing is deleted.** Unmatched files, leftovers from an interrupted
//! run and unreadable entries are all reported and left where they are.
//!
//! Dry run is the default and does the whole scan and match, so the plan
//! a user reads is the plan that would run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use uguisu_archive::collision;
use uguisu_archive::import::{
    Candidate, EpisodeFacts, ImportFormat, MatchedBy, Verdict, classify, matching,
};
use uguisu_archive::layout;
use uguisu_archive::path::{RelativePath, resolve_checked};
use uguisu_archive::scan::{self, ScanOptions, ScannedFile};
use uguisu_archive::template::Context;
use uguisu_archive::verify::hash_file;
use uguisu_core::UguisuError;
use uguisu_core::archive::{ArchiveErrorKind, ArchiveFile, ArchiveOrigin, TagState, reason};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::{ArchiveFileId, EpisodeId, PodcastId};
use uguisu_core::model::{Episode, Podcast};
use uguisu_discovery::dedup::feed_key;
use uguisu_feed::identity::normalize_enclosure_url;
use uguisu_storage::podgrab::PodgrabRecord;
use uguisu_storage::{archive_files, episodes, events, podcasts, sources};
use url::Url;

use crate::Engine;
use crate::archive::{DbAndDisk, archive_error};

/// Largest side-metadata file read from a source tree.
const MAX_SIDE_BYTES: u64 = 64 * 1024;

/// How alike a directory name and a podcast title must be, in percent,
/// and by how much the best must beat the runner-up.
const PODCAST_GATE: u32 = 80;
const PODCAST_MARGIN: u32 = 10;

/// Files examined before the layout is decided.
const DETECT_SAMPLE: usize = 64;

/// What an import was asked to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// Whether files are copied. The default is a dry run.
    pub apply: bool,
    /// The layout to read the tree as; detected when absent.
    pub format: Option<ImportFormat>,
    /// Import only into this podcast, skipping the directory gate.
    pub podcast: Option<PodcastId>,
    /// Confidence a match needs, in percent; the configured default when
    /// absent.
    pub threshold: Option<u32>,
    /// Podgrab's database, read only, to name each file exactly; implies
    /// the Podgrab layout (ADR 0050).
    pub podgrab_db: Option<PathBuf>,
}

/// What an import decided about one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Copy it in.
    Import,
    /// Uguisu already holds these exact bytes for this episode.
    AlreadyPresent,
    /// Something else is at the target path.
    Conflict,
    /// More than one episode explains it.
    Ambiguous,
    /// No episode explains it.
    Unmatched,
    /// It is not usable as media at all.
    Invalid,
}

impl Action {
    /// Stable string form, as the CLI, the API and the events spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::AlreadyPresent => "already_present",
            Self::Conflict => "conflict",
            Self::Ambiguous => "ambiguous",
            Self::Unmatched => "unmatched",
            Self::Invalid => "invalid",
        }
    }
}

/// One line of an import plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Path relative to the import root.
    pub source_path: String,
    /// Length in bytes.
    pub size_bytes: u64,
    /// What would happen, or did.
    pub action: Action,
    /// The podcast it was attributed to.
    pub podcast_id: Option<PodcastId>,
    /// The episode it matched.
    pub episode_id: Option<EpisodeId>,
    /// Matching confidence in percent.
    pub confidence: u32,
    /// How it was decided: `scored`, or `embedded_guid` when the file's own
    /// tags named the episode (ADR 0050).
    pub matched_by: Option<String>,
    /// Where it would land, relative to the media root.
    pub target_path: Option<String>,
    /// Anything a person needs to read.
    pub detail: Option<String>,
}

/// How many items fell into each action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportCounts {
    /// Files examined.
    pub scanned: u64,
    /// Files that would be, or were, imported.
    pub imported: u64,
    /// Files Uguisu already holds.
    pub already_present: u64,
    /// Files whose target is taken by something else.
    pub conflicts: u64,
    /// Files matching more than one episode too closely to choose.
    pub ambiguous: u64,
    /// Files matching nothing well enough.
    pub unmatched: u64,
    /// Files that are not usable media.
    pub invalid: u64,
    /// Entries the scan could not look at.
    pub unreadable: u64,
}

/// What an import would do, or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    /// Whether files were copied.
    pub applied: bool,
    /// The tree that was read.
    pub source_root: String,
    /// The layout it was read as.
    pub format: ImportFormat,
    /// Confidence a match needed.
    pub threshold: u32,
    /// The counts.
    pub counts: ImportCounts,
    /// Every file, in scan order.
    pub items: Vec<PlanItem>,
}

impl Engine {
    /// Reads a foreign archive and says what importing it would do.
    ///
    /// Performs the whole scan and match, and writes nothing whatsoever.
    pub async fn import_plan(
        &self,
        source: &Path,
        options: &ImportOptions,
    ) -> Result<ImportPlan, UguisuError> {
        self.import_inner(
            source,
            &ImportOptions {
                apply: false,
                ..options.clone()
            },
        )
        .await
    }

    /// Reads a foreign archive and copies in what it can place.
    pub async fn import_apply(
        &self,
        source: &Path,
        options: &ImportOptions,
    ) -> Result<ImportPlan, UguisuError> {
        self.import_inner(
            source,
            &ImportOptions {
                apply: true,
                ..options.clone()
            },
        )
        .await
    }

    #[allow(clippy::too_many_lines)] // one pass, and splitting it would hide the order
    async fn import_inner(
        &self,
        source: &Path,
        options: &ImportOptions,
    ) -> Result<ImportPlan, UguisuError> {
        let root = self.import_root(source)?;
        let media_root = self.media_root()?;
        let threshold = options
            .threshold
            .unwrap_or(self.config().archive.import_match_threshold)
            .clamp(1, 100);

        let records = match &options.podgrab_db {
            Some(_) if options.format == Some(ImportFormat::Generic) => {
                return Err(UguisuError::Invalid(
                    "a Podgrab database describes Podgrab's layout, not the generic one".to_owned(),
                ));
            }
            Some(db) => Some(podgrab_records(db).await?),
            None => None,
        };

        let mut counts = ImportCounts::default();
        let walk_root = root.clone();
        let (files, unreadable) = blocking(move || {
            let mut files: Vec<ScannedFile> = Vec::new();
            let mut unreadable = 0;
            for entry in scan::walk(&walk_root, &ScanOptions::default()) {
                match entry {
                    Ok(file) => files.push(file),
                    Err(e) => {
                        unreadable += 1;
                        tracing::debug!(path = %e.path().display(), error = %e, "import entry skipped");
                    }
                }
            }
            (files, unreadable)
        })
        .await?;
        counts.unreadable += unreadable;
        let format = match (options.format, &records) {
            (_, Some(_)) => ImportFormat::Podgrab,
            (Some(f), None) => f,
            (None, None) => ImportFormat::detect(&files[..files.len().min(DETECT_SAMPLE)]),
        };
        let reader = format.reader();

        let library = self.podcast_titles().await?;
        let feeds: HashMap<String, PodcastId> = if records.is_some() {
            let mut conn = self.storage().reader().await?;
            sources::known_urls(&mut conn)
                .await?
                .into_iter()
                .map(|(id, url)| (feed_key(&url), id))
                .collect()
        } else {
            HashMap::new()
        };
        let mut items = Vec::new();
        // The podcast is decided per directory, or per file where Podgrab's
        // database names it, and its episodes are loaded only when it
        // changes: the memory an import needs is one podcast's index
        // rather than the library's.
        let mut gates: HashMap<String, Option<PodcastId>> = HashMap::new();
        let mut current: Option<PodcastIndex> = None;
        // Targets this run has already given out, so two episodes never
        // plan onto one path that the database and the disk still call free.
        let mut planned: HashMap<String, EpisodeId> = HashMap::new();

        for file in &files {
            counts.scanned += 1;
            let record = records
                .as_ref()
                .and_then(|r| r.get(&path_tail(&file.absolute.to_string_lossy())))
                .and_then(Option::as_ref);
            let chosen = match (
                options.podcast,
                record.and_then(|r| podcast_of(r, &feeds, &library)),
            ) {
                (Some(id), _) | (None, Some(id)) => Some(id),
                (None, None) => *gates
                    .entry(file.relative.parent().unwrap_or("").to_owned())
                    .or_insert_with(|| {
                        reader
                            .observe(file)
                            .podcast_hint
                            .as_deref()
                            .and_then(|hint| gate(hint, &library))
                    }),
            };
            if current.as_ref().map(|i| i.podcast.id) != chosen {
                current = match chosen {
                    Some(id) => self.podcast_index(id).await?,
                    None => None,
                };
            }
            let (mut candidate, embedded_guid) = blocking({
                let (root, file) = (root.clone(), file.clone());
                move || observe(format, &root, &file)
            })
            .await?;
            let item = match &current {
                Some(index) => {
                    let named = record
                        .and_then(|r| recorded_episode(r, index, &mut candidate))
                        .or_else(|| {
                            embedded_guid
                                .and_then(|guid| index.by_guid.get(&guid).copied())
                                .map(|id| (id, MatchedBy::EmbeddedGuid))
                        });
                    self.plan_one(
                        &media_root,
                        &candidate,
                        index,
                        named,
                        threshold,
                        &mut planned,
                    )
                    .await?
                }
                None => PlanItem {
                    source_path: file.relative.as_str().to_owned(),
                    size_bytes: file.size_bytes,
                    action: Action::Unmatched,
                    podcast_id: None,
                    episode_id: None,
                    confidence: 0,
                    matched_by: None,
                    target_path: None,
                    detail: Some(record.map_or_else(
                        || {
                            "no podcast in the library matches this directory closely enough"
                                .to_owned()
                        },
                        |r| {
                            format!(
                                "Podgrab's podcast {} is not in the library; import Podgrab's OPML export first",
                                r.feed_url.as_ref().map_or(r.podcast_title.as_str(), Url::as_str)
                            )
                        },
                    )),
                },
            };
            items.push(item);
        }

        one_file_per_episode(&mut items, &files).await?;

        if options.apply {
            for (item, file) in items.iter_mut().zip(files.iter()) {
                if item.action == Action::Import {
                    self.execute_one(&media_root, item, file).await;
                }
            }
        }
        for item in &items {
            match item.action {
                Action::Import => counts.imported += 1,
                Action::AlreadyPresent => counts.already_present += 1,
                Action::Conflict => counts.conflicts += 1,
                Action::Ambiguous => counts.ambiguous += 1,
                Action::Unmatched => counts.unmatched += 1,
                Action::Invalid => counts.invalid += 1,
            }
        }

        let event = Event::now(
            options.podcast,
            None,
            EventKind::ArchiveImportCompleted {
                applied: options.apply,
                format: format.as_str().to_owned(),
                scanned: counts.scanned,
                imported: counts.imported,
                ambiguous: counts.ambiguous,
                unmatched: counts.unmatched,
                already_present: counts.already_present,
            },
        );
        let mut tx = self.storage().begin().await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));

        Ok(ImportPlan {
            applied: options.apply,
            source_root: root.display().to_string(),
            format,
            threshold,
            counts,
            items,
        })
    }

    /// Checks the tree a user pointed at and refuses the ones that would
    /// make an import mean something other than what it says.
    pub(crate) fn import_root(&self, source: &Path) -> Result<PathBuf, UguisuError> {
        let root = source.canonicalize().map_err(|e| {
            archive_error(
                ArchiveErrorKind::ImportSourceInvalid,
                format!("{}: {e}", source.display()),
            )
        })?;
        if !root.is_dir() {
            return Err(archive_error(
                ArchiveErrorKind::ImportSourceInvalid,
                format!("{} is not a directory", root.display()),
            ));
        }
        let media = canonical_enough(&self.media_root()?);
        if root == media || root.starts_with(&media) || media.starts_with(&root) {
            // Importing the archive into itself would copy every file to a
            // second path under a different name. Whatever a user meant by
            // it, it was not that.
            return Err(archive_error(
                ArchiveErrorKind::ImportSourceInvalid,
                format!(
                    "{} overlaps the media directory; an archive cannot be imported into itself",
                    root.display()
                ),
            ));
        }
        Ok(root)
    }

    /// Every podcast's identifier and title, for the directory gate.
    async fn podcast_titles(&self) -> Result<Vec<(PodcastId, String)>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(podcasts::list(&mut reader)
            .await?
            .into_iter()
            .map(|p| (p.id, p.title))
            .collect())
    }

    /// One podcast's episodes, loaded for matching.
    async fn podcast_index(&self, id: PodcastId) -> Result<Option<PodcastIndex>, UguisuError> {
        let mut conn = self.storage().reader().await?;
        let Some(podcast) = podcasts::get(&mut conn, id).await? else {
            return Ok(None);
        };
        let mut facts = Vec::new();
        // A GUID or an enclosure two episodes share names neither of them.
        let mut guids: HashMap<String, Option<EpisodeId>> = HashMap::new();
        let mut enclosures: HashMap<String, Option<EpisodeId>> = HashMap::new();
        let mut after = None;
        loop {
            let page = episodes::page(&mut conn, id, after, 500).await?;
            let Some(last) = page.last() else { break };
            after = Some((last.sort_at, last.id));
            for episode in page {
                if let Some(guid) = episode
                    .guid
                    .as_deref()
                    .map(str::trim)
                    .filter(|g| !g.is_empty())
                {
                    guids
                        .entry(guid.to_owned())
                        .and_modify(|seen| *seen = None)
                        .or_insert(Some(episode.id));
                }
                if let Some(key) = &episode.identity.enclosure_key {
                    enclosures
                        .entry(key.clone())
                        .and_modify(|seen| *seen = None)
                        .or_insert(Some(episode.id));
                }
                facts.push(EpisodeFacts {
                    id: episode.id,
                    title: episode.title.clone(),
                    published: episode.published_at.map(OffsetDateTime::date),
                    season: episode.season,
                    number: episode.episode_number,
                    duration_secs: episode.duration_secs,
                    // A declared length of 0 is a feed that did not know the
                    // size, not a size: scored, it would sink every match.
                    enclosure_bytes: episode
                        .primary_enclosure()
                        .and_then(|e| e.length_bytes)
                        .filter(|&n| n > 0),
                });
            }
        }
        Ok(Some(PodcastIndex {
            podcast,
            facts,
            by_guid: unique(guids),
            by_enclosure: unique(enclosures),
        }))
    }

    /// Decides what one source file would become.
    #[allow(clippy::too_many_lines)] // one verdict per file, its checks in the order they apply
    async fn plan_one(
        &self,
        media_root: &Path,
        candidate: &Candidate,
        index: &PodcastIndex,
        named: Option<(EpisodeId, MatchedBy)>,
        threshold: u32,
        planned: &mut HashMap<String, EpisodeId>,
    ) -> Result<PlanItem, UguisuError> {
        let podcast = &index.podcast;
        let mut item = PlanItem {
            source_path: candidate.relative.as_str().to_owned(),
            size_bytes: candidate.size_bytes,
            action: Action::Unmatched,
            podcast_id: Some(podcast.id),
            episode_id: None,
            confidence: 0,
            matched_by: None,
            target_path: None,
            detail: None,
        };
        if !candidate.looks_like_media {
            item.action = Action::Invalid;
            item.detail = Some("the first bytes are not a media container".to_owned());
            return Ok(item);
        }

        let verdict = decide(candidate, &index.facts, named, threshold);
        let (episode_id, confidence, matched_by) = match &verdict {
            Verdict::Matched {
                episode_id,
                confidence,
                matched_by,
                ..
            } => (*episode_id, *confidence, matched_by.as_str()),
            Verdict::Ambiguous { top, detail } => {
                item.action = Action::Ambiguous;
                item.confidence = top.first().map_or(0, |s| s.confidence);
                item.detail = Some((*detail).to_owned());
                return Ok(item);
            }
            Verdict::Unmatched { best } => {
                item.confidence = best.as_ref().map_or(0, |s| s.confidence);
                item.detail = Some("no episode explains the file well enough".to_owned());
                return Ok(item);
            }
        };
        item.episode_id = Some(episode_id);
        item.confidence = confidence;
        item.matched_by = Some(matched_by.to_owned());

        let (_, episode, existing) = self.archive_subject(episode_id).await?;
        if let Some(archived) = existing {
            return already_archived(media_root, item, candidate, &archived).await;
        }
        // The import cannot stop a job that is already queued, and that job
        // would fetch the file again once the workers run.
        let queued = self
            .downloads()
            .job_for_episode(episode_id)
            .await?
            .is_some_and(|job| {
                job.state.is_pending() || job.state == uguisu_core::download::DownloadState::Paused
            });
        let extension = candidate
            .relative
            .file_name()
            .rsplit_once('.')
            .map_or_else(|| "mp3".to_owned(), |(_, e)| e.to_ascii_lowercase());
        let rendered = self.render_path(podcast, &episode, &extension)?;

        // Before collision handling: are these exact bytes already at the
        // path the template asks for? That is what an import interrupted
        // between the rename and the registration leaves behind, and
        // routing around it would copy the file a second time under a
        // suffixed name - a duplicate, created by the recovery.
        if let Ok(full) = resolve_checked(media_root, &rendered)
            && full.is_file()
        {
            let source = candidate.absolute.clone();
            let (on_disk, source_hash) =
                blocking(move || (hash_file(&full), hash_file(&source))).await?;
            let on_disk = on_disk.map_err(|e| {
                archive_error(
                    ArchiveErrorKind::VerificationIo,
                    format!("{}: {e}", rendered.as_str()),
                )
            })?;
            let source_hash = source_hash.map_err(|e| {
                archive_error(
                    ArchiveErrorKind::ImportSourceInvalid,
                    format!("{}: {e}", candidate.relative.as_str()),
                )
            })?;
            if on_disk == source_hash {
                item.target_path = Some(rendered.as_str().to_owned());
                planned.insert(rendered.as_str().to_owned(), episode_id);
                item.action = Action::Import;
                let in_place = "the file is already in place; only the record is missing";
                item.detail = Some(if queued {
                    format!("{in_place}; {QUEUED_JOB}")
                } else {
                    in_place.to_owned()
                });
                return Ok(item);
            }
        }

        // Something else is at the natural path, or nothing is. The
        // collision handling picks a free one; when even the disambiguated
        // form is taken, that is a conflict and nothing is written.
        match self
            .place_target(media_root, &rendered, episode_id, planned)
            .await
        {
            Ok(target) => {
                planned.insert(target.as_str().to_owned(), episode_id);
                item.target_path = Some(target.as_str().to_owned());
                item.action = Action::Import;
                if queued {
                    item.detail = Some(QUEUED_JOB.to_owned());
                }
            }
            Err(detail) => {
                item.action = Action::Conflict;
                item.detail = Some(detail);
            }
        }
        Ok(item)
    }

    /// What the template produces, before collisions are considered.
    fn render_path(
        &self,
        podcast: &Podcast,
        episode: &Episode,
        extension: &str,
    ) -> Result<RelativePath, UguisuError> {
        let template = self.template()?;
        let ctx = Context::new(podcast, episode, extension);
        template
            .render(&ctx, self.config().archive.path_profile)
            .map_err(|e| archive_error(ArchiveErrorKind::TemplateInvalid, e.to_string()))
    }

    /// A free path for an episode, or why there is not one.
    ///
    /// The database's owners and the disk are both consulted, so a file
    /// Uguisu never wrote - the user's own, a leftover from another
    /// layout - is never overwritten either.
    async fn place_target(
        &self,
        media_root: &Path,
        rendered: &RelativePath,
        episode_id: EpisodeId,
        planned: &HashMap<String, EpisodeId>,
    ) -> Result<RelativePath, String> {
        let mut owners = self
            .path_owners(std::slice::from_ref(rendered), episode_id)
            .await
            .map_err(|e| e.to_string())?;
        let profile = self.config().archive.path_profile;
        let suffixed =
            collision::with_suffix(rendered, &collision::suffix_for(episode_id), profile);
        for path in std::iter::once(rendered).chain(suffixed.as_ref().ok()) {
            if let Some(owner) = planned.get(path.as_str()) {
                owners.entry(path.as_str().to_owned()).or_insert(*owner);
            }
        }
        collision::place(
            rendered,
            episode_id,
            self.config().archive.path_profile,
            &DbAndDisk {
                owners: &owners,
                root: media_root,
            },
        )
        .map(|placement| placement.path)
        .map_err(|e| e.to_string())
    }

    /// Copies one accepted file in and records it.
    ///
    /// Copy into Uguisu's own scratch directory under the media root while
    /// hashing, flush, then a rename into place - so the target path only
    /// ever holds a complete file, and the rename stays inside one file
    /// system even when the source was on another disk. A failure at any
    /// step downgrades the item and leaves the archive as it was.
    async fn execute_one(&self, media_root: &Path, item: &mut PlanItem, file: &ScannedFile) {
        let (Some(episode_id), Some(podcast_id), Some(target)) =
            (item.episode_id, item.podcast_id, item.target_path.clone())
        else {
            return;
        };
        match self
            .copy_and_register(media_root, item, (episode_id, podcast_id), &target, file)
            .await
        {
            Ok(()) => {}
            Err(e) => {
                item.action = Action::Conflict;
                item.detail = Some(e.to_string());
            }
        }
    }

    #[allow(clippy::too_many_lines)] // copy, rename, register: the order is the point
    async fn copy_and_register(
        &self,
        media_root: &Path,
        item: &PlanItem,
        (episode_id, podcast_id): (EpisodeId, PodcastId),
        target: &str,
        file: &ScannedFile,
    ) -> Result<(), UguisuError> {
        let target = RelativePath::parse(target)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let full = resolve_checked(media_root, &target)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;

        if self.archive_file(episode_id).await?.is_some() {
            return Err(archived_meanwhile(episode_id));
        }
        let (hash, size) = blocking({
            let (media_root, full, target, file) = (
                media_root.to_path_buf(),
                full.clone(),
                target.clone(),
                file.clone(),
            );
            move || land(&media_root, &full, &target, &file)
        })
        .await??;

        let now = OffsetDateTime::now_utc();
        let record = ArchiveFile {
            id: ArchiveFileId::new(),
            episode_id,
            podcast_id,
            relative_path: target.as_str().to_owned(),
            size_bytes: size,
            content_type: None,
            sniffed_type: None,
            hash_algo: "sha256".to_owned(),
            hash_value: hash.clone(),
            // The bytes as found in the source archive. There was no
            // download, so this is the only provenance there is.
            source_size_bytes: Some(size),
            source_hash_algo: Some("sha256".to_owned()),
            source_hash_value: Some(hash.clone()),
            mtime_unix: std::fs::metadata(&full).ok().and_then(|m| mtime_of(&m)),
            // Hashed while copying, so the record is true of the bytes that
            // landed - but it was not checked afterwards, and `verified` is
            // a verification's word to say.
            verification_state: uguisu_core::archive::VerificationState::Unchecked,
            verification_reason: Some(reason::IMPORTED.to_owned()),
            verified_at: None,
            origin: ArchiveOrigin::Import,
            tag_state: TagState::Untagged,
            tag_mode: None,
            tagged_at: None,
            sidecar_written_at: None,
            original_tags: None,
            source_changed_at: None,
            registered_at: now,
            created_at: now,
            updated_at: now,
        };

        let event = Event::now(
            Some(podcast_id),
            Some(episode_id),
            EventKind::ArchiveImported {
                archive_file_id: record.id,
                path: record.relative_path.clone(),
                confidence: item.confidence,
                matched_by: item.matched_by.clone().unwrap_or_default(),
            },
        );
        let mut tx = self.storage().begin().await?;
        // A download can finish while the file is being copied; its record
        // wins, and the copy stays where it is, reported.
        if archive_files::get_by_episode(&mut tx, episode_id)
            .await?
            .is_some()
        {
            return Err(archived_meanwhile(episode_id));
        }
        archive_files::upsert(&mut tx, &record).await?;
        crate::archive_meta::mark_stale(&mut tx, podcast_id, now).await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));

        if let Err(e) = self.write_sidecar(episode_id).await {
            tracing::warn!(episode = %episode_id, error = %e, "sidecar not written after import");
        }
        Ok(())
    }
}

/// One podcast's episodes, as an import matches files against them.
struct PodcastIndex {
    podcast: Podcast,
    facts: Vec<EpisodeFacts>,
    /// Episode GUIDs that exactly one episode carries.
    by_guid: HashMap<String, EpisodeId>,
    /// Normalized enclosure URLs that exactly one episode carries.
    by_enclosure: HashMap<String, EpisodeId>,
}

fn unique(keys: HashMap<String, Option<EpisodeId>>) -> HashMap<String, EpisodeId> {
    keys.into_iter()
        .filter_map(|(key, id)| Some((key, id?)))
        .collect()
}

/// Podgrab's downloaded episodes, by the last two components of the path
/// it wrote (ADR 0050). A path several rows share - Podgrab does not
/// download a file again when one is already there - names no row.
async fn podgrab_records(db: &Path) -> Result<HashMap<String, Option<PodgrabRecord>>, UguisuError> {
    let invalid = |detail: String| archive_error(ArchiveErrorKind::ImportSourceInvalid, detail);
    let db = db
        .canonicalize()
        .map_err(|e| invalid(format!("{}: {e}", db.display())))?;
    if !std::fs::metadata(&db).is_ok_and(|m| m.is_file()) {
        return Err(invalid(format!("{} is not a file", db.display())));
    }
    let records = uguisu_storage::podgrab::read(&db).await.map_err(|e| {
        invalid(format!(
            "{} is not a Podgrab database Uguisu can read: {e}",
            db.display()
        ))
    })?;
    let mut index: HashMap<String, Option<PodgrabRecord>> = HashMap::new();
    for record in records {
        index
            .entry(path_tail(&record.download_path))
            .and_modify(|shared| *shared = None)
            .or_insert(Some(record));
    }
    Ok(index)
}

/// The last two components of a path, `/` or `\` separated: the podcast
/// folder and the file, wherever the tree was mounted.
fn path_tail(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    parts[parts.len().saturating_sub(2)..].join("/")
}

/// The podcast a Podgrab record belongs to: its feed URL, current or
/// former, else a podcast titled exactly as Podgrab titled it.
fn podcast_of(
    record: &PodgrabRecord,
    feeds: &HashMap<String, PodcastId>,
    library: &[(PodcastId, String)],
) -> Option<PodcastId> {
    if let Some(id) = record
        .feed_url
        .as_ref()
        .and_then(|u| feeds.get(&feed_key(u)))
    {
        return Some(*id);
    }
    let mut titled = library
        .iter()
        .filter(|(_, title)| *title == record.podcast_title);
    match (titled.next(), titled.next()) {
        (Some((id, _)), None) => Some(*id),
        _ => None,
    }
}

/// The episode a Podgrab record names by its GUID or its enclosure. When it
/// names none, the record's title and date - the feed's own, of which the
/// file name is a mangled copy - are what the file is scored with.
fn recorded_episode(
    record: &PodgrabRecord,
    index: &PodcastIndex,
    candidate: &mut Candidate,
) -> Option<(EpisodeId, MatchedBy)> {
    let named = record
        .guid
        .as_deref()
        .and_then(|guid| index.by_guid.get(guid.trim()))
        .or_else(|| {
            record
                .enclosure_url
                .as_ref()
                .and_then(|url| index.by_enclosure.get(&normalize_enclosure_url(url)))
        });
    if let Some(id) = named {
        return Some((*id, MatchedBy::SourceDatabase));
    }
    if !record.title.trim().is_empty()
        && let Some(name) = candidate.title.replace(record.title.clone())
    {
        candidate.fallback_titles.insert(0, name);
    }
    if let Some(published) = record.published_at {
        candidate.published = Some(published.date());
    }
    None
}

/// What one source file says about itself: what its layout reads from its
/// name and side files, and what its own tags add (ADR 0050). The tag
/// title is a fallback, the audio's length fills a gap, and the embedded
/// GUID is returned for the caller to look up. A file whose tags cannot be
/// read is observed exactly as before.
fn observe(format: ImportFormat, root: &Path, file: &ScannedFile) -> (Candidate, Option<String>) {
    let reader = format.reader();
    let mut candidate = reader.observe(file);
    for side in reader.side_files(file) {
        if let Some(text) = read_side_file(root, &side) {
            reader.refine(&mut candidate, &text);
        }
    }
    if !candidate.looks_like_media {
        return (candidate, None);
    }
    let Ok(identity) = uguisu_metadata::read_identity(&file.absolute) else {
        return (candidate, None);
    };
    if candidate.duration_secs.is_none() {
        candidate.duration_secs = identity.duration_secs;
    }
    if let Some(title) = identity.title
        && candidate.title.as_ref() != Some(&title)
    {
        candidate.fallback_titles.push(title);
    }
    (candidate, identity.episode_guid)
}

/// The verdict for one file, given the episode something else named.
///
/// A tag can be stale - copied from another episode by the publisher's
/// tooling - so an embedded GUID names the episode only when the name and
/// the dates do not clearly name a different one; when they do, the file
/// does not identify either.
fn decide(
    candidate: &Candidate,
    episodes: &[EpisodeFacts],
    named: Option<(EpisodeId, MatchedBy)>,
    threshold: u32,
) -> Verdict {
    match named {
        Some((id, MatchedBy::EmbeddedGuid)) => match classify(candidate, episodes, threshold) {
            Verdict::Matched { episode_id, .. } if episode_id != id => Verdict::Ambiguous {
                top: Vec::new(),
                detail: "the embedded episode GUID names another episode than the name does",
            },
            _ => exactly(candidate, episodes, id, MatchedBy::EmbeddedGuid),
        },
        Some((id, by)) => exactly(candidate, episodes, id, by),
        None => classify(candidate, episodes, threshold),
    }
}

/// The verdict for a file whose episode is already known.
fn exactly(
    candidate: &Candidate,
    episodes: &[EpisodeFacts],
    id: EpisodeId,
    by: MatchedBy,
) -> Verdict {
    let named = Candidate {
        exact_match: Some((id, by)),
        ..candidate.clone()
    };
    classify(&named, episodes, 100)
}

/// Which podcast a directory name belongs to, if one clearly does.
///
/// The same threshold-and-margin shape as episode matching, for the same
/// reason: two shows whose titles are equally close to a directory name
/// mean the name does not identify either of them.
fn gate(hint: &str, library: &[(PodcastId, String)]) -> Option<PodcastId> {
    let mut scored: Vec<(u32, PodcastId)> = library
        .iter()
        .map(|(id, title)| (matching::title_similarity(hint, title), *id))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let (best, id) = *scored.first()?;
    if best < PODCAST_GATE {
        return None;
    }
    let runner_up = scored.get(1).map_or(0, |s| s.0);
    (best.saturating_sub(runner_up) >= PODCAST_MARGIN).then_some(id)
}

/// Runs file work on the blocking pool: an import reads and copies whole
/// archives, and on a runtime worker that would stall every request the
/// worker also serves.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, UguisuError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| UguisuError::Internal(format!("import task failed: {e}")))
}

/// The plan's note on a file whose episode a download job would fetch again.
const QUEUED_JOB: &str =
    "a download of this episode is still queued or paused; cancel it, or it fetches the file again";

/// Puts the source's bytes at the target, returning their hash and size.
///
/// Copy into Uguisu's own scratch directory under the media root while
/// hashing, flush, then a rename into place - so the target path only
/// ever holds a complete file, and the rename stays inside one file system
/// even when the source was on another disk.
fn land(
    media_root: &Path,
    full: &Path,
    target: &RelativePath,
    file: &ScannedFile,
) -> Result<(String, u64), UguisuError> {
    if full.is_file() {
        // Recovery: the bytes are already in place from an interrupted
        // run, and only the record is missing. Re-hash what is there
        // rather than copying it again - and only adopt it when it is
        // the source's bytes, not whatever happens to sit at the path.
        let hash = hash_file(full).map_err(|e| {
            archive_error(ArchiveErrorKind::VerificationIo, format!("{target}: {e}"))
        })?;
        let source_hash = hash_file(&file.absolute).map_err(|e| {
            archive_error(
                ArchiveErrorKind::ImportSourceInvalid,
                format!("{}: {e}", file.relative.as_str()),
            )
        })?;
        if hash != source_hash {
            return Err(archive_error(
                ArchiveErrorKind::PathCollision,
                format!("{target} holds other bytes than {}", file.relative.as_str()),
            ));
        }
        let size = std::fs::metadata(full).map(|m| m.len()).unwrap_or_default();
        Ok((hash, size))
    } else {
        let scratch = layout::tmp_path(&format!("{}.import", ArchiveFileId::new()));
        let scratch_full = resolve_checked(media_root, &scratch)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        let _held = layout::Scratch::hold(&scratch_full);
        if let Some(parent) = scratch_full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                archive_error(
                    ArchiveErrorKind::VerificationIo,
                    format!("{}: {e}", parent.display()),
                )
            })?;
        }
        let copied = copy_and_hash(&file.absolute, &scratch_full).map_err(|e| {
            archive_error(
                ArchiveErrorKind::ImportSourceInvalid,
                format!("{}: {e}", file.relative.as_str()),
            )
        })?;
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                archive_error(
                    ArchiveErrorKind::VerificationIo,
                    format!("{}: {e}", parent.display()),
                )
            })?;
        }
        // The target must still be free, in any case the file system folds.
        // Anything else and the scratch file stays where it is, to be
        // reported rather than silently dropped on someone's data.
        uguisu_download::paths::rename_new(&scratch_full, full).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                archive_error(
                    ArchiveErrorKind::PathCollision,
                    format!("{target} was taken while the file was being copied"),
                )
            } else {
                archive_error(ArchiveErrorKind::VerificationIo, format!("{target}: {e}"))
            }
        })?;
        crate::archive::sync_dir(full.parent());
        Ok(copied)
    }
}

/// Settles the files that matched one episode between them (ADR 0050).
///
/// Identical bytes are one file seen twice: the first is imported and the
/// rest are already present. Different bytes mean the files do not
/// identify the episode between them, and choosing one would be a guess,
/// so none is imported.
async fn one_file_per_episode(
    items: &mut [PlanItem],
    files: &[ScannedFile],
) -> Result<(), UguisuError> {
    let mut by_episode: HashMap<EpisodeId, Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        if item.action == Action::Import
            && let Some(episode) = item.episode_id
        {
            by_episode.entry(episode).or_default().push(index);
        }
    }
    for group in by_episode.into_values().filter(|g| g.len() > 1) {
        let sources: Vec<PathBuf> = group.iter().map(|&i| files[i].absolute.clone()).collect();
        let hashes: Vec<Option<String>> =
            blocking(move || sources.iter().map(|path| hash_file(path).ok()).collect()).await?;
        let first = group[0];
        let identical = hashes.iter().all(|h| h.is_some() && *h == hashes[0]);
        if identical {
            let first_path = items[first].source_path.clone();
            for &i in &group[1..] {
                items[i].action = Action::AlreadyPresent;
                items[i].detail = Some(format!("the same bytes as {first_path}"));
            }
        } else {
            let paths: Vec<String> = group
                .iter()
                .map(|&i| items[i].source_path.clone())
                .collect();
            for &i in &group {
                items[i].action = Action::Conflict;
                items[i].detail = Some(format!(
                    "{} different files match this episode ({}); none was imported",
                    group.len(),
                    paths.join(", ")
                ));
            }
        }
    }
    Ok(())
}

/// What a file matched to an archived episode becomes. Never registered
/// again: the record says where the episode's bytes are and what they were,
/// and an import that rewrote it would lose a download's provenance
/// (ADR 0050).
async fn already_archived(
    media_root: &Path,
    mut item: PlanItem,
    candidate: &Candidate,
    archived: &ArchiveFile,
) -> Result<PlanItem, UguisuError> {
    item.target_path = Some(archived.relative_path.clone());
    if let Some(exact) = same_bytes(candidate, archived).await? {
        item.action = Action::AlreadyPresent;
        let gone = RelativePath::parse(&archived.relative_path)
            .ok()
            .and_then(|p| resolve_checked(media_root, &p).ok())
            .is_some_and(|full| {
                std::fs::symlink_metadata(full)
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            });
        item.detail = Some(
            match (gone, exact) {
                (false, _) => "the archive already holds these bytes",
                (true, true) => {
                    "the archived file is missing; `archive restore` puts these exact bytes back (ADR 0060)"
                }
                (true, false) => {
                    "the archived file is missing; these are its bytes from before Uguisu wrote tags, which `archive restore` does not use"
                }
            }
            .to_owned(),
        );
    } else {
        item.action = Action::Conflict;
        item.detail = Some(format!(
            "the episode is already archived at {}; this file differs",
            archived.relative_path
        ));
    }
    Ok(item)
}

/// Whether a source file is the archived episode's bytes: `Some(true)` as
/// they are now, `Some(false)` only as they were received. The sizes are
/// compared first, so a file that cannot be the same is never read.
async fn same_bytes(
    candidate: &Candidate,
    archived: &ArchiveFile,
) -> Result<Option<bool>, UguisuError> {
    if candidate.size_bytes != archived.size_bytes
        && archived.source_size_bytes != Some(candidate.size_bytes)
    {
        return Ok(None);
    }
    let source = candidate.absolute.clone();
    let hash = blocking(move || hash_file(&source)).await?.map_err(|e| {
        archive_error(
            ArchiveErrorKind::ImportSourceInvalid,
            format!("{}: {e}", candidate.relative.as_str()),
        )
    })?;
    Ok(if hash == archived.hash_value {
        Some(true)
    } else {
        (archived.source_hash_value.as_deref() == Some(hash.as_str())).then_some(false)
    })
}

fn archived_meanwhile(episode_id: EpisodeId) -> UguisuError {
    archive_error(
        ArchiveErrorKind::PathCollision,
        format!("episode {episode_id} was archived while this file was being imported"),
    )
}

/// Reads a side-metadata file from the source tree, bounded.
///
/// It is a stranger's file and it is only ever used to fill gaps, so a
/// large one is skipped rather than read.
fn read_side_file(root: &Path, relative: &RelativePath) -> Option<String> {
    let full = resolve_checked(root, relative).ok()?;
    let meta = std::fs::metadata(&full).ok()?;
    if !meta.is_file() || meta.len() > MAX_SIDE_BYTES {
        return None;
    }
    std::fs::read_to_string(&full).ok()
}

/// Copies a file while hashing it, and flushes the copy to disk.
pub(crate) fn copy_and_hash(from: &Path, to: &Path) -> std::io::Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};

    let mut source = std::fs::File::open(from)?;
    let mut target = std::fs::File::create(to)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        target.write_all(&buffer[..read])?;
        total += read as u64;
    }
    target.flush()?;
    target.sync_all()?;
    Ok((hex::encode(hasher.finalize()), total))
}

fn mtime_of(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

/// The media root, canonicalised as far as it exists.
///
/// A plain `canonicalize` fails when the directory has not been created
/// yet — which is the state of every fresh install — and a guard that is
/// skipped on failure is not a guard. So the deepest existing ancestor is
/// resolved and the rest is appended lexically, which is the same thing
/// `resolve_checked` does for archive paths and for the same reason: the
/// answer has to exist before the directory does.
fn canonical_enough(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut probe = path;
    loop {
        if let Ok(found) = probe.canonicalize() {
            let mut out = found;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (probe.file_name(), probe.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_owned());
                probe = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}
