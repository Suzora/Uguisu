//! One attempt of one job: prepare, inspect the `.part`, request (with a
//! validated `Range`), stream to disk while hashing, validate, finalize
//! atomically and persist the outcome (ADR 0019).
//!
//! Every database write after the claim is a compare-and-set from the
//! state the worker believes it holds; when a user command changed the
//! job meanwhile, the worker only closes its file and records the bytes.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use uguisu_core::download::{
    AttemptOutcome, DownloadAttempt, DownloadErrorKind, DownloadJob, DownloadState, PauseAllReason,
};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::{AttemptId, JobId};
use uguisu_core::model::ArchiveState;
use uguisu_core::redact;
use uguisu_http::{ContentRange, GetOptions, HostKey, HttpError, StatusCode, StreamingResponse};
use uguisu_storage::downloads::{self, AttemptEnd, ResponseMeta, Transition};
use uguisu_storage::{StorageError, episodes, events as event_repo};

use crate::deps::Deps;
use crate::error::{DownloadError, classify};
use crate::handle::{JobHandle, StopReason};
use crate::paths;
use crate::progress::ProgressMeter;
use crate::retry::RetryPlan;
use crate::sniff;
use crate::space::bytes_needed;
use crate::state::{JobEvent, reason, transition};

/// Write buffer in front of the `.part` file.
const WRITE_BUFFER: usize = 256 * 1024;
/// Read buffer when re-hashing a resumed prefix.
const REHASH_BUFFER: usize = 1024 * 1024;
/// Retries of a rename another process blocks, after doubling waits from
/// 100 ms (ADR 0019).
const RENAME_RETRIES: u32 = 5;

/// How a job run ended, for the scheduler and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// The file is at its final path.
    Completed,
    /// A retry is scheduled.
    RetryScheduled {
        /// When.
        next_attempt_at: OffsetDateTime,
    },
    /// The job gave up.
    Failed {
        /// `state_reason`.
        reason: String,
    },
    /// The job was paused (user, pause-all, disk full).
    Paused {
        /// `state_reason`.
        reason: String,
    },
    /// The user cancelled while it ran.
    Cancelled,
    /// A shutdown parked it as `queued(shutdown)`.
    Requeued,
    /// The job changed under the worker; nothing more to do.
    Lost,
    /// A fail point fired (tests): the worker vanished without writing.
    Died,
}

/// A job a worker owns.
#[derive(Debug, Clone)]
pub struct Claimed {
    /// The job as of the claim (`downloading`, attempt count incremented).
    pub job: DownloadJob,
    /// The open attempt row.
    pub attempt: DownloadAttempt,
}

/// Claims a `queued`/`retrying` job for a worker: compare-and-set to
/// `downloading`, an open attempt row, the episode's archive state and the
/// `download.started` event, in one transaction. `None` when the job is
/// gone or not claimable any more.
pub async fn claim(
    deps: &Deps,
    id: JobId,
    now: OffsetDateTime,
) -> Result<Option<Claimed>, StorageError> {
    let mut tx = deps.storage.begin().await?;
    let Some(before) = downloads::get_job(&mut tx, id).await? else {
        return Ok(None);
    };
    if transition(before.state, JobEvent::Claim).is_err()
        || !downloads::claim(&mut tx, id, now).await?
    {
        return Ok(None);
    }
    let Some(job) = downloads::get_job(&mut tx, id).await? else {
        return Ok(None);
    };
    let mut attempt = DownloadAttempt {
        id: AttemptId::new(),
        job_id: id,
        // Replaced by the number the database assigns (max + 1).
        attempt_no: job.attempt_count,
        started_at: now,
        finished_at: None,
        source_url: job.source_url.clone(),
        range_start: job.bytes_downloaded,
        http_status: None,
        bytes_received: 0,
        duration_ms: 0,
        avg_rate_bps: None,
        outcome: None,
        error_kind: None,
        error_detail: None,
        next_attempt_at: None,
    };
    attempt.attempt_no = downloads::insert_attempt(&mut tx, &attempt).await?;
    episodes::set_archive_state(
        &mut tx,
        job.episode_id,
        ArchiveState::Downloading,
        None,
        now,
    )
    .await?;
    let ev = Event::now(
        Some(job.podcast_id),
        Some(job.episode_id),
        EventKind::DownloadStarted {
            job_id: id,
            attempt: job.attempt_count,
            resumed_from: job.bytes_downloaded,
            total_bytes: job.total_bytes,
            url: job.source_url.clone(),
        },
    );
    event_repo::insert_all(&mut tx, std::slice::from_ref(&ev)).await?;
    tx.commit().await?;
    deps.sink.publish(&[ev]);
    tracing::info!(job = %id, episode = %job.episode_id, attempt = job.attempt_count, resumed_from = job.bytes_downloaded, url = %redact::urls(job.source_url.as_str()), "download started");
    Ok(Some(Claimed { job, attempt }))
}

/// Mutable state of one attempt.
struct Ctx {
    job: DownloadJob,
    attempt: DownloadAttempt,
    /// Bytes known to be on disk (flushed).
    bytes_on_disk: u64,
    /// First bytes of the file, for sniffing.
    head: Vec<u8>,
    /// Attempt-log notes (`range_ignored`, `source_changed`, …).
    notes: Vec<&'static str>,
    /// Status of the final hop.
    http_status: Option<u16>,
    /// Bytes received in this attempt.
    attempt_bytes: u64,
    /// Average rate over the attempt.
    avg_rate_bps: Option<u64>,
    /// Declared enclosure length (space check).
    declared_length: Option<u64>,
}

