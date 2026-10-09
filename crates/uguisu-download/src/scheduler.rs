//! The scheduler loop (claim → per-host admission → worker task) and the
//! startup reconciliation.
//!
//! The only timer here is the retry tick: it wakes the loop when the
//! earliest `retrying` download job becomes due. Feed refreshes are not
//! scheduled by this crate.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use time::OffsetDateTime;
use uguisu_core::UguisuError;
use uguisu_core::download::{AttemptOutcome, DownloadErrorKind, DownloadState};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::model::ArchiveState;
use uguisu_http::HostKey;
use uguisu_storage::downloads::{self, Transition};
use uguisu_storage::{StorageError, episodes, events as event_repo};

use crate::deps::Deps;
use crate::handle::{JobHandle, StopReason};
use crate::paths::resolve;
use crate::service::{Inner, ReconcileReport, lock};
use crate::state::reason;
use crate::worker::{self, JobOutcome};

/// Longest sleep between loop iterations (the retry tick is also bounded by it).
const MAX_TICK: Duration = Duration::from_secs(30);

/// The scheduler task body.
pub(crate) async fn run(inner: Arc<Inner>) {
    loop {
        if inner.deps.shutdown.is_cancelled() || inner.stopping.load(Ordering::SeqCst) {
            return;
        }
        let paused = match control_paused(&inner.deps).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "download control unreadable; retrying later");
                true
            }
        };
        if !paused {
            let global = inner.deps.config.global_concurrency.max(1);
            while lock(&inner.running).len() < global && !inner.stopping.load(Ordering::SeqCst) {
                match claim_next(&inner).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "claim failed; retrying later");
                        break;
                    }
                }
            }
        }
        let tick = match next_retry_due(&inner.deps).await {
            Some(due) => {
                let now = OffsetDateTime::now_utc();
                if due <= now {
                    Duration::from_millis(50)
                } else {
                    Duration::try_from(due - now)
                        .unwrap_or(MAX_TICK)
                        .min(MAX_TICK)
                }
            }
            None => MAX_TICK,
        };
        tokio::select! {
            () = inner.wake.notified() => {}
            () = tokio::time::sleep(tick) => {}
            () = inner.deps.shutdown.cancelled() => return,
        }
    }
}

async fn control_paused(deps: &Deps) -> Result<bool, StorageError> {
    let mut r = deps.storage.reader().await?;
    Ok(downloads::control_get(&mut r).await?.paused)
}

/// Earliest due time of a `retrying` download job (the retry tick).
async fn next_retry_due(deps: &Deps) -> Option<OffsetDateTime> {
    let mut r = deps.storage.reader().await.ok()?;
    downloads::next_retry_due(&mut r).await.ok().flatten()
}

/// Claims the next eligible job on a host with a free slot and spawns its
/// worker. `Ok(true)` when something happened (claimed, or a candidate was
/// skipped and the loop should look again), `Ok(false)` when nothing is
/// claimable right now.
async fn claim_next(inner: &Arc<Inner>) -> Result<bool, StorageError> {
    let _serialised = inner.claim_lock.lock().await;
    let saturated: Vec<String> = inner
        .deps
        .hosts
        .saturated()
        .iter()
        .map(|k| k.as_str().to_owned())
        .collect();
    let now = OffsetDateTime::now_utc();
    let candidate = {
        let mut r = inner.deps.storage.reader().await?;
        downloads::select_claimable(&mut r, now, &saturated).await?
    };
    let Some(job) = candidate else {
        return Ok(false);
    };
    let host: HostKey = match job.host_key.parse() {
        Ok(h) => h,
        Err(e) => {
            // A corrupt host key can never be admitted. The job fails the way
            // any fatal attempt does, through `downloading`, so the table,
            // the attempt log, the episode and the event all agree.
            if let Some(claimed) = worker::claim(&inner.deps, job.id, now).await? {
                let handle = JobHandle::new(&inner.deps.shutdown);
                worker::fail_claimed(
                    &inner.deps,
                    &handle,
                    claimed,
                    reason::VALIDATION,
                    DownloadErrorKind::Validation,
                    format!("bad host key: {e}"),
                )
                .await;
            }
            return Ok(true);
        }
    };
    let Some(permit) = inner.deps.hosts.try_acquire(&host) else {
        // Saturated between the query and now; the next iteration excludes it.
        return Ok(true);
    };
    let Some(claimed) = worker::claim(&inner.deps, job.id, now).await? else {
        return Ok(true);
    };
    let handle = JobHandle::new(&inner.deps.shutdown);
    let id = job.id;
    let task_inner = Arc::clone(inner);
    let task_handle = Arc::clone(&handle);
    let task = tokio::spawn(async move {
        let outcome = worker::run_job(&task_inner.deps, &task_handle, claimed).await;
        drop(permit);
        if matches!(&outcome, JobOutcome::Paused { reason } if reason == reason::DISK_FULL) {
            task_inner.stop_all_running(StopReason::DiskFull);
        }
        lock(&task_inner.running).remove(&id);
        tracing::debug!(job = %id, ?outcome, "worker finished");
        task_inner.wake.notify_one();
        task_inner.idle.notify_waiters();
    });
    lock(&inner.running).insert(id, (handle, task));
    Ok(true)
}

