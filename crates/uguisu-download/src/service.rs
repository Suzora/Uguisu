//! The download service: enqueueing, user commands, listing, global
//! control, startup reconciliation and the lifecycle of the scheduler.
//!
//! User commands write the database first and only then stop a running
//! worker; the worker's later compare-and-set fails and it merely records
//! the bytes it kept (ADR 0018).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::Notify;
use uguisu_core::UguisuError;
use uguisu_core::archive::policy_reason;
use uguisu_core::download::{
    DownloadAttempt, DownloadControl, DownloadJob, DownloadState, JobSummary, PauseAllReason,
    Priority, ProgressSnapshot,
};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::{EpisodeId, JobId, PodcastId};
use uguisu_core::model::{ArchiveState, Episode};
use uguisu_core::page;
use uguisu_http::HostKey;
use uguisu_storage::downloads::{self, Transition};
use uguisu_storage::{
    SqliteConnection, StorageError, archive_files, episodes, events as event_repo, podcasts,
};

use crate::deps::Deps;
use crate::deps::DestinationRequest;
use crate::handle::{JobHandle, StopReason};
use crate::paths::{self, Destination, TMP_DIR, extension_for};
use crate::state::{JobEvent, reason, transition};

/// Largest page of jobs.
pub const MAX_PAGE: u32 = uguisu_core::page::MAX;

/// What an enqueue did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "outcome", content = "job", rename_all = "snake_case")]
pub enum EnqueueOutcome {
    /// A new job was queued.
    Created(DownloadJob),
    /// The episode already has a pending or paused job.
    Existing(DownloadJob),
    /// A failed or cancelled job, or a completed one by a redownload, was
    /// queued again with a fresh budget.
    Requeued(DownloadJob),
    /// The episode is already downloaded.
    AlreadyCompleted(DownloadJob),
}

impl EnqueueOutcome {
    /// The job in every case.
    #[must_use]
    pub const fn job(&self) -> &DownloadJob {
        match self {
            Self::Created(j)
            | Self::Existing(j)
            | Self::Requeued(j)
            | Self::AlreadyCompleted(j) => j,
        }
    }

    /// Whether a job was created or re-queued (a `download.queued` event fired).
    #[must_use]
    pub const fn queued(&self) -> bool {
        matches!(self, Self::Created(_) | Self::Requeued(_))
    }
}

/// An episode a bulk enqueue left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkippedEpisode {
    /// The episode.
    pub episode_id: EpisodeId,
    /// `no_enclosure`, `duplicate_candidate`, `skipped`, `removed_from_feed`.
    pub reason: String,
}

/// Result of a bulk enqueue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EnqueueSummary {
    /// New jobs.
    pub created: u32,
    /// Episodes that already had a pending or paused job.
    pub existing: u32,
    /// Failed/cancelled jobs queued again.
    pub requeued: u32,
    /// Episodes already downloaded.
    pub completed: u32,
    /// Episodes left out, with reasons.
    pub skipped: Vec<SkippedEpisode>,
}

/// Listing filter and page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobFilter {
    /// Only this state.
    pub state: Option<DownloadState>,
    /// Only this podcast.
    pub podcast_id: Option<PodcastId>,
    /// Keyset cursor: the last job of the previous page.
    pub after: Option<JobId>,
    /// Page size (clamped to [`MAX_PAGE`]).
    pub limit: u32,
}

/// One page of jobs, newest first.
// Not `Eq`: a summary carries a percentage, which is a float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobPage {
    /// Jobs, each with the episode and podcast it belongs to named.
    pub jobs: Vec<JobSummary>,
    /// Cursor for the next page, when there may be more.
    pub next_after: Option<JobId>,
}

/// A job with its history and live progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobDetail {
    /// The job.
    pub job: DownloadJob,
    /// Attempts, oldest first.
    pub attempts: Vec<DownloadAttempt>,
    /// Live progress when a worker holds the job in this process.
    pub progress: Option<ProgressSnapshot>,
}

/// Queue statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DownloadStats {
    /// Jobs per state.
    pub by_state: BTreeMap<DownloadState, u64>,
    /// Jobs held by workers in this process.
    pub running: u32,
    /// Whether the scheduler runs in this process.
    pub workers_started: bool,
    /// Global pause reason, when paused.
    pub paused_all: Option<PauseAllReason>,
    /// Earliest scheduled retry.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_retry_at: Option<OffsetDateTime>,
    /// Orphan `.part` files found by the last reconciliation.
    pub orphan_parts: u32,
}

/// What startup reconciliation found and did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReconcileReport {
    /// `downloading` jobs re-queued as `queued(recovered)`.
    pub recovered: u32,
    /// `finalizing` jobs whose target was in place or could be renamed now.
    pub finalized: u32,
    /// `finalizing` jobs with neither `.part` nor target.
    pub finalization_lost: u32,
    /// `finalizing` jobs failed as `target_exists`: what is at the target
    /// is not the file they downloaded, and both it and the `.part` stay.
    #[serde(default)]
    pub finalization_conflicts: u32,
    /// `finalizing` jobs failed as `finalization`: the `.part` is there, but
    /// renaming it failed again for another reason, and it stays.
    #[serde(default)]
    pub finalization_failed: u32,
    /// `.part` files under `.uguisu-tmp` that belong to no job (relative paths).
    pub orphan_parts: Vec<String>,
    /// Deep only: completed jobs whose target is missing.
    pub missing_targets: u32,
    /// Deep only: pending jobs whose target already exists.
    pub unexpected_targets: u32,
    /// `retrying` jobs left alone.
    pub kept_retrying: u32,
    /// `paused` jobs left alone.
    pub kept_paused: u32,
    /// The global pause in force, if any.
    pub paused_all: Option<PauseAllReason>,
}