impl Ctx {
    fn new(claimed: Claimed) -> Self {
        Self {
            bytes_on_disk: claimed.job.bytes_downloaded,
            job: claimed.job,
            attempt: claimed.attempt,
            head: Vec::new(),
            notes: Vec::new(),
            http_status: None,
            attempt_bytes: 0,
            avg_rate_bps: None,
            declared_length: None,
        }
    }
}

/// How the attempt ended before persistence.
enum End {
    Completed {
        hash: String,
        size: u64,
        sniffed: Option<String>,
    },
    Error(DownloadError),
    Fatal {
        reason: &'static str,
        kind: DownloadErrorKind,
        detail: String,
    },
    RenameFailed {
        reason: &'static str,
        detail: String,
    },
    DiskFull {
        needed: u64,
        available: u64,
    },
    Stopped,
    Lost,
    /// A fail point fired (tests only).
    #[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
    Died,
}

fn fatal(reason: &'static str, kind: DownloadErrorKind, detail: impl Into<String>) -> End {
    End::Fatal {
        reason,
        kind,
        detail: detail.into(),
    }
}

fn io_end(path: &Path, source: std::io::Error) -> End {
    End::Error(DownloadError::Io {
        path: path.to_path_buf(),
        source,
    })
}

impl From<StorageError> for End {
    fn from(e: StorageError) -> Self {
        Self::Error(DownloadError::Storage(e))
    }
}

/// Runs one attempt of a claimed job to its persisted end.
pub async fn run_job(deps: &Deps, handle: &JobHandle, claimed: Claimed) -> JobOutcome {
    let started = Instant::now();
    let mut ctx = Ctx::new(claimed);
    let end = run_attempt(deps, handle, &mut ctx, started).await;
    settle(deps, handle, ctx, end, started).await
}

/// Fails a claimed job before its attempt reaches the network, exactly as
/// an attempt that ended in a fatal `reason` would.
pub async fn fail_claimed(
    deps: &Deps,
    handle: &JobHandle,
    claimed: Claimed,
    reason: &'static str,
    kind: DownloadErrorKind,
    detail: String,
) -> JobOutcome {
    let end = fatal(reason, kind, detail);
    settle(deps, handle, Ctx::new(claimed), end, Instant::now()).await
}

async fn settle(
    deps: &Deps,
    handle: &JobHandle,
    ctx: Ctx,
    end: End,
    started: Instant,
) -> JobOutcome {
    match finish(deps, handle, ctx, end, started).await {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!(error = %e, "download outcome could not be persisted");
            JobOutcome::Lost
        }
    }
}

// ---- the attempt
struct Prepared {
    part: PathBuf,
    target: PathBuf,
}

#[allow(clippy::too_many_lines)] // the response table reads best in one place
async fn run_attempt(deps: &Deps, handle: &JobHandle, ctx: &mut Ctx, started: Instant) -> End {
    let prepared = match prepare(deps, ctx).await {
        Ok(p) => p,
        Err(end) => return end,
    };
    let resume = match inspect_part(deps, ctx, &prepared.part).await {
        Ok(r) => r,
        Err(end) => return end,
    };
    let mut offset = resume.offset;
    let mut hasher = resume.hasher;
    ctx.head = resume.head;
    let mut complete = resume.complete;

    if !complete {
        // Request; a 416 on an incomplete file or a 200 answering a Range
        // restarts once from zero within this attempt.
        let mut restarted = false;
        let response = loop {
            let resp = match request(deps, handle, ctx, offset).await {
                Ok(r) => r,
                Err(end) => return end,
            };
            ctx.http_status = Some(resp.status.as_u16());
            match resp.status {
                StatusCode::OK => {
                    if offset > 0 {
                        ctx.notes.push("range_ignored");
                        if let Err(e) = truncate(&prepared.part, 0).await {
                            return io_end(&prepared.part, e);
                        }
                        offset = 0;
                        hasher = Sha256::new();
                        ctx.head.clear();
                        ctx.bytes_on_disk = 0;
                    }
                    ctx.job.total_bytes = resp.content_length();
                    if resp.content_encoding().is_some() {
                        ctx.notes.push("content_encoding");
                    }
                }
                StatusCode::PARTIAL_CONTENT => match resp.content_range() {
                    Some(Ok(ContentRange::Bytes { start, total, .. })) if start == offset => {
                        ctx.job.total_bytes = total
                            .or_else(|| resp.content_length().map(|n| n.saturating_add(offset)));
                        if ctx
                            .job
                            .total_bytes
                            .is_some_and(|t| t > deps.config.max_bytes)
                        {
                            return too_large(deps);
                        }
                    }
                    other => {
                        let detail = match other {
                            Some(Ok(cr)) => format!("{cr:?} does not start at {offset}"),
                            Some(Err(e)) => e.to_string(),
                            None => "206 without Content-Range".to_owned(),
                        };
                        return fatal(
                            reason::NOT_RETRYABLE,
                            DownloadErrorKind::RangeInvalid,
                            detail,
                        );
                    }
                },
                StatusCode::RANGE_NOT_SATISFIABLE => {
                    let total = resp
                        .content_range()
                        .and_then(Result::ok)
                        .and_then(ContentRange::total);
                    if offset > 0 && total == Some(offset) {
                        ctx.job.total_bytes = total;
                        complete = true;
                        record_meta(deps, ctx, &resp, true).await;
                        break None;
                    }
                    if restarted {
                        return fatal(
                            reason::NOT_RETRYABLE,
                            DownloadErrorKind::RangeInvalid,
                            "416 for a fresh request",
                        );
                    }
                    ctx.notes.push("range_unsatisfiable");
                    if let Err(e) = truncate(&prepared.part, 0).await {
                        return io_end(&prepared.part, e);
                    }
                    offset = 0;
                    hasher = Sha256::new();
                    ctx.head.clear();
                    ctx.bytes_on_disk = 0;
                    ctx.job.total_bytes = None;
                    restarted = true;
                    continue;
                }
                status => {
                    let retry_after = uguisu_http::retry_after(&resp.headers);
                    return End::Error(DownloadError::Status {
                        status: status.as_u16(),
                        retry_after,
                    });
                }
            }
            record_meta(deps, ctx, &resp, resp.status == StatusCode::PARTIAL_CONTENT).await;
            break Some(resp);
        };

        if let Some(resp) = response {
            let received = match stream_body(
                deps,
                handle,
                ctx,
                &prepared.part,
                resp,
                offset,
                &mut hasher,
                started,
            )
            .await
            {
                Ok(n) => n,
                Err(end) => return end,
            };
            offset = received;
        }
    }

    // Validate.
    if let Some(total) = ctx.job.total_bytes
        && offset != total
    {
        return End::Error(DownloadError::LengthMismatch {
            expected: total,
            received: offset,
        });
    }
    if offset == 0 {
        return fatal(
            reason::VALIDATION,
            DownloadErrorKind::Validation,
            "empty body",
        );
    }
    let sniffed = sniff::sniff(&ctx.head).map(str::to_owned);
    if complete {
        ctx.bytes_on_disk = offset;
    }

    finalize(deps, ctx, &prepared, hasher, offset, sniffed).await
}