/// Startup reconciliation (`docs/DOWNLOAD_ENGINE.md` "Recovery").
#[allow(clippy::too_many_lines)]
pub(crate) async fn reconcile(deps: &Deps, deep: bool) -> Result<ReconcileReport, UguisuError> {
    let now = OffsetDateTime::now_utc();
    let mut report = ReconcileReport::default();
    let mut events: Vec<Event> = Vec::new();
    let mut tx = deps.storage.begin().await.map_err(UguisuError::from)?;

    // 1. Jobs a dead worker held.
    let held = downloads::jobs_in_states(&mut tx, &[DownloadState::Downloading])
        .await
        .map_err(UguisuError::from)?;
    for job in &held {
        let t = Transition {
            to: DownloadState::Queued,
            reason: Some(reason::RECOVERED),
            ..Transition::default()
        };
        if downloads::transition(&mut tx, job.id, &[DownloadState::Downloading], &t, now)
            .await
            .map_err(UguisuError::from)?
        {
            downloads::close_open_attempts(&mut tx, job.id, AttemptOutcome::Interrupted, now)
                .await
                .map_err(UguisuError::from)?;
            episodes::set_archive_state(&mut tx, job.episode_id, ArchiveState::Queued, None, now)
                .await
                .map_err(UguisuError::from)?;
            report.recovered += 1;
        }
    }

    // 2. Jobs that crashed while finalizing.
    let finalizing = downloads::jobs_in_states(&mut tx, &[DownloadState::Finalizing])
        .await
        .map_err(UguisuError::from)?;
    for job in &finalizing {
        let target = resolve(&deps.media_dir, &job.target_path);
        let part = resolve(&deps.media_dir, &job.part_path);
        let found = target
            .as_ref()
            .ok()
            .and_then(|p| std::fs::symlink_metadata(p).ok());
        let part_exists = part.as_ref().is_ok_and(|p| p.exists());
        // Whatever is at the target is adopted only when it has the size the
        // job recorded: the finished download, renamed before the crash. A
        // different file is somebody else's and stays, and so does the
        // `.part`; the job fails rather than claim either.
        let mut failure = found
            .as_ref()
            .filter(|m| !m.is_file() || m.len() != job.bytes_downloaded)
            .map(|m| {
                let detail = if m.is_file() {
                    format!(
                        "a file of {} bytes is at the target where {} were expected",
                        m.len(),
                        job.bytes_downloaded
                    )
                } else {
                    "something that is not a file is at the target".to_owned()
                };
                (reason::TARGET_EXISTS, detail)
            });
        let done = if found.is_some() {
            failure.is_none()
        } else if part_exists && let (Ok(part), Ok(target)) = (&part, &target) {
            match worker::rename_into_place(part, target).await {
                Ok(()) => true,
                Err((why, detail)) => {
                    tracing::warn!(job = %job.id, error = %detail, "finalization could not be repeated");
                    failure = Some((why, detail));
                    false
                }
            }
        } else {
            false
        };
        if done {
            let t = Transition {
                to: DownloadState::Completed,
                reason: None,
                finished: true,
                ..Transition::default()
            };
            if downloads::transition(&mut tx, job.id, &[DownloadState::Finalizing], &t, now)
                .await
                .map_err(UguisuError::from)?
            {
                downloads::close_open_attempts(&mut tx, job.id, AttemptOutcome::Completed, now)
                    .await
                    .map_err(UguisuError::from)?;
                episodes::set_archive_state(
                    &mut tx,
                    job.episode_id,
                    ArchiveState::Archived,
                    None,
                    now,
                )
                .await
                .map_err(UguisuError::from)?;
                events.push(Event::now(
                    Some(job.podcast_id),
                    Some(job.episode_id),
                    EventKind::DownloadCompleted {
                        job_id: job.id,
                        path: job.target_path.clone(),
                        size_bytes: job.bytes_downloaded,
                        hash_algo: job.hash_algo.clone(),
                        hash_value: job.hash_value.clone().unwrap_or_default(),
                        content_type: job.content_type.clone(),
                        sniffed_type: job.sniffed_type.clone(),
                        attempts: job.attempt_count,
                        duration_ms: 0,
                    },
                ));
                report.finalized += 1;
            }
        } else {
            let (why, detail) = failure.as_ref().map_or(
                (
                    reason::FINALIZATION_LOST,
                    "neither the .part nor the target survived",
                ),
                |(why, detail)| (*why, detail.as_str()),
            );
            let t = Transition {
                to: DownloadState::Failed,
                reason: Some(why),
                error: Some((DownloadErrorKind::Io, detail)),
                finished: true,
                ..Transition::default()
            };
            if downloads::transition(&mut tx, job.id, &[DownloadState::Finalizing], &t, now)
                .await
                .map_err(UguisuError::from)?
            {
                downloads::close_open_attempts(&mut tx, job.id, AttemptOutcome::Failed, now)
                    .await
                    .map_err(UguisuError::from)?;
                episodes::set_archive_state(
                    &mut tx,
                    job.episode_id,
                    ArchiveState::Failed,
                    None,
                    now,
                )
                .await
                .map_err(UguisuError::from)?;
                events.push(Event::now(
                    Some(job.podcast_id),
                    Some(job.episode_id),
                    EventKind::DownloadFailed {
                        job_id: job.id,
                        reason: why.to_owned(),
                        error_kind: DownloadErrorKind::Io,
                        detail: detail.to_owned(),
                        http_status: None,
                        attempts: job.attempt_count,
                    },
                ));
                match why {
                    reason::TARGET_EXISTS => report.finalization_conflicts += 1,
                    reason::FINALIZATION_LOST => report.finalization_lost += 1,
                    _ => report.finalization_failed += 1,
                }
            }
        }
    }

    // 3. Counts of what is left alone.
    let counts = downloads::count_by_state(&mut tx)
        .await
        .map_err(UguisuError::from)?;
    report.kept_retrying =
        u32::try_from(counts.get(&DownloadState::Retrying).copied().unwrap_or(0))
            .unwrap_or(u32::MAX);
    report.kept_paused =
        u32::try_from(counts.get(&DownloadState::Paused).copied().unwrap_or(0)).unwrap_or(u32::MAX);
    let control = downloads::control_get(&mut tx)
        .await
        .map_err(UguisuError::from)?;
    report.paused_all = control.paused.then_some(control.paused_reason).flatten();

    // 5. Deep checks on targets.
    if deep {
        let completed = downloads::jobs_in_states(&mut tx, &[DownloadState::Completed])
            .await
            .map_err(UguisuError::from)?;
        for job in &completed {
            let exists = resolve(&deps.media_dir, &job.target_path).is_ok_and(|p| p.exists());
            if !exists {
                report.missing_targets += 1;
                episodes::set_archive_state(
                    &mut tx,
                    job.episode_id,
                    ArchiveState::Missing,
                    None,
                    now,
                )
                .await
                .map_err(UguisuError::from)?;
            }
        }
        let pending = downloads::jobs_in_states(
            &mut tx,
            &[
                DownloadState::Queued,
                DownloadState::Retrying,
                DownloadState::Paused,
            ],
        )
        .await
        .map_err(UguisuError::from)?;
        for job in &pending {
            if resolve(&deps.media_dir, &job.target_path).is_ok_and(|p| p.exists()) {
                report.unexpected_targets += 1;
            }
        }
    }
    event_repo::insert_all(&mut tx, &events)
        .await
        .map_err(UguisuError::from)?;
    let ids = downloads::job_ids_owning_parts(&mut tx)
        .await
        .map_err(UguisuError::from)?;
    tx.commit()
        .await
        .map_err(|e| UguisuError::from(StorageError::from(e)))?;
    deps.sink.publish(&events);

    // 4. Orphan .part files (reported, never deleted).
    let known: std::collections::HashSet<String> = ids.iter().map(ToString::to_string).collect();
    let media_dir = deps.media_dir.clone();
    let orphans = tokio::task::spawn_blocking(move || scan_orphans(&media_dir, &known))
        .await
        .unwrap_or_default();
    report.orphan_parts = orphans;
    Ok(report)
}

fn scan_orphans(
    media_dir: &std::path::Path,
    known: &std::collections::HashSet<String>,
) -> Vec<String> {
    crate::service::part_files(media_dir)
        .into_iter()
        .filter(|part| {
            let stem = std::path::Path::new(part)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            !known.contains(stem)
        })
        .collect()
}