pub(crate) struct Inner {
    pub(crate) deps: Deps,
    pub(crate) running: Mutex<HashMap<JobId, (Arc<JobHandle>, tokio::task::JoinHandle<()>)>>,
    pub(crate) wake: Notify,
    pub(crate) idle: Notify,
    pub(crate) claim_lock: tokio::sync::Mutex<()>,
    scheduler: Mutex<Option<tokio::task::JoinHandle<()>>>,
    started: AtomicBool,
    /// Set by `shutdown` before anything is cancelled so the scheduler
    /// claims no further job (a job claimed after the running set was
    /// drained would outlive the shutdown and hit a closed pool).
    pub(crate) stopping: AtomicBool,
    last_reconcile: Mutex<Option<ReconcileReport>>,
}

/// The queue's public face; cheap to clone.
#[derive(Clone)]
pub struct DownloadService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DownloadService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadService")
            .field("running", &self.running_count())
            .field("started", &self.inner.started.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

fn storage_err(e: StorageError) -> UguisuError {
    e.into()
}

impl DownloadService {
    /// Builds the service; workers start with [`start`](Self::start).
    #[must_use]
    pub fn new(deps: Deps) -> Self {
        Self {
            inner: Arc::new(Inner {
                deps,
                running: Mutex::new(HashMap::new()),
                wake: Notify::new(),
                idle: Notify::new(),
                claim_lock: tokio::sync::Mutex::new(()),
                scheduler: Mutex::new(None),
                started: AtomicBool::new(false),
                stopping: AtomicBool::new(false),
                last_reconcile: Mutex::new(None),
            }),
        }
    }

    /// The dependencies.
    #[must_use]
    pub fn deps(&self) -> &Deps {
        &self.inner.deps
    }

    /// Starts the scheduler (idempotent).
    pub fn start(&self) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.stopping.store(false, Ordering::SeqCst);
        let inner = Arc::clone(&self.inner);
        let task = tokio::spawn(crate::scheduler::run(inner));
        *lock(&self.inner.scheduler) = Some(task);
        tracing::info!(
            global = self.inner.deps.config.global_concurrency,
            per_host = self.inner.deps.config.per_host_concurrency,
            "download workers started"
        );
    }

    /// Whether the scheduler runs.
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.inner.started.load(Ordering::SeqCst)
    }

    /// Holds the claim lock, so a test can keep open the window between
    /// a claim and its worker being recorded.
    #[cfg(any(test, feature = "testing"))]
    pub async fn hold_claims(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.claim_lock.lock().await
    }

    /// Jobs held by workers right now.
    #[must_use]
    pub fn running_count(&self) -> usize {
        lock(&self.inner.running).len()
    }

    /// Whether no worker holds a job, nothing is claimable and no retry is
    /// pending (a paused queue counts as idle).
    ///
    /// Serialised against a claim in progress. The scheduler commits
    /// `downloading` and *then* records the worker, both under
    /// `claim_lock`; without taking it, this could look in between and
    /// find an empty map and nothing claimable — an idle-looking queue
    /// with a transfer about to start. `download run --wait` returned
    /// there, and the shutdown/restart test failed on it under load.
    ///
    /// Deliberately not "no job is in `downloading`": a worker that died
    /// leaves one behind, and a queue that can never be idle again until
    /// something reconciles it is worse than the race.
    pub async fn is_idle(&self) -> Result<bool, UguisuError> {
        let _serialised = self.inner.claim_lock.lock().await;
        if self.running_count() > 0 {
            return Ok(false);
        }
        let mut r = self
            .inner
            .deps
            .storage
            .reader()
            .await
            .map_err(storage_err)?;
        let control = downloads::control_get(&mut r).await.map_err(storage_err)?;
        if control.paused {
            return Ok(true);
        }
        let next = downloads::select_claimable(&mut r, OffsetDateTime::now_utc(), &[])
            .await
            .map_err(storage_err)?;
        if next.is_some() {
            return Ok(false);
        }
        let pending_retry = downloads::next_retry_due(&mut r)
            .await
            .map_err(storage_err)?;
        Ok(pending_retry.is_none())
    }

    /// Waits until [`is_idle`](Self::is_idle) holds (polling the queue
    /// state on every worker exit and at least every second).
    pub async fn wait_idle(&self) -> Result<(), UguisuError> {
        loop {
            if self.is_idle().await? {
                return Ok(());
            }
            tokio::select! {
                () = self.inner.idle.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    /// Stops the scheduler and the workers: running jobs are parked as
    /// `queued(shutdown)` (finalization is allowed to finish), tasks that
    /// outlive `grace` are aborted and recovered at the next start.
    pub async fn shutdown(&self, grace: Duration) {
        // 1. No further claims: the scheduler sees `stopping`, finishes the
        //    claim it may be in the middle of and returns.
        self.inner.stopping.store(true, Ordering::SeqCst);
        self.inner.wake.notify_one();
        let scheduler = lock(&self.inner.scheduler).take();
        if let Some(task) = scheduler
            && tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .is_err()
        {
            tracing::warn!("download scheduler did not stop within 5 s");
        }
        // 2. Every running job is told why it stops before its token fires,
        //    so each one parks itself as `queued(shutdown)`.
        let handles: Vec<(JobId, Arc<JobHandle>, tokio::task::JoinHandle<()>)> =
            lock(&self.inner.running)
                .drain()
                .map(|(id, (h, t))| (id, h, t))
                .collect();
        for (_, h, _) in &handles {
            h.stop(StopReason::Shutdown);
        }
        self.inner.deps.shutdown.cancel();
        let deadline = tokio::time::Instant::now() + grace;
        for (id, _, task) in handles {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if tokio::time::timeout(left, task).await.is_err() {
                tracing::warn!(job = %id, "worker did not stop within the grace period; it will be recovered at the next start");
            }
        }
        self.inner.started.store(false, Ordering::SeqCst);
        tracing::info!("download workers stopped");
    }

    // ---- enqueue
    /// Queues an episode's primary enclosure (idempotent per episode).
    pub async fn enqueue_episode(
        &self,
        episode_id: EpisodeId,
        priority: Priority,
    ) -> Result<EnqueueOutcome, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.inner.deps.storage.begin().await.map_err(storage_err)?;
        let episode = episodes::get(&mut tx, episode_id)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "episode".to_owned(),
                id: episode_id.to_string(),
            })?;
        if let Some(refused) = refusal_error(&episode) {
            return Err(refused);
        }
        if archived_without_its_download(&mut tx, &self.inner.deps.media_dir, episode.id).await? {
            return Err(UguisuError::Conflict(ALREADY_ARCHIVED.to_owned()));
        }
        let (outcome, events) = self.enqueue_in(&mut tx, &episode, priority, now).await?;
        tx.commit().await.map_err(|e| storage_err(e.into()))?;
        self.inner.deps.sink.publish(&events);
        if outcome.queued() {
            self.inner.wake.notify_one();
        }
        Ok(outcome)
    }

    /// Downloads an archived episode again because its file is gone (ADR 0060).
    ///
    /// Refused unless the episode has an archive record and nothing is at the
    /// record's path now: a present file is never replaced, whatever state its
    /// record is in. A completed job starts over from the episode's current
    /// enclosure; a record an import or a rebuild made gets a job of its own.
    /// When the download completes, its registration takes the record over.
    pub async fn redownload(
        &self,
        episode_id: EpisodeId,
        priority: Priority,
    ) -> Result<EnqueueOutcome, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.inner.deps.storage.begin().await.map_err(storage_err)?;
        let episode = episodes::get(&mut tx, episode_id)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "episode".to_owned(),
                id: episode_id.to_string(),
            })?;
        if let Some(refused) = refusal_error(&episode) {
            return Err(refused);
        }
        let record = archive_files::get_by_episode(&mut tx, episode_id)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| {
                UguisuError::Conflict("episode has no archived file to download again".to_owned())
            })?;
        if !record_file_gone(&self.inner.deps.media_dir, &record.relative_path) {
            return Err(UguisuError::Conflict(format!(
                "{} is still there, or cannot be checked; a download never replaces an archived file",
                record.relative_path
            )));
        }
        let existing = downloads::get_job_by_episode(&mut tx, episode_id)
            .await
            .map_err(storage_err)?;
        let (outcome, events) = match existing {
            Some(job) if job.state == DownloadState::Completed => {
                // The job writes its `.part` again. After a finalization that
                // could not take the old name back (`paths::rename_new`), that
                // name is a second link to the archived bytes, perhaps the only
                // one left, and the worker would truncate it.
                if !record_file_gone(&self.inner.deps.media_dir, &job.part_path) {
                    return Err(UguisuError::Conflict(format!(
                        "{} may hold the archived bytes; move it out of the media directory before downloading again",
                        job.part_path
                    )));
                }
                self.requeue_completed(&mut tx, &episode, job, priority, now)
                    .await?
            }
            _ => self.enqueue_in(&mut tx, &episode, priority, now).await?,
        };
        tx.commit().await.map_err(|e| storage_err(e.into()))?;
        self.inner.deps.sink.publish(&events);
        if outcome.queued() {
            self.inner.wake.notify_one();
        }
        Ok(outcome)
    }

    /// Points a completed job at the episode's current enclosure and target
    /// and queues it again.
    async fn requeue_completed(
        &self,
        conn: &mut SqliteConnection,
        episode: &Episode,
        job: DownloadJob,
        priority: Priority,
        now: OffsetDateTime,
    ) -> Result<(EnqueueOutcome, Vec<Event>), UguisuError> {
        transition(job.state, JobEvent::Redownload)
            .map_err(|e| UguisuError::Conflict(e.to_string()))?;
        let enclosure = episode
            .primary_enclosure()
            .ok_or_else(|| UguisuError::Invalid("episode has no enclosure".to_owned()))?;
        let host = HostKey::of(&enclosure.url)
            .map_err(|e| UguisuError::Invalid(format!("enclosure url: {e}")))?;
        downloads::set_source(
            conn,
            job.id,
            &enclosure.url,
            host.as_str(),
            Some(enclosure.id),
            now,
        )
        .await
        .map_err(storage_err)?;
        downloads::reset_transfer(conn, job.id, now)
            .await
            .map_err(storage_err)?;
        // The enclosure may now be another type: the target is rendered for
        // it again, and the registration moves the record when it differs.
        let ext = extension_for(enclosure.mime_type.as_deref(), &enclosure.url);
        let target = match self.resolve_target(conn, episode, &ext).await? {
            Some(target) => target,
            None => Destination::for_job(episode.podcast_id, episode.id, job.id, &ext).target,
        };
        downloads::set_target_path(conn, episode.id, &target, now)
            .await
            .map_err(storage_err)?;
        let requeued = downloads::requeue(
            conn,
            job.id,
            &[DownloadState::Completed],
            reason::REDOWNLOAD,
            Some(priority),
            now,
        )
        .await
        .map_err(storage_err)?;
        if !requeued {
            return Err(UguisuError::Conflict(format!(
                "job {} changed while it was queued again",
                job.id
            )));
        }
        episodes::set_archive_state(conn, episode.id, ArchiveState::Queued, None, now)
            .await
            .map_err(storage_err)?;
        let job = downloads::get_job(conn, job.id)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| UguisuError::Internal("job vanished".to_owned()))?;
        let ev = Event::now(
            Some(job.podcast_id),
            Some(job.episode_id),
            EventKind::DownloadQueued {
                job_id: job.id,
                enclosure_url: job.source_url.clone(),
                priority: job.priority,
                requeued: true,
            },
        );
        event_repo::insert_all(conn, std::slice::from_ref(&ev))
            .await
            .map_err(storage_err)?;
        Ok((EnqueueOutcome::Requeued(job), vec![ev]))
    }

    /// Queues every downloadable episode of a podcast, newest first.
    pub async fn enqueue_podcast(
        &self,
        podcast_id: PodcastId,
        priority: Priority,
    ) -> Result<EnqueueSummary, UguisuError> {
        {
            let mut r = self
                .inner
                .deps
                .storage
                .reader()
                .await
                .map_err(storage_err)?;
            podcasts::get(&mut r, podcast_id)
                .await
                .map_err(storage_err)?
                .ok_or_else(|| UguisuError::NotFound {
                    entity: "podcast".to_owned(),
                    id: podcast_id.to_string(),
                })?;
        }
        let mut summary = EnqueueSummary::default();
        let mut after: Option<(OffsetDateTime, EpisodeId)> = None;
        loop {
            let now = OffsetDateTime::now_utc();
            let mut tx = self.inner.deps.storage.begin().await.map_err(storage_err)?;
            let page = episodes::page(&mut tx, podcast_id, after, MAX_PAGE)
                .await
                .map_err(storage_err)?;
            if page.is_empty() {
                break;
            }
            let mut events = Vec::new();
            for episode in &page {
                if let Some(why) = refusal(episode) {
                    summary.skipped.push(SkippedEpisode {
                        episode_id: episode.id,
                        reason: why.to_owned(),
                    });
                    continue;
                }
                if archived_without_its_download(&mut tx, &self.inner.deps.media_dir, episode.id)
                    .await?
                {
                    summary.skipped.push(SkippedEpisode {
                        episode_id: episode.id,
                        reason: policy_reason::ALREADY_ARCHIVED.to_owned(),
                    });
                    continue;
                }
                let (outcome, evs) = self.enqueue_in(&mut tx, episode, priority, now).await?;
                events.extend(evs);
                match outcome {
                    EnqueueOutcome::Created(_) => summary.created += 1,
                    EnqueueOutcome::Existing(_) => summary.existing += 1,
                    EnqueueOutcome::Requeued(_) => summary.requeued += 1,
                    EnqueueOutcome::AlreadyCompleted(_) => summary.completed += 1,
                }
            }
            let last = page.last().map(|e| (e.sort_at, e.id));
            let done = page.len() < MAX_PAGE as usize;
            tx.commit().await.map_err(|e| storage_err(e.into()))?;
            self.inner.deps.sink.publish(&events);
            after = last;
            if done {
                break;
            }
        }
        if summary.created + summary.requeued > 0 {
            self.inner.wake.notify_one();
        }
        Ok(summary)
    }

    #[allow(clippy::too_many_lines)]
    async fn enqueue_in(
        &self,
        conn: &mut SqliteConnection,
        episode: &Episode,
        priority: Priority,
        now: OffsetDateTime,
    ) -> Result<(EnqueueOutcome, Vec<Event>), UguisuError> {
        let enclosure = episode
            .primary_enclosure()
            .ok_or_else(|| UguisuError::Invalid("episode has no enclosure".to_owned()))?;
        let existing = downloads::get_job_by_episode(conn, episode.id)
            .await
            .map_err(storage_err)?;
        let queued_event = |job: &DownloadJob, requeued: bool| {
            Event::now(
                Some(job.podcast_id),
                Some(job.episode_id),
                EventKind::DownloadQueued {
                    job_id: job.id,
                    enclosure_url: job.source_url.clone(),
                    priority: job.priority,
                    requeued,
                },
            )
        };
        match existing {
            Some(job) if job.state == DownloadState::Completed => {
                Ok((EnqueueOutcome::AlreadyCompleted(job), Vec::new()))
            }
            Some(job)
                if transition(job.state, JobEvent::UserRetry).is_ok()
                    && !job.state.is_pending() =>
            {
                // Boxed: a refresh that queues downloads awaits this, and the
                // future would otherwise cross clippy's size limit.
                Box::pin(self.retarget_taken(conn, &job, now)).await?;
                downloads::requeue(
                    conn,
                    job.id,
                    &[job.state],
                    reason::REQUEUED,
                    Some(priority),
                    now,
                )
                .await
                .map_err(storage_err)?;
                episodes::set_archive_state(conn, job.episode_id, ArchiveState::Queued, None, now)
                    .await
                    .map_err(storage_err)?;
                let job = downloads::get_job(conn, job.id)
                    .await
                    .map_err(storage_err)?
                    .ok_or_else(|| UguisuError::Internal("job vanished".to_owned()))?;
                let ev = queued_event(&job, true);
                event_repo::insert_all(conn, std::slice::from_ref(&ev))
                    .await
                    .map_err(storage_err)?;
                Ok((EnqueueOutcome::Requeued(job), vec![ev]))
            }
            Some(job) => Ok((EnqueueOutcome::Existing(job), Vec::new())),
            None => {
                let host = HostKey::of(&enclosure.url)
                    .map_err(|e| UguisuError::Invalid(format!("enclosure url: {e}")))?;
                let id = JobId::new();
                let ext = extension_for(enclosure.mime_type.as_deref(), &enclosure.url);
                let mut dest = Destination::for_job(episode.podcast_id, episode.id, id, &ext);
                if let Some(target) = self.resolve_target(conn, episode, &ext).await? {
                    dest.target = target;
                }
                let job = DownloadJob {
                    id,
                    episode_id: episode.id,
                    podcast_id: episode.podcast_id,
                    enclosure_id: Some(enclosure.id),
                    source_url: enclosure.url.clone(),
                    host_key: host.as_str().to_owned(),
                    state: DownloadState::Queued,
                    state_reason: Some(reason::USER.to_owned()),
                    priority,
                    attempt_count: 0,
                    max_attempts: self.inner.deps.config.max_attempts,
                    next_attempt_at: None,
                    bytes_downloaded: 0,
                    total_bytes: None,
                    part_path: dest.part,
                    target_path: dest.target,
                    content_type: None,
                    sniffed_type: None,
                    etag: None,
                    last_modified: None,
                    accept_ranges: None,
                    hash_algo: "sha256".to_owned(),
                    hash_value: None,
                    last_http_status: None,
                    last_error_kind: None,
                    last_error_detail: None,
                    claimed_at: None,
                    progress_at: None,
                    started_at: None,
                    finished_at: None,
                    created_at: now,
                    updated_at: now,
                };
                downloads::insert_job(conn, &job)
                    .await
                    .map_err(storage_err)?;
                episodes::set_archive_state(conn, job.episode_id, ArchiveState::Queued, None, now)
                    .await
                    .map_err(storage_err)?;
                let ev = queued_event(&job, false);
                event_repo::insert_all(conn, std::slice::from_ref(&ev))
                    .await
                    .map_err(storage_err)?;
                Ok((EnqueueOutcome::Created(job), vec![ev]))
            }
        }
    }

    /// Asks the configured resolver where this job's file should end up.
    ///
    /// Four things can send the answer back to the identifier
    /// layout, and none of them is an error worth failing an enqueue over:
    /// no resolver is configured, the podcast row cannot be read, the
    /// proposed path is unusable or already claimed by another job, or
    /// something is on the disk under that name, in any case the file system
    /// folds. The identifier layout always works, and `archive relocate` can
    /// move the file to the template path later.
    async fn resolve_target(
        &self,
        conn: &mut uguisu_storage::SqliteConnection,
        episode: &Episode,
        extension: &str,
    ) -> Result<Option<String>, UguisuError> {
        let Some(podcast) = podcasts::get(conn, episode.podcast_id)
            .await
            .map_err(storage_err)?
        else {
            return Ok(None);
        };
        let Some(proposed) = self.inner.deps.destinations.target(&DestinationRequest {
            podcast: &podcast,
            episode,
            extension,
        }) else {
            return Ok(None);
        };
        // The resolver is host-supplied code: its answer is checked here,
        // not trusted. `resolve` refuses traversal, absolute forms and
        // anything that would leave the media directory.
        if let Err(e) = paths::resolve(&self.inner.deps.media_dir, &proposed) {
            tracing::warn!(episode = %episode.id, path = %proposed, error = %e, "rendered destination refused; keeping the identifier layout");
            return Ok(None);
        }
        if let Some(owner) = downloads::job_owner_of_target(conn, &proposed)
            .await
            .map_err(storage_err)?
            && owner != episode.id
        {
            tracing::warn!(episode = %episode.id, path = %proposed, %owner, "rendered destination is already claimed; keeping the identifier layout");
            return Ok(None);
        }
        // An import or a rebuild records a path no job targets.
        if let Some(owner) = archive_files::owner_of_path(conn, &proposed)
            .await
            .map_err(storage_err)?
            && owner != episode.id
        {
            tracing::warn!(episode = %episode.id, path = %proposed, %owner, "rendered destination is another episode's archived file; keeping the identifier layout");
            return Ok(None);
        }
        // The database compares names exactly, the disk as it folds them: a
        // title that differs only in case is the same file on NTFS and APFS.
        if !record_file_gone(&self.inner.deps.media_dir, &proposed) {
            tracing::warn!(episode = %episode.id, path = %proposed, "rendered destination is taken on disk; keeping the identifier layout");
            return Ok(None);
        }
        Ok(Some(proposed))
    }

    /// Renders a `target_exists` job's target again before it is queued
    /// again. The name it was given is taken, which [`Self::resolve_target`]
    /// now sees, so a retry can finish instead of failing the same way.
    async fn retarget_taken(
        &self,
        conn: &mut SqliteConnection,
        job: &DownloadJob,
        now: OffsetDateTime,
    ) -> Result<(), UguisuError> {
        if job.state != DownloadState::Failed
            || job.state_reason.as_deref() != Some(reason::TARGET_EXISTS)
        {
            return Ok(());
        }
        let Some(episode) = episodes::get(conn, job.episode_id)
            .await
            .map_err(storage_err)?
        else {
            return Ok(());
        };
        let ext = Path::new(&job.target_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin");
        let target = match self.resolve_target(conn, &episode, ext).await? {
            Some(target) => target,
            None => Destination::for_job(job.podcast_id, job.episode_id, job.id, ext).target,
        };
        if target != job.target_path {
            tracing::info!(job = %job.id, from = %job.target_path, to = %target, "target taken; queued under another name");
            downloads::retarget_failed(conn, job.id, &target, now)
                .await
                .map_err(storage_err)?;
        }
        Ok(())
    }

    // ---- queries
    /// One page of jobs.
    pub async fn list(&self, filter: &JobFilter) -> Result<JobPage, UguisuError> {
        let limit = page::limit(Some(filter.limit), page::DEFAULT, MAX_PAGE)?;
        let mut r = self
            .inner
            .deps
            .storage
            .reader()
            .await
            .map_err(storage_err)?;
        let after = match filter.after {
            Some(id) => {
                // A cursor is a request parameter, not a collection: one that
                // names no row, or a row outside what the filter asked for, is
                // a bad request rather than a missing resource (ADR 0040).
                let j = downloads::get_job(&mut r, id)
                    .await
                    .map_err(storage_err)?
                    .filter(|j| filter.podcast_id.is_none_or(|p| j.podcast_id == p))
                    .filter(|j| filter.state.is_none_or(|s| j.state == s))
                    .ok_or_else(|| UguisuError::Invalid(format!("unknown cursor {id}")))?;
                Some((j.created_at, j.id))
            }
            None => None,
        };
        let rows = downloads::list_summaries(
            &mut r,
            &downloads::JobFilter {
                state: filter.state,
                podcast_id: filter.podcast_id,
            },
            after,
            page::over_fetch(limit),
        )
        .await
        .map_err(storage_err)?;
        let (jobs, next_after) = page::truncate(rows, limit, |j| j.job.id);
        Ok(JobPage { jobs, next_after })
    }

    /// A job with attempts and live progress.
    pub async fn job(&self, id: JobId) -> Result<JobDetail, UguisuError> {
        let mut r = self
            .inner
            .deps
            .storage
            .reader()
            .await
            .map_err(storage_err)?;
        let job = downloads::get_job(&mut r, id)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "download job".to_owned(),
                id: id.to_string(),
            })?;
        let attempts = downloads::list_attempts(&mut r, id)
            .await
            .map_err(storage_err)?;
        Ok(JobDetail {
            job,
            attempts,
            progress: self.progress(id),
        })
    }

    /// The job of an episode, if any.
    pub async fn job_for_episode(
        &self,
        episode_id: EpisodeId,
    ) -> Result<Option<DownloadJob>, UguisuError> {
        let mut r = self
            .inner
            .deps
            .storage
            .reader()
            .await
            .map_err(storage_err)?;
        downloads::get_job_by_episode(&mut r, episode_id)
            .await
            .map_err(storage_err)
    }

    /// Live progress of a job held by a worker in this process.
    #[must_use]
    pub fn progress(&self, id: JobId) -> Option<ProgressSnapshot> {
        lock(&self.inner.running)
            .get(&id)
            .and_then(|(h, _)| h.progress())
    }

    /// Queue statistics.
    pub async fn stats(&self) -> Result<DownloadStats, UguisuError> {
        let mut r = self
            .inner
            .deps
            .storage
            .reader()
            .await
            .map_err(storage_err)?;
        let by_state = downloads::count_by_state(&mut r)
            .await
            .map_err(storage_err)?;
        let control = downloads::control_get(&mut r).await.map_err(storage_err)?;
        let next_retry_at = downloads::next_retry_due(&mut r)
            .await
            .map_err(storage_err)?;
        let orphan_parts = lock(&self.inner.last_reconcile).as_ref().map_or(0, |r| {
            u32::try_from(r.orphan_parts.len()).unwrap_or(u32::MAX)
        });
        Ok(DownloadStats {
            by_state,
            running: u32::try_from(self.running_count()).unwrap_or(u32::MAX),
            workers_started: self.is_started(),
            paused_all: control.paused.then_some(control.paused_reason).flatten(),
            next_retry_at,
            orphan_parts,
        })
    }

    // ---- user commands
    /// Cancels a job (keeps its `.part`).
    pub async fn cancel(&self, id: JobId) -> Result<DownloadJob, UguisuError> {
        self.command(id, JobEvent::UserCancel).await
    }

    /// Pauses a job (keeps its `.part`).
    pub async fn pause(&self, id: JobId) -> Result<DownloadJob, UguisuError> {
        self.command(id, JobEvent::UserPause).await
    }

    /// Resumes a paused job.
    pub async fn resume(&self, id: JobId) -> Result<DownloadJob, UguisuError> {
        self.command(id, JobEvent::UserResume).await
    }

    /// Queues a retrying, failed or cancelled job again with a fresh budget.
    pub async fn retry(&self, id: JobId) -> Result<DownloadJob, UguisuError> {
        self.command(id, JobEvent::UserRetry).await
    }

    async fn command(&self, id: JobId, event: JobEvent) -> Result<DownloadJob, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.inner.deps.storage.begin().await.map_err(storage_err)?;
        let job = downloads::get_job(&mut tx, id)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "download job".to_owned(),
                id: id.to_string(),
            })?;
        let to = transition(job.state, event).map_err(|e| UguisuError::Conflict(e.to_string()))?;
        if matches!(event, JobEvent::UserRetry | JobEvent::UserResume)
            && archived_without_its_download(&mut tx, &self.inner.deps.media_dir, job.episode_id)
                .await?
        {
            return Err(UguisuError::Conflict(ALREADY_ARCHIVED.to_owned()));
        }
        let changed = if event == JobEvent::UserRetry {
            self.retarget_taken(&mut tx, &job, now).await?;
            downloads::requeue(&mut tx, id, &[job.state], reason::REQUEUED, None, now)
                .await
                .map_err(storage_err)?
        } else {
            let why = match event {
                JobEvent::UserCancel | JobEvent::UserPause => reason::USER,
                _ => reason::RESUMED,
            };
            let t = Transition {
                to,
                reason: Some(why),
                finished: to == DownloadState::Cancelled,
                ..Transition::default()
            };
            downloads::transition(&mut tx, id, &[job.state], &t, now)
                .await
                .map_err(storage_err)?
        };
        if !changed {
            return Err(UguisuError::Conflict(format!(
                "job {id} changed while the command ran"
            )));
        }
        let archive = match to {
            DownloadState::Cancelled => ArchiveState::Expected,
            _ => ArchiveState::Queued,
        };
        episodes::set_archive_state(&mut tx, job.episode_id, archive, None, now)
            .await
            .map_err(storage_err)?;
        let bytes = job.bytes_downloaded;
        let kind = match event {
            JobEvent::UserCancel => EventKind::DownloadCancelled {
                job_id: id,
                bytes_downloaded: bytes,
            },
            JobEvent::UserPause => EventKind::DownloadPaused {
                job_id: id,
                reason: reason::USER.to_owned(),
                bytes_downloaded: bytes,
            },
            JobEvent::UserResume => EventKind::DownloadResumed { job_id: id },
            _ => EventKind::DownloadQueued {
                job_id: id,
                enclosure_url: job.source_url.clone(),
                priority: job.priority,
                requeued: true,
            },
        };
        let ev = Event::now(Some(job.podcast_id), Some(job.episode_id), kind);
        event_repo::insert_all(&mut tx, std::slice::from_ref(&ev))
            .await
            .map_err(storage_err)?;
        let after = downloads::get_job(&mut tx, id)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| UguisuError::Internal("job vanished".to_owned()))?;
        tx.commit().await.map_err(|e| storage_err(e.into()))?;
        self.inner.deps.sink.publish(&[ev]);
        // The database is final; now stop a worker that still holds it.
        if matches!(event, JobEvent::UserCancel | JobEvent::UserPause)
            && let Some((h, _)) = lock(&self.inner.running).get(&id)
        {
            h.stop(StopReason::User);
        }
        if matches!(event, JobEvent::UserResume | JobEvent::UserRetry) {
            self.inner.wake.notify_one();
        }
        tracing::info!(job = %id, ?event, state = %after.state, "download command applied");
        Ok(after)
    }

    /// Queues every failed job again, except one whose episode's file an import
    /// or a rebuild put in place; returns how many.
    pub async fn retry_failed(&self) -> Result<u64, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.inner.deps.storage.begin().await.map_err(storage_err)?;
        let failed = downloads::jobs_in_states(&mut tx, &[DownloadState::Failed])
            .await
            .map_err(storage_err)?;
        let mut events = Vec::new();
        for job in &failed {
            if archived_without_its_download(&mut tx, &self.inner.deps.media_dir, job.episode_id)
                .await?
            {
                continue;
            }
            self.retarget_taken(&mut tx, job, now).await?;
            if downloads::requeue(
                &mut tx,
                job.id,
                &[DownloadState::Failed],
                reason::REQUEUED,
                None,
                now,
            )
            .await
            .map_err(storage_err)?
            {
                episodes::set_archive_state(
                    &mut tx,
                    job.episode_id,
                    ArchiveState::Queued,
                    None,
                    now,
                )
                .await
                .map_err(storage_err)?;
                events.push(Event::now(
                    Some(job.podcast_id),
                    Some(job.episode_id),
                    EventKind::DownloadQueued {
                        job_id: job.id,
                        enclosure_url: job.source_url.clone(),
                        priority: job.priority,
                        requeued: true,
                    },
                ));
            }
        }
        event_repo::insert_all(&mut tx, &events)
            .await
            .map_err(storage_err)?;
        tx.commit().await.map_err(|e| storage_err(e.into()))?;
        self.inner.deps.sink.publish(&events);
        if !events.is_empty() {
            self.inner.wake.notify_one();
        }
        Ok(events.len() as u64)
    }

    /// Pauses the queue: pending jobs become `paused(paused_all)`, running
    /// workers stop and park their jobs the same way, nothing is claimed
    /// until [`resume_all`](Self::resume_all).
    pub async fn pause_all(&self, why: PauseAllReason) -> Result<DownloadControl, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.inner.deps.storage.begin().await.map_err(storage_err)?;
        let control = downloads::control_set(&mut tx, true, Some(why), now)
            .await
            .map_err(storage_err)?;
        let reason = match why {
            PauseAllReason::User => reason::PAUSED_ALL,
            PauseAllReason::DiskFull => reason::DISK_FULL,
        };
        let ids = downloads::pause_pending(&mut tx, reason, now)
            .await
            .map_err(storage_err)?;
        let ev = Event::now(None, None, EventKind::DownloadPausedAll { reason: why });
        event_repo::insert_all(&mut tx, std::slice::from_ref(&ev))
            .await
            .map_err(storage_err)?;
        tx.commit().await.map_err(|e| storage_err(e.into()))?;
        self.inner.deps.sink.publish(&[ev]);
        let stop = match why {
            PauseAllReason::User => StopReason::PausedAll,
            PauseAllReason::DiskFull => StopReason::DiskFull,
        };
        self.inner.stop_all_running(stop);
        tracing::warn!(
            reason = why.as_str(),
            pending = ids.len(),
            "downloads paused"
        );
        Ok(control)
    }

    /// Lifts a global pause and re-queues the jobs it parked.
    pub async fn resume_all(&self) -> Result<DownloadControl, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.inner.deps.storage.begin().await.map_err(storage_err)?;
        let control = downloads::control_set(&mut tx, false, None, now)
            .await
            .map_err(storage_err)?;
        let ids = downloads::requeue_where(
            &mut tx,
            &[DownloadState::Paused],
            Some(&[reason::PAUSED_ALL, reason::DISK_FULL]),
            reason::RESUMED,
            now,
        )
        .await
        .map_err(storage_err)?;
        let ev = Event::now(None, None, EventKind::DownloadResumedAll {});
        event_repo::insert_all(&mut tx, std::slice::from_ref(&ev))
            .await
            .map_err(storage_err)?;
        tx.commit().await.map_err(|e| storage_err(e.into()))?;
        self.inner.deps.sink.publish(&[ev]);
        self.inner.wake.notify_one();
        tracing::info!(requeued = ids.len(), "downloads resumed");
        Ok(control)
    }

    // ---- reconciliation
    /// Repairs what a crash left behind and reports what it cannot repair.
    /// `deep` also checks every completed job's target and every pending
    /// job's target path.
    pub async fn reconcile(&self, deep: bool) -> Result<ReconcileReport, UguisuError> {
        let report = crate::scheduler::reconcile(&self.inner.deps, deep).await?;
        *lock(&self.inner.last_reconcile) = Some(report.clone());
        if !report.orphan_parts.is_empty() {
            tracing::warn!(
                count = report.orphan_parts.len(),
                "orphan .part files found; run `download reconcile` to list them"
            );
        }
        if report.recovered + report.finalized + report.finalization_lost > 0 {
            tracing::info!(
                recovered = report.recovered,
                finalized = report.finalized,
                lost = report.finalization_lost,
                "download jobs recovered"
            );
        }
        if let Some(why) = report.paused_all {
            tracing::warn!(
                reason = why.as_str(),
                "downloads are paused; use `download resume-all`"
            );
        }
        self.inner.wake.notify_one();
        Ok(report)
    }

    /// The last reconciliation report of this process.
    #[must_use]
    pub fn last_reconcile(&self) -> Option<ReconcileReport> {
        lock(&self.inner.last_reconcile).clone()
    }
}