async fn prepare(deps: &Deps, ctx: &mut Ctx) -> Result<Prepared, End> {
    let now = OffsetDateTime::now_utc();
    let episode = {
        let mut r = deps.storage.reader().await?;
        episodes::get(&mut r, ctx.job.episode_id).await?
    };
    let Some(episode) = episode else {
        return Err(fatal(
            reason::SOURCE_MISSING,
            DownloadErrorKind::Io,
            "episode no longer exists",
        ));
    };
    let Some(enclosure) = episode.primary_enclosure() else {
        return Err(fatal(
            reason::SOURCE_MISSING,
            DownloadErrorKind::Io,
            "episode has no enclosure",
        ));
    };
    ctx.declared_length = enclosure.length_bytes;
    if enclosure.url != ctx.job.source_url {
        let host = HostKey::of(&enclosure.url).map_err(|e| {
            fatal(
                reason::VALIDATION,
                DownloadErrorKind::Validation,
                e.to_string(),
            )
        })?;
        tracing::info!(job = %ctx.job.id, from = %redact::urls(ctx.job.source_url.as_str()), to = %redact::urls(enclosure.url.as_str()), "enclosure url changed; restarting the transfer");
        let mut w = deps.storage.writer().await?;
        downloads::set_source(
            &mut w,
            ctx.job.id,
            &enclosure.url,
            host.as_str(),
            Some(enclosure.id),
            now,
        )
        .await?;
        downloads::reset_transfer(&mut w, ctx.job.id, now).await?;
        drop(w);
        ctx.job.source_url = enclosure.url.clone();
        host.as_str().clone_into(&mut ctx.job.host_key);
        ctx.job.enclosure_id = Some(enclosure.id);
        ctx.job.bytes_downloaded = 0;
        ctx.job.total_bytes = None;
        ctx.job.etag = None;
        ctx.job.last_modified = None;
        ctx.job.accept_ranges = None;
        ctx.job.hash_value = None;
        ctx.bytes_on_disk = 0;
        ctx.attempt.source_url = enclosure.url.clone();
        ctx.attempt.range_start = 0;
        ctx.notes.push("source_changed");
        if let Ok(part) = paths::resolve(&deps.media_dir, &ctx.job.part_path) {
            let _ = tokio::fs::remove_file(&part).await;
        }
    }
    let target = paths::resolve(&deps.media_dir, &ctx.job.target_path)
        .map_err(|e| fatal(reason::VALIDATION, DownloadErrorKind::Io, e.to_string()))?;
    let part = paths::resolve(&deps.media_dir, &ctx.job.part_path)
        .map_err(|e| fatal(reason::VALIDATION, DownloadErrorKind::Io, e.to_string()))?;
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| io_end(parent, e))?;
    }
    match tokio::fs::try_exists(&target).await {
        Ok(true) => {
            return Err(fatal(
                reason::TARGET_EXISTS,
                DownloadErrorKind::Io,
                format!("a file already exists at {}", ctx.job.target_path),
            ));
        }
        Ok(false) => {}
        Err(e) => return Err(io_end(&target, e)),
    }
    let needed = bytes_needed(
        ctx.job.total_bytes,
        ctx.job.bytes_downloaded,
        ctx.declared_length,
    )
    .saturating_add(deps.config.min_free_bytes);
    match deps.space.available(&deps.media_dir) {
        Ok(available) if available < needed => {
            return Err(End::DiskFull { needed, available });
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, dir = %deps.media_dir.display(), "free-space probe failed; continuing without the check");
        }
    }
    Ok(Prepared { part, target })
}

struct Resume {
    offset: u64,
    hasher: Sha256,
    head: Vec<u8>,
    complete: bool,
}

fn validator(job: &DownloadJob) -> Option<String> {
    if let Some(e) = &job.etag
        && uguisu_http::headers::is_strong_etag(e)
    {
        return Some(e.clone());
    }
    job.last_modified.clone()
}