impl Inner {
    pub(crate) fn stop_all_running(&self, reason: StopReason) {
        for (h, _) in lock(&self.running).values() {
            h.stop(reason);
        }
    }
}

/// The refusal for an episode whose file an import or a rebuild put in place.
const ALREADY_ARCHIVED: &str =
    "episode is already archived and its file is in place; an import or a rebuild put it there";

/// Whether the episode has an archived file, still in place, that no
/// completed download of its own put there: a download would fetch the bytes
/// again, then fail `target_exists` or take the record over and leave the
/// found file unowned. A record whose file is gone refuses nothing, because
/// downloading it is the repair (ADR 0060).
async fn archived_without_its_download(
    conn: &mut SqliteConnection,
    media_dir: &Path,
    episode_id: EpisodeId,
) -> Result<bool, UguisuError> {
    let Some(record) = archive_files::get_by_episode(conn, episode_id)
        .await
        .map_err(storage_err)?
    else {
        return Ok(false);
    };
    let job = downloads::get_job_by_episode(conn, episode_id)
        .await
        .map_err(storage_err)?;
    Ok(job.is_none_or(|j| j.state != DownloadState::Completed)
        && !record_file_gone(media_dir, &record.relative_path))
}

/// Whether nothing is at a stored path. Only `NotFound` means gone: a file
/// that cannot be checked, or a path this crate does not resolve (a `:` the
/// posix profile allowed), is never downloaded over (ADR 0021).
fn record_file_gone(media_dir: &Path, relative: &str) -> bool {
    paths::resolve(media_dir, relative).is_ok_and(|at| {
        std::fs::symlink_metadata(at).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    })
}