async fn inspect_part(deps: &Deps, ctx: &mut Ctx, part: &Path) -> Result<Resume, End> {
    let file_len = match tokio::fs::metadata(part).await {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(io_end(part, e)),
    };
    let mut offset = file_len.min(ctx.job.bytes_downloaded);
    let can_resume =
        offset > 0 && ctx.job.accept_ranges == Some(true) && validator(&ctx.job).is_some();
    if !can_resume {
        if offset > 0 {
            ctx.notes.push("resume_not_possible");
        }
        offset = 0;
    }
    if file_len != offset {
        truncate(part, offset).await.map_err(|e| io_end(part, e))?;
    }
    ctx.bytes_on_disk = offset;
    let complete = offset > 0 && ctx.job.total_bytes == Some(offset);
    let (hasher, head) = if offset > 0 {
        let p = part.to_path_buf();
        tokio::task::spawn_blocking(move || rehash(&p, offset))
            .await
            .map_err(|e| {
                fatal(
                    reason::VALIDATION,
                    DownloadErrorKind::Io,
                    format!("rehash task failed: {e}"),
                )
            })?
            .map_err(|e| io_end(part, e))?
    } else {
        (Sha256::new(), Vec::new())
    };
    let _ = deps;
    Ok(Resume {
        offset,
        hasher,
        head,
        complete,
    })
}

fn rehash(part: &Path, len: u64) -> std::io::Result<(Sha256, Vec<u8>)> {
    use std::io::Read;
    let mut file = std::fs::File::open(part)?;
    let mut hasher = Sha256::new();
    let mut head = Vec::with_capacity(sniff::SNIFF_LEN);
    let mut buf = vec![0u8; REHASH_BUFFER];
    let mut remaining = len;
    while remaining > 0 {
        let want = usize::try_from(remaining.min(REHASH_BUFFER as u64)).unwrap_or(REHASH_BUFFER);
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "part file shorter than recorded",
            ));
        }
        if head.len() < sniff::SNIFF_LEN {
            let take = (sniff::SNIFF_LEN - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok((hasher, head))
}

async fn truncate(part: &Path, len: u64) -> std::io::Result<()> {
    let f = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)
        .await?;
    f.set_len(len).await?;
    f.sync_all().await
}

async fn request(
    deps: &Deps,
    handle: &JobHandle,
    ctx: &Ctx,
    offset: u64,
) -> Result<StreamingResponse, End> {
    let opts = GetOptions {
        range_start: (offset > 0).then_some(offset),
        if_range: (offset > 0).then(|| validator(&ctx.job)).flatten(),
        idle_timeout: Some(deps.config.idle_timeout),
        cancel: Some(handle.token()),
        retry: Some(false),
        // The cap is for the file. A 200 counts from zero, so the client holds
        // it to the cap; a 206 starts at `offset`, so the worker does.
        max_bytes: Some(deps.config.max_bytes),
        ..GetOptions::default()
    };
    match deps.client.get_stream(&ctx.job.source_url, &opts).await {
        Ok(r) => Ok(r),
        Err(HttpError::Cancelled) => Err(End::Stopped),
        Err(e) => Err(End::Error(DownloadError::Http(e))),
    }
}

async fn record_meta(deps: &Deps, ctx: &mut Ctx, resp: &StreamingResponse, partial: bool) {
    let meta = ResponseMeta {
        content_type: resp.content_type().map(str::to_owned),
        etag: resp.etag().map(str::to_owned),
        last_modified: resp.last_modified().map(str::to_owned),
        accept_ranges: Some(partial || resp.accept_ranges_bytes()),
        total_bytes: ctx.job.total_bytes,
        http_status: resp.status.as_u16(),
    };
    ctx.job.content_type.clone_from(&meta.content_type);
    ctx.job.etag.clone_from(&meta.etag);
    ctx.job.last_modified.clone_from(&meta.last_modified);
    ctx.job.accept_ranges = meta.accept_ranges;
    ctx.job.last_http_status = Some(meta.http_status);
    match deps.storage.writer().await {
        Ok(mut w) => {
            if let Err(e) =
                downloads::set_response_meta(&mut w, ctx.job.id, &meta, OffsetDateTime::now_utc())
                    .await
            {
                tracing::warn!(job = %ctx.job.id, error = %e, "response metadata not recorded");
            }
        }
        Err(e) => tracing::warn!(job = %ctx.job.id, error = %e, "response metadata not recorded"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_body(
    deps: &Deps,
    handle: &JobHandle,
    ctx: &mut Ctx,
    part: &Path,
    mut resp: StreamingResponse,
    offset: u64,
    hasher: &mut Sha256,
    started: Instant,
) -> Result<u64, End> {
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)
        .await
        .map_err(|e| io_end(part, e))?;
    let mut file = file;
    file.seek(SeekFrom::Start(offset))
        .await
        .map_err(|e| io_end(part, e))?;
    let mut writer = BufWriter::with_capacity(WRITE_BUFFER, file);
    let mut meter = ProgressMeter::new(
        Instant::now(),
        offset,
        ctx.job.total_bytes,
        deps.config.progress_interval,
    );
    let mut received = offset;
    let end: Option<End> = loop {
        let chunk = match resp.body.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break None,
            Err(HttpError::Cancelled) => break Some(End::Stopped),
            Err(e) => break Some(End::Error(DownloadError::Http(e))),
        };
        #[cfg(any(test, feature = "testing"))]
        if let Some(view) = deps.fail_point()
            && let crate::deps::FailPoint::WriteError(at, kind) = view.point
            && received >= at
        {
            deps.fire_fail_point();
            break Some(io_end(part, std::io::Error::from(kind)));
        }
        if received.saturating_add(chunk.len() as u64) > deps.config.max_bytes {
            break Some(too_large(deps));
        }
        hasher.update(&chunk);
        if ctx.head.len() < sniff::SNIFF_LEN {
            let take = (sniff::SNIFF_LEN - ctx.head.len()).min(chunk.len());
            ctx.head.extend_from_slice(&chunk[..take]);
        }
        if let Err(e) = writer.write_all(&chunk).await {
            break Some(io_end(part, e));
        }
        received += chunk.len() as u64;
        ctx.attempt_bytes = received - offset;
        let now = Instant::now();
        meter.record(now, received);
        #[cfg(any(test, feature = "testing"))]
        if let Some(view) = deps.fail_point()
            && let crate::deps::FailPoint::AfterBytes(n) = view.point
            && ctx.attempt_bytes >= n
        {
            deps.fire_fail_point();
            // A crash loses whatever the buffer still holds.
            std::mem::forget(writer);
            return Err(End::Died);
        }
        if meter.should_report(now) {
            if let Err(e) = writer.flush().await {
                break Some(io_end(part, e));
            }
            ctx.bytes_on_disk = received;
            ctx.job.bytes_downloaded = received;
            persist_progress(deps, handle, ctx, &meter).await;
            meter.reported(now);
        }
    };
    // Whatever the outcome, keep what reached the disk.
    match writer.flush().await {
        Ok(()) => {
            ctx.bytes_on_disk = received;
            ctx.job.bytes_downloaded = received;
        }
        Err(e) => {
            if end.is_none() {
                return Err(io_end(part, e));
            }
        }
    }
    ctx.avg_rate_bps = meter.average_bps(Instant::now());
    let _ = started;
    if let Some(end) = end {
        return Err(end);
    }
    let file = writer.into_inner();
    file.sync_all().await.map_err(|e| io_end(part, e))?;
    drop(file);
    Ok(received)
}

fn too_large(deps: &Deps) -> End {
    End::Error(DownloadError::Http(HttpError::BodyTooLarge {
        limit: deps.config.max_bytes,
    }))
}

async fn persist_progress(deps: &Deps, handle: &JobHandle, ctx: &Ctx, meter: &ProgressMeter) {
    let now = OffsetDateTime::now_utc();
    let snapshot = meter.snapshot(now);
    handle.set_progress(snapshot.clone());
    match deps.storage.writer().await {
        Ok(mut w) => {
            if let Err(e) = downloads::set_progress(
                &mut w,
                ctx.job.id,
                snapshot.bytes_downloaded,
                snapshot.total_bytes,
                now,
            )
            .await
            {
                tracing::warn!(job = %ctx.job.id, error = %e, "progress not recorded");
            }
        }
        Err(e) => tracing::warn!(job = %ctx.job.id, error = %e, "progress not recorded"),
    }
    deps.sink.publish(&[Event::now(
        Some(ctx.job.podcast_id),
        Some(ctx.job.episode_id),
        EventKind::DownloadProgress {
            job_id: ctx.job.id,
            bytes_downloaded: snapshot.bytes_downloaded,
            total_bytes: snapshot.total_bytes,
            percentage: snapshot.percentage,
            speed_bps: snapshot.speed_bps,
            eta_secs: snapshot.eta_secs,
        },
    )]);
}

async fn finalize(
    deps: &Deps,
    ctx: &mut Ctx,
    prepared: &Prepared,
    hasher: Sha256,
    size: u64,
    sniffed: Option<String>,
) -> End {
    let hash = hex::encode(hasher.finalize());
    #[cfg(any(test, feature = "testing"))]
    if died_at(deps, crate::deps::FailPoint::BeforeFinalizingWrite) {
        return End::Died;
    }
    let now = OffsetDateTime::now_utc();
    let marked = match deps.storage.writer().await {
        Ok(mut w) => {
            downloads::set_finalizing(&mut w, ctx.job.id, &hash, size, sniffed.as_deref(), now)
                .await
        }
        Err(e) => Err(e),
    };
    match marked {
        Ok(true) => {}
        Ok(false) => return End::Lost,
        Err(e) => return End::Error(DownloadError::Storage(e)),
    }
    ctx.job.state = DownloadState::Finalizing;
    ctx.job.hash_value = Some(hash.clone());
    #[cfg(any(test, feature = "testing"))]
    if died_at(deps, crate::deps::FailPoint::AfterFinalizingWrite) {
        return End::Died;
    }
    if let Err((reason, detail)) = rename_into_place(&prepared.part, &prepared.target).await {
        return End::RenameFailed { reason, detail };
    }
    #[cfg(any(test, feature = "testing"))]
    if died_at(deps, crate::deps::FailPoint::AfterRename) {
        return End::Died;
    }
    End::Completed {
        hash,
        size,
        sniffed,
    }
}

#[cfg(any(test, feature = "testing"))]
fn died_at(deps: &Deps, point: crate::deps::FailPoint) -> bool {
    if deps.fail_point().is_some_and(|v| v.point == point) {
        deps.fire_fail_point();
        return true;
    }
    false
}

/// Whether another process holds the file, which waiting can clear: on
/// Windows a sharing violation (32) or the access denial (5) a scanner's open
/// handle causes. Elsewhere a failed rename does not clear by itself.
fn rename_blocked(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32))
}