/// The error for an episode that cannot be queued, if it cannot.
fn refusal_error(episode: &Episode) -> Option<UguisuError> {
    refusal(episode).map(|why| match why {
        "no_enclosure" => UguisuError::Invalid("episode has no enclosure".to_owned()),
        other => UguisuError::Conflict(format!(
            "episode is not downloadable ({other}{})",
            episode
                .skip_reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default()
        )),
    })
}

/// Why an episode cannot be queued, if it cannot.
fn refusal(episode: &Episode) -> Option<&'static str> {
    if episode.primary_enclosure().is_none() {
        return Some("no_enclosure");
    }
    if episode.duplicate_of_episode_id.is_some() {
        return Some("duplicate_candidate");
    }
    if episode.archive_state == ArchiveState::Skipped {
        return Some("skipped");
    }
    if episode.removed_from_feed_at.is_some() {
        return Some("removed_from_feed");
    }
    None
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Relative path of an orphan `.part` under the media directory.
pub(crate) fn relative_part(media_dir: &std::path::Path, part: &std::path::Path) -> String {
    part.strip_prefix(media_dir).map_or_else(
        |_| part.display().to_string(),
        |p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        },
    )
}

/// Every `.part` file under `<media dir>/<podcast>/.uguisu-tmp/`, relative to
/// the media directory and sorted, whether a job owns it or not.
#[must_use]
pub fn part_files(media_dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(podcasts) = std::fs::read_dir(media_dir) else {
        return out;
    };
    for podcast in podcasts.flatten() {
        let Ok(parts) = std::fs::read_dir(podcast.path().join(TMP_DIR)) else {
            continue;
        };
        for entry in parts.flatten() {
            let p = entry.path();
            if is_part_path(&p) {
                out.push(relative_part(media_dir, &p));
            }
        }
    }
    out.sort();
    out
}

/// Whether a path is a `.part` in a `.uguisu-tmp` directory.
pub(crate) fn is_part_path(p: &std::path::Path) -> bool {
    p.extension().is_some_and(|e| e == crate::paths::PART_EXT)
        && p.parent()
            .and_then(std::path::Path::file_name)
            .is_some_and(|d| d == TMP_DIR)
}