/// Moves the complete `.part` to the target: never over an existing
/// file, with a short retry on sharing violations, and a directory fsync
/// where the platform has one. A failure is its `state_reason` and detail:
/// `target_exists` when the name is taken, in any case the file system
/// folds, and `finalization` otherwise.
pub(crate) async fn rename_into_place(
    part: &Path,
    target: &Path,
) -> Result<(), (&'static str, String)> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            (
                reason::FINALIZATION,
                format!("create {}: {e}", parent.display()),
            )
        })?;
    }
    let mut retry = 0u32;
    loop {
        let (from, to) = (part.to_owned(), target.to_owned());
        let renamed = tokio::task::spawn_blocking(move || crate::paths::rename_new(&from, &to))
            .await
            .map_err(|e| (reason::FINALIZATION, format!("rename task: {e}")))?;
        match renamed {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err((
                    reason::TARGET_EXISTS,
                    format!("{} appeared during the download", target.display()),
                ));
            }
            Err(e) if retry < RENAME_RETRIES && rename_blocked(&e) => {
                let wait = Duration::from_millis(100 << retry);
                retry += 1;
                tracing::debug!(retry, error = %e, "rename blocked; retrying");
                tokio::time::sleep(wait).await;
            }
            Err(e) => {
                return Err((
                    reason::FINALIZATION,
                    format!("rename to {}: {e}", target.display()),
                ));
            }
        }
    }
    #[cfg(unix)]
    if let Some(parent) = target.parent()
        && let Ok(dir) = tokio::fs::File::open(parent).await
    {
        let _ = dir.sync_all().await;
    }
    Ok(())
}

// ---- persistence of the outcome
/// Builds the attempt row's closing values.
struct Ender {
    now: OffsetDateTime,
    http_status: Option<u16>,
    bytes_received: u64,
    duration_ms: u64,
    avg_rate_bps: Option<u64>,
    notes: Option<String>,
}

impl Ender {
    fn end(
        &self,
        outcome: AttemptOutcome,
        error: Option<(DownloadErrorKind, String)>,
        next: Option<OffsetDateTime>,
    ) -> AttemptEnd {
        AttemptEnd {
            finished_at: self.now,
            http_status: self.http_status,
            bytes_received: self.bytes_received,
            duration_ms: self.duration_ms,
            avg_rate_bps: self.avg_rate_bps,
            outcome,
            error_kind: error.as_ref().map(|(k, _)| *k),
            error_detail: error
                .map(|(_, d)| d)
                .or_else(|| self.notes.clone())
                .map(|d| d.chars().take(1024).collect()),
            next_attempt_at: next,
        }
    }
}

#[allow(clippy::too_many_lines)] // one arm per outcome, each a small transaction
async fn finish(
    deps: &Deps,
    handle: &JobHandle,
    ctx: Ctx,
    end: End,
    started: Instant,
) -> Result<JobOutcome, StorageError> {
    let now = OffsetDateTime::now_utc();
    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = started.elapsed().as_millis() as u64;
    let plan = RetryPlan::from_config(&deps.config);
    let job = &ctx.job;
    let notes = if ctx.notes.is_empty() {
        None
    } else {
        Some(ctx.notes.join(","))
    };
    let attempt_end = Ender {
        now,
        http_status: ctx.http_status,
        bytes_received: ctx.attempt_bytes,
        duration_ms,
        avg_rate_bps: ctx.avg_rate_bps,
        notes,
    };

    match end {
        End::Died => Ok(JobOutcome::Died),
        End::Completed {
            hash,
            size,
            sniffed,
        } => {
            let mut tx = deps.storage.begin().await?;
            let t = Transition {
                to: DownloadState::Completed,
                reason: None,
                bytes_downloaded: Some(size),
                finished: true,
                http_status: ctx.http_status,
                ..Transition::default()
            };
            let changed =
                downloads::transition(&mut tx, job.id, &[DownloadState::Finalizing], &t, now)
                    .await?;
            downloads::finish_attempt(
                &mut tx,
                ctx.attempt.id,
                &attempt_end.end(AttemptOutcome::Completed, None, None),
            )
            .await?;
            episodes::set_archive_state(&mut tx, job.episode_id, ArchiveState::Archived, None, now)
                .await?;
            let ev = Event::now(
                Some(job.podcast_id),
                Some(job.episode_id),
                EventKind::DownloadCompleted {
                    job_id: job.id,
                    path: job.target_path.clone(),
                    size_bytes: size,
                    hash_algo: job.hash_algo.clone(),
                    hash_value: hash,
                    content_type: job.content_type.clone(),
                    sniffed_type: sniffed,
                    attempts: job.attempt_count,
                    duration_ms,
                },
            );
            if changed {
                event_repo::insert_all(&mut tx, std::slice::from_ref(&ev)).await?;
            }
            tx.commit().await?;
            if changed {
                deps.sink.publish(&[ev]);
            }
            tracing::info!(job = %job.id, episode = %job.episode_id, bytes = size, attempts = job.attempt_count, duration_ms, "download completed");
            Ok(JobOutcome::Completed)
        }
        End::RenameFailed { reason, detail } => {
            fail(
                deps,
                &ctx,
                reason,
                DownloadErrorKind::Io,
                detail,
                &attempt_end,
                &[DownloadState::Finalizing],
                now,
            )
            .await
        }
        End::Fatal {
            reason,
            kind,
            detail,
        } => {
            fail(
                deps,
                &ctx,
                reason,
                kind,
                detail,
                &attempt_end,
                &[DownloadState::Downloading],
                now,
            )
            .await
        }
        End::Error(e) => {
            let (kind, retryable) = classify(&e);
            let detail = e.detail();
            if kind == DownloadErrorKind::DiskFull {
                return disk_full(deps, handle, &ctx, 0, 0, &attempt_end, now).await;
            }
            if kind == DownloadErrorKind::Cancelled {
                return stopped(deps, handle, &ctx, &attempt_end, now).await;
            }
            if retryable && plan.allows_another(job.attempt_count) {
                let next = plan.next_attempt_at(now, job.attempt_count, e.retry_after());
                let mut tx = deps.storage.begin().await?;
                let t = Transition {
                    to: DownloadState::Retrying,
                    reason: Some(kind.as_str()),
                    bytes_downloaded: Some(ctx.bytes_on_disk),
                    next_attempt_at: Some(next),
                    error: Some((kind, detail.as_str())),
                    http_status: ctx.http_status,
                    finished: false,
                };
                let changed =
                    downloads::transition(&mut tx, job.id, &[DownloadState::Downloading], &t, now)
                        .await?;
                downloads::finish_attempt(
                    &mut tx,
                    ctx.attempt.id,
                    &attempt_end.end(
                        AttemptOutcome::RetryScheduled,
                        Some((kind, detail.clone())),
                        Some(next),
                    ),
                )
                .await?;
                let ev = Event::now(
                    Some(job.podcast_id),
                    Some(job.episode_id),
                    EventKind::DownloadRetryScheduled {
                        job_id: job.id,
                        attempt: job.attempt_count,
                        error_kind: kind,
                        detail: detail.clone(),
                        http_status: ctx.http_status,
                        next_attempt_at: next,
                    },
                );
                if changed {
                    episodes::set_archive_state(
                        &mut tx,
                        job.episode_id,
                        ArchiveState::Queued,
                        None,
                        now,
                    )
                    .await?;
                    event_repo::insert_all(&mut tx, std::slice::from_ref(&ev)).await?;
                }
                tx.commit().await?;
                if changed {
                    deps.sink.publish(&[ev]);
                }
                tracing::warn!(job = %job.id, kind = kind.as_str(), detail = %redact::urls(&detail), attempt = job.attempt_count, next = %next, "download attempt failed; retry scheduled");
                Ok(JobOutcome::RetryScheduled {
                    next_attempt_at: next,
                })
            } else {
                let reason = if retryable {
                    reason::MAX_ATTEMPTS
                } else {
                    reason::NOT_RETRYABLE
                };
                fail(
                    deps,
                    &ctx,
                    reason,
                    kind,
                    detail,
                    &attempt_end,
                    &[DownloadState::Downloading],
                    now,
                )
                .await
            }
        }
        End::DiskFull { needed, available } => {
            disk_full(deps, handle, &ctx, needed, available, &attempt_end, now).await
        }
        End::Stopped => stopped(deps, handle, &ctx, &attempt_end, now).await,
        End::Lost => {
            let mut w = deps.storage.writer().await?;
            let state = downloads::get_job(&mut w, job.id).await?.map(|j| j.state);
            let outcome = match state {
                Some(DownloadState::Cancelled) => AttemptOutcome::Cancelled,
                Some(DownloadState::Paused) => AttemptOutcome::Paused,
                _ => AttemptOutcome::Interrupted,
            };
            downloads::finish_attempt(
                &mut w,
                ctx.attempt.id,
                &attempt_end.end(outcome, None, None),
            )
            .await?;
            Ok(JobOutcome::Lost)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn fail(
    deps: &Deps,
    ctx: &Ctx,
    reason: &'static str,
    kind: DownloadErrorKind,
    detail: String,
    attempt_end: &Ender,
    from: &[DownloadState],
    now: OffsetDateTime,
) -> Result<JobOutcome, StorageError> {
    let job = &ctx.job;
    let mut tx = deps.storage.begin().await?;
    let t = Transition {
        to: DownloadState::Failed,
        reason: Some(reason),
        bytes_downloaded: Some(ctx.bytes_on_disk),
        next_attempt_at: None,
        error: Some((kind, detail.as_str())),
        http_status: ctx.http_status,
        finished: true,
    };
    let changed = downloads::transition(&mut tx, job.id, from, &t, now).await?;
    downloads::finish_attempt(
        &mut tx,
        ctx.attempt.id,
        &attempt_end.end(AttemptOutcome::Failed, Some((kind, detail.clone())), None),
    )
    .await?;
    let ev = Event::now(
        Some(job.podcast_id),
        Some(job.episode_id),
        EventKind::DownloadFailed {
            job_id: job.id,
            reason: reason.to_owned(),
            error_kind: kind,
            detail: detail.clone(),
            http_status: ctx.http_status,
            attempts: job.attempt_count,
        },
    );
    if changed {
        episodes::set_archive_state(&mut tx, job.episode_id, ArchiveState::Failed, None, now)
            .await?;
        event_repo::insert_all(&mut tx, std::slice::from_ref(&ev)).await?;
    }
    tx.commit().await?;
    if changed {
        deps.sink.publish(&[ev]);
    }
    tracing::warn!(job = %job.id, episode = %job.episode_id, reason, kind = kind.as_str(), detail = %redact::urls(&detail), attempts = job.attempt_count, "download failed");
    Ok(JobOutcome::Failed {
        reason: reason.to_owned(),
    })
}

async fn disk_full(
    deps: &Deps,
    handle: &JobHandle,
    ctx: &Ctx,
    needed: u64,
    available: u64,
    attempt_end: &Ender,
    now: OffsetDateTime,
) -> Result<JobOutcome, StorageError> {
    let _ = handle;
    let job = &ctx.job;
    let detail = if needed > 0 {
        format!("{needed} bytes needed, {available} available")
    } else {
        "write failed: no space left on device".to_owned()
    };
    let mut tx = deps.storage.begin().await?;
    let t = Transition {
        to: DownloadState::Paused,
        reason: Some(reason::DISK_FULL),
        bytes_downloaded: Some(ctx.bytes_on_disk),
        next_attempt_at: None,
        error: Some((DownloadErrorKind::DiskFull, detail.as_str())),
        http_status: ctx.http_status,
        finished: false,
    };
    let changed =
        downloads::transition(&mut tx, job.id, &[DownloadState::Downloading], &t, now).await?;
    downloads::refund_attempt(&mut tx, job.id, now).await?;
    downloads::finish_attempt(
        &mut tx,
        ctx.attempt.id,
        &attempt_end.end(
            AttemptOutcome::Paused,
            Some((DownloadErrorKind::DiskFull, detail.clone())),
            None,
        ),
    )
    .await?;
    let control = downloads::control_get(&mut tx).await?;
    let mut events = Vec::new();
    if changed {
        episodes::set_archive_state(&mut tx, job.episode_id, ArchiveState::Queued, None, now)
            .await?;
        events.push(Event::now(
            Some(job.podcast_id),
            Some(job.episode_id),
            EventKind::DownloadPaused {
                job_id: job.id,
                reason: reason::DISK_FULL.to_owned(),
                bytes_downloaded: ctx.bytes_on_disk,
            },
        ));
    }
    if !control.paused {
        downloads::control_set(&mut tx, true, Some(PauseAllReason::DiskFull), now).await?;
        events.push(Event::now(
            None,
            None,
            EventKind::DownloadPausedAll {
                reason: PauseAllReason::DiskFull,
            },
        ));
    }
    if !events.is_empty() {
        event_repo::insert_all(&mut tx, &events).await?;
    }
    tx.commit().await?;
    deps.sink.publish(&events);
    tracing::error!(job = %job.id, dir = %deps.media_dir.display(), %detail, "media file system is full; downloads paused");
    Ok(JobOutcome::Paused {
        reason: reason::DISK_FULL.to_owned(),
    })
}

async fn stopped(
    deps: &Deps,
    handle: &JobHandle,
    ctx: &Ctx,
    attempt_end: &Ender,
    now: OffsetDateTime,
) -> Result<JobOutcome, StorageError> {
    let job = &ctx.job;
    let stop = handle.stop_reason();
    let (to, why, outcome, event) = match stop {
        Some(StopReason::PausedAll) => (
            DownloadState::Paused,
            reason::PAUSED_ALL,
            AttemptOutcome::Paused,
            Some(EventKind::DownloadPaused {
                job_id: job.id,
                reason: reason::PAUSED_ALL.to_owned(),
                bytes_downloaded: ctx.bytes_on_disk,
            }),
        ),
        Some(StopReason::DiskFull) => (
            DownloadState::Paused,
            reason::DISK_FULL,
            AttemptOutcome::Paused,
            Some(EventKind::DownloadPaused {
                job_id: job.id,
                reason: reason::DISK_FULL.to_owned(),
                bytes_downloaded: ctx.bytes_on_disk,
            }),
        ),
        Some(StopReason::Shutdown) => (
            DownloadState::Queued,
            reason::SHUTDOWN,
            AttemptOutcome::Interrupted,
            None,
        ),
        Some(StopReason::User) | None => {
            // The command already wrote the final state; record the bytes.
            let mut w = deps.storage.writer().await?;
            downloads::set_progress(&mut w, job.id, ctx.bytes_on_disk, job.total_bytes, now)
                .await?;
            let state = downloads::get_job(&mut w, job.id).await?.map(|j| j.state);
            let outcome = match state {
                Some(DownloadState::Cancelled) => AttemptOutcome::Cancelled,
                Some(DownloadState::Paused) => AttemptOutcome::Paused,
                _ => AttemptOutcome::Interrupted,
            };
            downloads::finish_attempt(
                &mut w,
                ctx.attempt.id,
                &attempt_end.end(outcome, None, None),
            )
            .await?;
            return Ok(match state {
                Some(DownloadState::Cancelled) => JobOutcome::Cancelled,
                Some(DownloadState::Paused) => JobOutcome::Paused {
                    reason: reason::USER.to_owned(),
                },
                _ => JobOutcome::Lost,
            });
        }
    };
    let mut tx = deps.storage.begin().await?;
    let t = Transition {
        to,
        reason: Some(why),
        bytes_downloaded: Some(ctx.bytes_on_disk),
        next_attempt_at: None,
        error: None,
        http_status: ctx.http_status,
        finished: false,
    };
    let changed =
        downloads::transition(&mut tx, job.id, &[DownloadState::Downloading], &t, now).await?;
    downloads::finish_attempt(
        &mut tx,
        ctx.attempt.id,
        &attempt_end.end(outcome, None, None),
    )
    .await?;
    let mut events = Vec::new();
    if changed {
        episodes::set_archive_state(&mut tx, job.episode_id, ArchiveState::Queued, None, now)
            .await?;
        if let Some(kind) = event {
            events.push(Event::now(Some(job.podcast_id), Some(job.episode_id), kind));
        }
    }
    if !events.is_empty() {
        event_repo::insert_all(&mut tx, &events).await?;
    }
    tx.commit().await?;
    deps.sink.publish(&events);
    tracing::info!(job = %job.id, state = %to, reason = why, bytes = ctx.bytes_on_disk, "download stopped");
    Ok(match to {
        DownloadState::Queued => JobOutcome::Requeued,
        _ => JobOutcome::Paused {
            reason: why.to_owned(),
        },
    })
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;
    use std::time::Duration;

    use super::rename_into_place;

    /// Opens the file the way a scanner does, sharing nothing, so a rename
    /// fails with a sharing violation while the handle lives.
    fn hold(path: &Path) -> std::fs::File {
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap()
    }

    #[tokio::test]
    async fn held_part_renames_once_released() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.part");
        let target = dir.path().join("a.mp3");
        std::fs::write(&part, b"audio").unwrap();
        let handle = hold(&part);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            drop(handle);
        });
        rename_into_place(&part, &target).await.unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"audio");
    }

    #[tokio::test]
    async fn held_part_fails_after_retries() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.part");
        let target = dir.path().join("a.mp3");
        std::fs::write(&part, b"audio").unwrap();
        let _handle = hold(&part);
        assert!(rename_into_place(&part, &target).await.is_err());
        assert!(part.exists(), "the part file is kept");
        assert!(!target.exists());
    }
}
