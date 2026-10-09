//! `download_jobs`, `download_attempts` and `download_control` repository
//! (ADR 0018).
//!
//! State changes are compare-and-set: every writer names the states it
//! expects and learns from the affected-row count whether it still owned
//! the job. That is what lets a user command and a worker race safely.

use std::collections::BTreeMap;

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::download::{
    AttemptOutcome, DownloadAttempt, DownloadControl, DownloadErrorKind, DownloadJob,
    DownloadState, JobSummary, PauseAllReason, Priority,
};
use uguisu_core::ids::{AttemptId, EpisodeId, JobId, PodcastId};

use crate::row::{self, bool_from, i64_from_u64, u32_from, u64_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "download_jobs";
const ATTEMPTS: &str = "download_attempts";

const JOB_COLUMNS: &str = "id, episode_id, podcast_id, enclosure_id, source_url, host_key, state, state_reason, \
    priority, attempt_count, max_attempts, next_attempt_at, bytes_downloaded, total_bytes, part_path, \
    target_path, content_type, sniffed_type, etag, last_modified, accept_ranges, hash_algo, hash_value, \
    last_http_status, last_error_kind, last_error_detail, claimed_at, progress_at, started_at, finished_at, \
    created_at, updated_at";

#[derive(FromRow)]
#[allow(clippy::struct_excessive_bools)]
struct JobRow {
    id: String,
    episode_id: String,
    podcast_id: String,
    enclosure_id: Option<String>,
    source_url: String,
    host_key: String,
    state: String,
    state_reason: Option<String>,
    priority: i64,
    attempt_count: i64,
    max_attempts: i64,
    next_attempt_at: Option<String>,
    bytes_downloaded: i64,
    total_bytes: Option<i64>,
    part_path: String,
    target_path: String,
    content_type: Option<String>,
    sniffed_type: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    accept_ranges: Option<i64>,
    hash_algo: String,
    hash_value: Option<String>,
    last_http_status: Option<i64>,
    last_error_kind: Option<String>,
    last_error_detail: Option<String>,
    claimed_at: Option<String>,
    progress_at: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl JobRow {
    fn into_model(self) -> Result<DownloadJob> {
        let rid = self.id.as_str();
        Ok(DownloadJob {
            id: row::id(TABLE, rid, rid)?,
            episode_id: row::id(TABLE, rid, &self.episode_id)?,
            podcast_id: row::id(TABLE, rid, &self.podcast_id)?,
            enclosure_id: row::opt_id(TABLE, rid, self.enclosure_id.as_deref())?,
            source_url: row::req_url(TABLE, rid, &self.source_url)?,
            host_key: self.host_key.clone(),
            state: row::parse_enum(TABLE, rid, "state", &self.state, DownloadState::parse)?,
            state_reason: self.state_reason.clone(),
            priority: Priority::from_i64(self.priority).ok_or_else(|| {
                row::corrupt(TABLE, rid, format!("bad priority {}", self.priority))
            })?,
            attempt_count: u32_from(Some(self.attempt_count)).unwrap_or(0),
            max_attempts: u32_from(Some(self.max_attempts)).unwrap_or(1),
            next_attempt_at: row::opt_ts(TABLE, rid, self.next_attempt_at.as_deref())?,
            bytes_downloaded: u64_from(Some(self.bytes_downloaded)).unwrap_or(0),
            total_bytes: u64_from(self.total_bytes),
            part_path: self.part_path.clone(),
            target_path: self.target_path.clone(),
            content_type: self.content_type.clone(),
            sniffed_type: self.sniffed_type.clone(),
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
            accept_ranges: bool_from(self.accept_ranges),
            hash_algo: self.hash_algo.clone(),
            hash_value: self.hash_value.clone(),
            last_http_status: self.last_http_status.and_then(|n| u16::try_from(n).ok()),
            last_error_kind: self
                .last_error_kind
                .as_deref()
                .map(|k| {
                    row::parse_enum(TABLE, rid, "last_error_kind", k, DownloadErrorKind::parse)
                })
                .transpose()?,
            last_error_detail: self.last_error_detail.clone(),
            claimed_at: row::opt_ts(TABLE, rid, self.claimed_at.as_deref())?,
            progress_at: row::opt_ts(TABLE, rid, self.progress_at.as_deref())?,
            started_at: row::opt_ts(TABLE, rid, self.started_at.as_deref())?,
            finished_at: row::opt_ts(TABLE, rid, self.finished_at.as_deref())?,
            created_at: row::ts(TABLE, rid, &self.created_at)?,
            updated_at: row::ts(TABLE, rid, &self.updated_at)?,
        })
    }
}

fn opt_i64(v: Option<u64>) -> Option<i64> {
    v.map(i64_from_u64)
}

/// Inserts a new job.
pub async fn insert_job(conn: &mut SqliteConnection, j: &DownloadJob) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO download_jobs ({JOB_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32)"
    ))
    .bind(j.id.to_string())
    .bind(j.episode_id.to_string())
    .bind(j.podcast_id.to_string())
    .bind(j.enclosure_id.map(|i| i.to_string()))
    .bind(j.source_url.as_str())
    .bind(&j.host_key)
    .bind(j.state.as_str())
    .bind(&j.state_reason)
    .bind(j.priority.as_i64())
    .bind(i64::from(j.attempt_count))
    .bind(i64::from(j.max_attempts))
    .bind(j.next_attempt_at.map(to_db_ts))
    .bind(i64_from_u64(j.bytes_downloaded))
    .bind(opt_i64(j.total_bytes))
    .bind(&j.part_path)
    .bind(&j.target_path)
    .bind(&j.content_type)
    .bind(&j.sniffed_type)
    .bind(&j.etag)
    .bind(&j.last_modified)
    .bind(j.accept_ranges.map(i64::from))
    .bind(&j.hash_algo)
    .bind(&j.hash_value)
    .bind(j.last_http_status.map(i64::from))
    .bind(j.last_error_kind.map(DownloadErrorKind::as_str))
    .bind(&j.last_error_detail)
    .bind(j.claimed_at.map(to_db_ts))
    .bind(j.progress_at.map(to_db_ts))
    .bind(j.started_at.map(to_db_ts))
    .bind(j.finished_at.map(to_db_ts))
    .bind(to_db_ts(j.created_at))
    .bind(to_db_ts(j.updated_at))
    .execute(conn)
    .await?;
    Ok(())
}

/// One job by id.
pub async fn get_job(conn: &mut SqliteConnection, id: JobId) -> Result<Option<DownloadJob>> {
    let row: Option<JobRow> = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM download_jobs WHERE id = ?1"
    ))
    .bind(id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(JobRow::into_model).transpose()
}

/// The job of an episode, if any (there is at most one).
pub async fn get_job_by_episode(
    conn: &mut SqliteConnection,
    episode_id: EpisodeId,
) -> Result<Option<DownloadJob>> {
    let row: Option<JobRow> = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM download_jobs WHERE episode_id = ?1"
    ))
    .bind(episode_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(JobRow::into_model).transpose()
}

/// Listing filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobFilter {
    /// Only this state.
    pub state: Option<DownloadState>,
    /// Only this podcast.
    pub podcast_id: Option<PodcastId>,
}

/// How many of a podcast's jobs are still outstanding: queued, retrying,
/// downloading or finalizing.
///
/// This is what the archive policy's backlog limit counts, so the limit
/// means "at most this many episodes waiting to be archived at once"
/// rather than "at most this many per evaluation", which a repeated
/// refresh would defeat.
pub async fn outstanding_for_podcast(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {TABLE} WHERE podcast_id = ?1 \
         AND state IN ('queued', 'retrying', 'downloading', 'finalizing')"
    ))
    .bind(podcast_id.to_string())
    .fetch_one(conn)
    .await?;
    Ok(u64_from(Some(count)).unwrap_or(0))
}

/// How many of a podcast's jobs are moving bytes right now: downloading or
/// finalizing.
pub async fn running_for_podcast(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {TABLE} WHERE podcast_id = ?1 AND state IN ('downloading', 'finalizing')"
    ))
    .bind(podcast_id.to_string())
    .fetch_one(conn)
    .await?;
    Ok(u64_from(Some(count)).unwrap_or(0))
}

/// Which episode's job already claims a target path, if any.
///
/// The rendered archive path must be unique across jobs too, not only
/// across archive records: two queued jobs aiming at one path would have
/// the second finalization overwrite the first.
pub async fn job_owner_of_target(
    conn: &mut SqliteConnection,
    target_path: &str,
) -> Result<Option<EpisodeId>> {
    let row: Option<(String, String)> = sqlx::query_as(&format!(
        "SELECT id, episode_id FROM {TABLE} WHERE target_path = ?1 LIMIT 1"
    ))
    .bind(target_path)
    .fetch_optional(conn)
    .await?;
    row.map(|(rid, ep)| row::id(TABLE, &rid, &ep)).transpose()
}

/// Jobs newest first with the episode and podcast titles, keyset-paged.
///
/// One statement rather than a sweep of the episode list afterwards: both
/// joins are on primary keys, both foreign keys are `NOT NULL` with
/// `ON DELETE CASCADE`, so an inner join cannot drop a row and no index is
/// needed beyond the ones the plain list already uses.
pub async fn list_summaries(
    conn: &mut SqliteConnection,
    filter: &JobFilter,
    after: Option<(OffsetDateTime, JobId)>,
    limit: u32,
) -> Result<Vec<JobSummary>> {
    let (after_ts, after_id) = match after {
        Some((t, id)) => (Some(to_db_ts(t)), Some(id.to_string())),
        None => (None, None),
    };
    let columns = JOB_COLUMNS
        .split(", ")
        .map(|c| format!("j.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let rows: Vec<SummaryRow> = sqlx::query_as(&format!(
        "SELECT {columns}, e.title AS episode_title, \
                e.published_at AS episode_published_at, p.title AS podcast_title \
         FROM download_jobs j \
         JOIN episodes e ON e.id = j.episode_id \
         JOIN podcasts p ON p.id = j.podcast_id \
         WHERE (?1 IS NULL OR j.state = ?1) AND (?2 IS NULL OR j.podcast_id = ?2) \
           AND (?3 IS NULL OR j.created_at < ?3 OR (j.created_at = ?3 AND j.id < ?4)) \
         ORDER BY j.created_at DESC, j.id DESC LIMIT ?5"
    ))
    .bind(filter.state.map(DownloadState::as_str))
    .bind(filter.podcast_id.map(|p| p.to_string()))
    .bind(after_ts)
    .bind(after_id)
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(SummaryRow::into_model).collect()
}

/// The joined row. `flatten` is what lets the job's thirty-two columns keep
/// one definition instead of being restated here.
#[derive(FromRow)]
struct SummaryRow {
    #[sqlx(flatten)]
    job: JobRow,
    episode_title: String,
    episode_published_at: Option<String>,
    podcast_title: String,
}

impl SummaryRow {
    fn into_model(self) -> Result<JobSummary> {
        let id = self.job.id.clone();
        let published_at = row::opt_ts("episodes", &id, self.episode_published_at.as_deref())?;
        let job = self.job.into_model()?;
        Ok(JobSummary {
            percentage: job.percentage(),
            job,
            episode_title: self.episode_title,
            podcast_title: self.podcast_title,
            published_at,
        })
    }
}

/// Jobs newest first, keyset-paged on `(created_at, id)`.
pub async fn list_jobs(
    conn: &mut SqliteConnection,
    filter: &JobFilter,
    after: Option<(OffsetDateTime, JobId)>,
    limit: u32,
) -> Result<Vec<DownloadJob>> {
    let (after_ts, after_id) = match after {
        Some((t, id)) => (Some(to_db_ts(t)), Some(id.to_string())),
        None => (None, None),
    };
    let rows: Vec<JobRow> = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM download_jobs \
         WHERE (?1 IS NULL OR state = ?1) AND (?2 IS NULL OR podcast_id = ?2) \
           AND (?3 IS NULL OR created_at < ?3 OR (created_at = ?3 AND id < ?4)) \
         ORDER BY created_at DESC, id DESC LIMIT ?5"
    ))
    .bind(filter.state.map(DownloadState::as_str))
    .bind(filter.podcast_id.map(|p| p.to_string()))
    .bind(after_ts)
    .bind(after_id)
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(JobRow::into_model).collect()
}

/// Every job in one of `states`, oldest first.
pub async fn jobs_in_states(
    conn: &mut SqliteConnection,
    states: &[DownloadState],
) -> Result<Vec<DownloadJob>> {
    if states.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (1..=states.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {JOB_COLUMNS} FROM download_jobs WHERE state IN ({placeholders}) ORDER BY created_at, id"
    );
    let mut q = sqlx::query_as::<_, JobRow>(&sql);
    for s in states {
        q = q.bind(s.as_str());
    }
    let rows = q.fetch_all(conn).await?;
    rows.into_iter().map(JobRow::into_model).collect()
}

/// The jobs that may own a `.part` (for orphan detection): every one but a
/// completed job, whose `.part` name is a leftover of an interrupted move.
pub async fn job_ids_owning_parts(conn: &mut SqliteConnection) -> Result<Vec<JobId>> {
    let ids: Vec<(String,)> =
        sqlx::query_as("SELECT id FROM download_jobs WHERE state <> 'completed'")
            .fetch_all(conn)
            .await?;
    ids.iter().map(|(s,)| row::id(TABLE, s, s)).collect()
}

/// Jobs per state.
pub async fn count_by_state(conn: &mut SqliteConnection) -> Result<BTreeMap<DownloadState, u64>> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT state, count(*) FROM download_jobs GROUP BY state")
            .fetch_all(conn)
            .await?;
    let mut out = BTreeMap::new();
    for (state, n) in rows {
        if let Some(s) = DownloadState::parse(&state) {
            out.insert(s, u64::try_from(n).unwrap_or(0));
        }
    }
    Ok(out)
}

/// The next claimable job: `queued`, or `retrying` whose `next_attempt_at`
/// has passed, on a host that is not in `excluded_hosts`; ordered by
/// priority (high first), then creation time, then id.
pub async fn select_claimable(
    conn: &mut SqliteConnection,
    now: OffsetDateTime,
    excluded_hosts: &[String],
) -> Result<Option<DownloadJob>> {
    let placeholders = (0..excluded_hosts.len())
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(", ");
    let exclusion = if excluded_hosts.is_empty() {
        String::new()
    } else {
        format!("AND host_key NOT IN ({placeholders})")
    };
    let sql = format!(
        "SELECT {JOB_COLUMNS} FROM download_jobs \
         WHERE (state = 'queued' OR (state = 'retrying' AND next_attempt_at <= ?1)) {exclusion} \
         ORDER BY priority DESC, created_at ASC, id ASC LIMIT 1"
    );
    let mut q = sqlx::query_as::<_, JobRow>(&sql).bind(to_db_ts(now));
    for h in excluded_hosts {
        q = q.bind(h);
    }
    let row = q.fetch_optional(conn).await?;
    row.map(JobRow::into_model).transpose()
}

/// Claims a `queued`/`retrying` job for a worker (compare-and-set).
pub async fn claim(conn: &mut SqliteConnection, id: JobId, now: OffsetDateTime) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE download_jobs SET state = 'downloading', state_reason = NULL, claimed_at = ?1, \
         started_at = COALESCE(started_at, ?1), attempt_count = attempt_count + 1, \
         next_attempt_at = NULL, updated_at = ?1 \
         WHERE id = ?2 AND state IN ('queued', 'retrying')",
    )
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// A state change with the columns it carries.
#[derive(Debug, Clone, Default)]
pub struct Transition<'a> {
    /// Target state.
    pub to: DownloadState,
    /// `state_reason` (stable vocabulary).
    pub reason: Option<&'a str>,
    /// New acknowledged byte count, when it changed.
    pub bytes_downloaded: Option<u64>,
    /// `next_attempt_at` (always written: `None` clears it).
    pub next_attempt_at: Option<OffsetDateTime>,
    /// Failure classification and detail, when there was one.
    pub error: Option<(DownloadErrorKind, &'a str)>,
    /// HTTP status, when one was received.
    pub http_status: Option<u16>,
    /// Whether `finished_at` is set to now.
    pub finished: bool,
}

/// Applies `t` when the job is in one of `from` (compare-and-set). Returns
/// whether the row changed.
pub async fn transition(
    conn: &mut SqliteConnection,
    id: JobId,
    from: &[DownloadState],
    t: &Transition<'_>,
    now: OffsetDateTime,
) -> Result<bool> {
    if from.is_empty() {
        return Ok(false);
    }
    let placeholders = (0..from.len())
        .map(|i| format!("?{}", i + 11))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPDATE download_jobs SET state = ?1, state_reason = ?2, updated_at = ?3, \
         bytes_downloaded = COALESCE(?4, bytes_downloaded), next_attempt_at = ?5, \
         last_error_kind = COALESCE(?6, last_error_kind), last_error_detail = COALESCE(?7, last_error_detail), \
         last_http_status = COALESCE(?8, last_http_status), \
         finished_at = CASE WHEN ?9 = 1 THEN ?3 ELSE finished_at END \
         WHERE id = ?10 AND state IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql)
        .bind(t.to.as_str())
        .bind(t.reason)
        .bind(to_db_ts(now))
        .bind(opt_i64(t.bytes_downloaded))
        .bind(t.next_attempt_at.map(to_db_ts))
        .bind(t.error.map(|(k, _)| k.as_str()))
        .bind(t.error.map(|(_, d)| d.to_owned()))
        .bind(t.http_status.map(i64::from))
        .bind(i64::from(t.finished))
        .bind(id.to_string());
    for s in from {
        q = q.bind(s.as_str());
    }
    Ok(q.execute(conn).await?.rows_affected() == 1)
}

/// Gives an attempt back (a full disk pauses the queue instead of
/// spending the budget).
pub async fn refund_attempt(
    conn: &mut SqliteConnection,
    id: JobId,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "UPDATE download_jobs SET attempt_count = CASE WHEN attempt_count > 0 THEN attempt_count - 1 ELSE 0 END, \
         updated_at = ?1 WHERE id = ?2",
    )
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// Records progress (autocommit-friendly, no state check).
pub async fn set_progress(
    conn: &mut SqliteConnection,
    id: JobId,
    bytes_downloaded: u64,
    total_bytes: Option<u64>,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "UPDATE download_jobs SET bytes_downloaded = ?1, total_bytes = COALESCE(?2, total_bytes), \
         progress_at = ?3, updated_at = ?3 WHERE id = ?4",
    )
    .bind(i64_from_u64(bytes_downloaded))
    .bind(opt_i64(total_bytes))
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// Response metadata recorded once the headers of an attempt are in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseMeta {
    /// `Content-Type` as served.
    pub content_type: Option<String>,
    /// `ETag`.
    pub etag: Option<String>,
    /// `Last-Modified`.
    pub last_modified: Option<String>,
    /// Range support (`None` leaves the stored value alone).
    pub accept_ranges: Option<bool>,
    /// Complete length (`None` leaves the stored value alone).
    pub total_bytes: Option<u64>,
    /// Status of the final hop.
    pub http_status: u16,
}

/// Stores response metadata on the job (validators, length, type, status).
pub async fn set_response_meta(
    conn: &mut SqliteConnection,
    id: JobId,
    meta: &ResponseMeta,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "UPDATE download_jobs SET content_type = ?1, etag = ?2, last_modified = ?3, \
         accept_ranges = COALESCE(?4, accept_ranges), total_bytes = COALESCE(?5, total_bytes), \
         last_http_status = ?6, updated_at = ?7 WHERE id = ?8",
    )
    .bind(&meta.content_type)
    .bind(&meta.etag)
    .bind(&meta.last_modified)
    .bind(meta.accept_ranges.map(i64::from))
    .bind(opt_i64(meta.total_bytes))
    .bind(i64::from(meta.http_status))
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// Resets the transfer state (offset, validators, length) so the next
/// request starts from zero; used when the resource changed or a range
/// answer could not be trusted.
pub async fn reset_transfer(
    conn: &mut SqliteConnection,
    id: JobId,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "UPDATE download_jobs SET bytes_downloaded = 0, total_bytes = NULL, etag = NULL, \
         last_modified = NULL, accept_ranges = NULL, hash_value = NULL, updated_at = ?1 WHERE id = ?2",
    )
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// Points the job at a new source URL (the episode's enclosure changed);
/// the caller resets the transfer separately.
pub async fn set_source(
    conn: &mut SqliteConnection,
    id: JobId,
    source_url: &url::Url,
    host_key: &str,
    enclosure_id: Option<uguisu_core::ids::EnclosureId>,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "UPDATE download_jobs SET source_url = ?1, host_key = ?2, enclosure_id = ?3, \
         state_reason = 'source_changed', updated_at = ?4 WHERE id = ?5",
    )
    .bind(source_url.as_str())
    .bind(host_key)
    .bind(enclosure_id.map(|i| i.to_string()))
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// Points a completed job at the path its file was moved to, so a deep
/// reconcile looks for it there; `false` when the episode has no completed
/// job, as for an imported file.
pub async fn set_target_path(
    conn: &mut SqliteConnection,
    episode_id: EpisodeId,
    target_path: &str,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(
        "UPDATE download_jobs SET target_path = ?1, updated_at = ?2 \
         WHERE episode_id = ?3 AND state = 'completed'",
    )
    .bind(target_path)
    .bind(to_db_ts(now))
    .bind(episode_id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Points a failed job at another target before it is queued again
/// (compare-and-set on `failed`); `false` when the job is not failed.
pub async fn retarget_failed(
    conn: &mut SqliteConnection,
    id: JobId,
    target_path: &str,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(
        "UPDATE download_jobs SET target_path = ?1, updated_at = ?2 \
         WHERE id = ?3 AND state = 'failed'",
    )
    .bind(target_path)
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Marks the file complete on disk: `downloading → finalizing` with the
/// hash and final counts (compare-and-set).
pub async fn set_finalizing(
    conn: &mut SqliteConnection,
    id: JobId,
    hash_value: &str,
    bytes: u64,
    sniffed_type: Option<&str>,
    now: OffsetDateTime,
) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE download_jobs SET state = 'finalizing', state_reason = NULL, hash_value = ?1, \
         bytes_downloaded = ?2, total_bytes = ?2, sniffed_type = ?3, updated_at = ?4 \
         WHERE id = ?5 AND state = 'downloading'",
    )
    .bind(hash_value)
    .bind(i64_from_u64(bytes))
    .bind(sniffed_type)
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// Re-queues a failed or cancelled job with a fresh attempt budget
/// (compare-and-set).
pub async fn requeue(
    conn: &mut SqliteConnection,
    id: JobId,
    from: &[DownloadState],
    reason: &str,
    priority: Option<Priority>,
    now: OffsetDateTime,
) -> Result<bool> {
    if from.is_empty() {
        return Ok(false);
    }
    let placeholders = (0..from.len())
        .map(|i| format!("?{}", i + 5))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPDATE download_jobs SET state = 'queued', state_reason = ?1, attempt_count = 0, \
         next_attempt_at = NULL, finished_at = NULL, priority = COALESCE(?2, priority), updated_at = ?3 \
         WHERE id = ?4 AND state IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql)
        .bind(reason)
        .bind(priority.map(Priority::as_i64))
        .bind(to_db_ts(now))
        .bind(id.to_string());
    for s in from {
        q = q.bind(s.as_str());
    }
    Ok(q.execute(conn).await?.rows_affected() == 1)
}

/// Re-queues every job in one of `states` (optionally only with one of
/// `reasons`), returning the ids that changed.
pub async fn requeue_where(
    conn: &mut SqliteConnection,
    states: &[DownloadState],
    reasons: Option<&[&str]>,
    new_reason: &str,
    now: OffsetDateTime,
) -> Result<Vec<JobId>> {
    if states.is_empty() {
        return Ok(Vec::new());
    }
    let mut binds: Vec<String> = states.iter().map(|s| s.as_str().to_owned()).collect();
    let state_ph = (1..=binds.len())
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(", ");
    let mut reason_clause = String::new();
    if let Some(rs) = reasons {
        let start = binds.len() + 3;
        let ph = (0..rs.len())
            .map(|i| format!("?{}", start + i))
            .collect::<Vec<_>>()
            .join(", ");
        reason_clause = format!("AND state_reason IN ({ph})");
        binds.extend(rs.iter().map(|r| (*r).to_owned()));
    }
    let sql = format!(
        "UPDATE download_jobs SET state = 'queued', state_reason = ?1, next_attempt_at = NULL, \
         updated_at = ?2 WHERE state IN ({state_ph}) {reason_clause} RETURNING id"
    );
    let mut q = sqlx::query_as::<_, (String,)>(&sql)
        .bind(new_reason)
        .bind(to_db_ts(now));
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q.fetch_all(conn).await?;
    rows.iter().map(|(s,)| row::id(TABLE, s, s)).collect()
}

/// Pauses every `queued`/`retrying` job with `reason` (pause-all),
/// returning the ids that changed.
pub async fn pause_pending(
    conn: &mut SqliteConnection,
    reason: &str,
    now: OffsetDateTime,
) -> Result<Vec<JobId>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "UPDATE download_jobs SET state = 'paused', state_reason = ?1, next_attempt_at = NULL, \
         updated_at = ?2 WHERE state IN ('queued', 'retrying') RETURNING id",
    )
    .bind(reason)
    .bind(to_db_ts(now))
    .fetch_all(conn)
    .await?;
    rows.iter().map(|(s,)| row::id(TABLE, s, s)).collect()
}

/// Earliest `next_attempt_at` among `retrying` jobs (the download
/// scheduler's retry tick; unrelated to feed refreshes).
pub async fn next_retry_due(conn: &mut SqliteConnection) -> Result<Option<OffsetDateTime>> {
    let v: Option<(Option<String>,)> =
        sqlx::query_as("SELECT min(next_attempt_at) FROM download_jobs WHERE state = 'retrying'")
            .fetch_optional(conn)
            .await?;
    Ok(v.and_then(|(s,)| s).and_then(|s| crate::parse_db_ts(&s)))
}

// ---- attempts
#[derive(FromRow)]
struct AttemptRow {
    id: String,
    job_id: String,
    attempt_no: i64,
    started_at: String,
    finished_at: Option<String>,
    source_url: String,
    range_start: i64,
    http_status: Option<i64>,
    bytes_received: i64,
    duration_ms: i64,
    avg_rate_bps: Option<i64>,
    outcome: Option<String>,
    error_kind: Option<String>,
    error_detail: Option<String>,
    next_attempt_at: Option<String>,
}

const ATTEMPT_COLUMNS: &str = "id, job_id, attempt_no, started_at, finished_at, source_url, range_start, http_status, \
    bytes_received, duration_ms, avg_rate_bps, outcome, error_kind, error_detail, next_attempt_at";

impl AttemptRow {
    fn into_model(self) -> Result<DownloadAttempt> {
        let rid = self.id.as_str();
        Ok(DownloadAttempt {
            id: row::id(ATTEMPTS, rid, rid)?,
            job_id: row::id(ATTEMPTS, rid, &self.job_id)?,
            attempt_no: u32_from(Some(self.attempt_no)).unwrap_or(0),
            started_at: row::ts(ATTEMPTS, rid, &self.started_at)?,
            finished_at: row::opt_ts(ATTEMPTS, rid, self.finished_at.as_deref())?,
            source_url: row::req_url(ATTEMPTS, rid, &self.source_url)?,
            range_start: u64_from(Some(self.range_start)).unwrap_or(0),
            http_status: self.http_status.and_then(|n| u16::try_from(n).ok()),
            bytes_received: u64_from(Some(self.bytes_received)).unwrap_or(0),
            duration_ms: u64_from(Some(self.duration_ms)).unwrap_or(0),
            avg_rate_bps: u64_from(self.avg_rate_bps),
            outcome: self
                .outcome
                .as_deref()
                .map(|o| row::parse_enum(ATTEMPTS, rid, "outcome", o, AttemptOutcome::parse))
                .transpose()?,
            error_kind: self
                .error_kind
                .as_deref()
                .map(|k| row::parse_enum(ATTEMPTS, rid, "error_kind", k, DownloadErrorKind::parse))
                .transpose()?,
            error_detail: self.error_detail.clone(),
            next_attempt_at: row::opt_ts(ATTEMPTS, rid, self.next_attempt_at.as_deref())?,
        })
    }
}

/// Opens an attempt and returns the number the database gave it.
///
/// The number is always one past the job's highest attempt so far, never
/// `a.attempt_no`: `attempt_count` is the retry budget and a user retry
/// resets it, while the attempt log has to stay unique and monotonic
/// (`UNIQUE (job_id, attempt_no)`).
pub async fn insert_attempt(conn: &mut SqliteConnection, a: &DownloadAttempt) -> Result<u32> {
    let attempt_no: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(attempt_no), 0) + 1 FROM download_attempts WHERE job_id = ?1",
    )
    .bind(a.job_id.to_string())
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(&format!(
        "INSERT INTO download_attempts ({ATTEMPT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
    ))
    .bind(a.id.to_string())
    .bind(a.job_id.to_string())
    .bind(attempt_no)
    .bind(to_db_ts(a.started_at))
    .bind(a.finished_at.map(to_db_ts))
    .bind(a.source_url.as_str())
    .bind(i64_from_u64(a.range_start))
    .bind(a.http_status.map(i64::from))
    .bind(i64_from_u64(a.bytes_received))
    .bind(i64_from_u64(a.duration_ms))
    .bind(opt_i64(a.avg_rate_bps))
    .bind(a.outcome.map(AttemptOutcome::as_str))
    .bind(a.error_kind.map(DownloadErrorKind::as_str))
    .bind(&a.error_detail)
    .bind(a.next_attempt_at.map(to_db_ts))
    .execute(&mut *conn)
    .await?;
    Ok(u32_from(Some(attempt_no)).unwrap_or(1))
}

/// How an attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptEnd {
    /// End time.
    pub finished_at: OffsetDateTime,
    /// HTTP status of the final hop, when any.
    pub http_status: Option<u16>,
    /// Bytes received in the attempt.
    pub bytes_received: u64,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Average rate.
    pub avg_rate_bps: Option<u64>,
    /// Outcome.
    pub outcome: AttemptOutcome,
    /// Failure classification.
    pub error_kind: Option<DownloadErrorKind>,
    /// Failure detail.
    pub error_detail: Option<String>,
    /// Scheduled retry.
    pub next_attempt_at: Option<OffsetDateTime>,
}

/// Closes an attempt.
pub async fn finish_attempt(
    conn: &mut SqliteConnection,
    id: AttemptId,
    end: &AttemptEnd,
) -> Result<()> {
    sqlx::query(
        "UPDATE download_attempts SET finished_at = ?1, http_status = ?2, bytes_received = ?3, \
         duration_ms = ?4, avg_rate_bps = ?5, outcome = ?6, error_kind = ?7, error_detail = ?8, \
         next_attempt_at = ?9 WHERE id = ?10",
    )
    .bind(to_db_ts(end.finished_at))
    .bind(end.http_status.map(i64::from))
    .bind(i64_from_u64(end.bytes_received))
    .bind(i64_from_u64(end.duration_ms))
    .bind(opt_i64(end.avg_rate_bps))
    .bind(end.outcome.as_str())
    .bind(end.error_kind.map(DownloadErrorKind::as_str))
    .bind(&end.error_detail)
    .bind(end.next_attempt_at.map(to_db_ts))
    .bind(id.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// Closes every open attempt of a job with `outcome` (startup recovery).
pub async fn close_open_attempts(
    conn: &mut SqliteConnection,
    job_id: JobId,
    outcome: AttemptOutcome,
    now: OffsetDateTime,
) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE download_attempts SET finished_at = ?1, outcome = ?2 WHERE job_id = ?3 AND finished_at IS NULL",
    )
    .bind(to_db_ts(now))
    .bind(outcome.as_str())
    .bind(job_id.to_string())
    .execute(conn)
    .await?
    .rows_affected())
}

/// Attempts of a job, oldest first.
pub async fn list_attempts(
    conn: &mut SqliteConnection,
    job_id: JobId,
) -> Result<Vec<DownloadAttempt>> {
    let rows: Vec<AttemptRow> = sqlx::query_as(&format!(
        "SELECT {ATTEMPT_COLUMNS} FROM download_attempts WHERE job_id = ?1 ORDER BY attempt_no"
    ))
    .bind(job_id.to_string())
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(AttemptRow::into_model).collect()
}

// ---- control
/// The global control row.
pub async fn control_get(conn: &mut SqliteConnection) -> Result<DownloadControl> {
    let (paused, reason, paused_at, updated_at): (i64, Option<String>, Option<String>, String) =
        sqlx::query_as(
            "SELECT paused, paused_reason, paused_at, updated_at FROM download_control WHERE id = 1",
        )
        .fetch_one(conn)
        .await?;
    Ok(DownloadControl {
        paused: paused != 0,
        paused_reason: reason
            .as_deref()
            .map(|r| {
                row::parse_enum(
                    "download_control",
                    "1",
                    "paused_reason",
                    r,
                    PauseAllReason::parse,
                )
            })
            .transpose()?,
        paused_at: row::opt_ts("download_control", "1", paused_at.as_deref())?,
        updated_at: row::ts("download_control", "1", &updated_at)?,
    })
}

/// Sets the global pause flag.
pub async fn control_set(
    conn: &mut SqliteConnection,
    paused: bool,
    reason: Option<PauseAllReason>,
    now: OffsetDateTime,
) -> Result<DownloadControl> {
    sqlx::query(
        "UPDATE download_control SET paused = ?1, paused_reason = ?2, \
         paused_at = CASE WHEN ?1 = 1 THEN COALESCE(paused_at, ?3) ELSE NULL END, updated_at = ?3 \
         WHERE id = 1",
    )
    .bind(i64::from(paused))
    .bind(if paused {
        reason.map(PauseAllReason::as_str)
    } else {
        None
    })
    .bind(to_db_ts(now))
    .execute(&mut *conn)
    .await?;
    control_get(conn).await
}

#[cfg(test)]
pub(crate) fn sample_job(
    podcast_id: PodcastId,
    episode_id: EpisodeId,
    priority: Priority,
    created_at: OffsetDateTime,
) -> DownloadJob {
    let id = JobId::new();
    DownloadJob {
        id,
        episode_id,
        podcast_id,
        enclosure_id: None,
        source_url: url::Url::parse("https://cdn.example/a.mp3").unwrap_or_else(|_| unreachable!()),
        host_key: "https://cdn.example:443".to_owned(),
        state: DownloadState::Queued,
        state_reason: Some("user".to_owned()),
        priority,
        attempt_count: 0,
        max_attempts: 3,
        next_attempt_at: None,
        bytes_downloaded: 0,
        total_bytes: None,
        part_path: format!("{podcast_id}/.uguisu-tmp/{id}.part"),
        target_path: format!("{podcast_id}/{episode_id}.mp3"),
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
        created_at,
        updated_at: created_at,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::many_single_char_names,
        clippy::similar_names
    )]

    use super::*;
    use crate::Storage;
    use time::Duration as TimeDuration;

    async fn seed(s: &Storage, n: usize) -> (PodcastId, Vec<EpisodeId>) {
        let p = crate::podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        let mut eps = Vec::new();
        for i in 0..n {
            let e = crate::episodes::sample(
                p.id,
                &format!("guid:{i}"),
                &format!("Episode {i}"),
                OffsetDateTime::now_utc(),
            );
            crate::episodes::upsert_all(&mut tx, std::slice::from_ref(&e))
                .await
                .unwrap();
            eps.push(e.id);
        }
        tx.commit().await.unwrap();
        (p.id, eps)
    }

    #[tokio::test]
    async fn claim_order_is_priority_then_age() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s, 5).await;
        let t0 = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let specs = [
            (Priority::Normal, 2),
            (Priority::High, 3),
            (Priority::Low, 0),
            (Priority::High, 1),
            (Priority::Normal, 2),
        ];
        let mut jobs = Vec::new();
        let mut tx = s.begin().await.unwrap();
        for (i, (prio, secs)) in specs.iter().enumerate() {
            let j = sample_job(p, eps[i], *prio, t0 + TimeDuration::seconds(*secs));
            insert_job(&mut tx, &j).await.unwrap();
            jobs.push(j);
        }
        tx.commit().await.unwrap();
        // Expected: High@1 (job 3), High@3 (job 1), Normal@2 (jobs 0 and 4 by id), Low (job 2).
        let mut expected = vec![jobs[3].id, jobs[1].id];
        let mut normals = vec![jobs[0].id, jobs[4].id];
        normals.sort();
        expected.extend(normals);
        expected.push(jobs[2].id);
        let now = t0 + TimeDuration::minutes(1);
        let mut got = Vec::new();
        let mut w = s.writer().await.unwrap();
        while let Some(j) = select_claimable(&mut w, now, &[]).await.unwrap() {
            assert!(claim(&mut w, j.id, now).await.unwrap());
            got.push(j.id);
        }
        assert_eq!(got, expected);
        // A second claim of a claimed job fails.
        assert!(!claim(&mut w, jobs[0].id, now).await.unwrap());
        let j = get_job(&mut w, jobs[0].id).await.unwrap().unwrap();
        assert_eq!(j.state, DownloadState::Downloading);
        assert_eq!(j.attempt_count, 1);
        assert!(j.started_at.is_some());
    }

    #[tokio::test]
    async fn host_exclusion_and_retry_due() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s, 3).await;
        let t0 = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let mut a = sample_job(p, eps[0], Priority::High, t0);
        a.host_key = "https://a.example:443".to_owned();
        let mut b = sample_job(p, eps[1], Priority::Normal, t0);
        b.host_key = "https://b.example:443".to_owned();
        let mut c = sample_job(p, eps[2], Priority::High, t0);
        c.state = DownloadState::Retrying;
        c.next_attempt_at = Some(t0 + TimeDuration::minutes(10));
        let mut tx = s.begin().await.unwrap();
        for j in [&a, &b, &c] {
            insert_job(&mut tx, j).await.unwrap();
        }
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();
        let next = select_claimable(&mut w, t0, &["https://a.example:443".to_owned()])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.id, b.id, "a is excluded, c is not due yet");
        let next = select_claimable(&mut w, t0 + TimeDuration::minutes(10), &[])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            next.id, a.id,
            "both high; a is older by id order? same ts → id"
        );
        assert_eq!(
            next_retry_due(&mut w).await.unwrap(),
            Some(t0 + TimeDuration::minutes(10))
        );
        let none = select_claimable(
            &mut w,
            t0,
            &[
                "https://a.example:443".to_owned(),
                "https://b.example:443".to_owned(),
            ],
        )
        .await
        .unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn transitions_are_compare_and_set() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s, 1).await;
        let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let j = sample_job(p, eps[0], Priority::Normal, now);
        let mut tx = s.begin().await.unwrap();
        insert_job(&mut tx, &j).await.unwrap();
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();
        // Cancel from queued: allowed.
        let t = Transition {
            to: DownloadState::Cancelled,
            reason: Some("user"),
            finished: true,
            ..Transition::default()
        };
        assert!(
            transition(&mut w, j.id, &[DownloadState::Queued], &t, now)
                .await
                .unwrap()
        );
        // The worker's CAS from downloading now fails.
        let t2 = Transition {
            to: DownloadState::Retrying,
            reason: Some("network"),
            next_attempt_at: Some(now + TimeDuration::seconds(30)),
            error: Some((DownloadErrorKind::Network, "reset")),
            http_status: Some(200),
            bytes_downloaded: Some(10),
            ..Transition::default()
        };
        assert!(
            !transition(&mut w, j.id, &[DownloadState::Downloading], &t2, now)
                .await
                .unwrap()
        );
        let got = get_job(&mut w, j.id).await.unwrap().unwrap();
        assert_eq!(got.state, DownloadState::Cancelled);
        assert_eq!(got.state_reason.as_deref(), Some("user"));
        assert!(got.finished_at.is_some());
        assert_eq!(got.bytes_downloaded, 0);
        // Requeue from cancelled resets the budget.
        assert!(
            requeue(
                &mut w,
                j.id,
                &[DownloadState::Failed, DownloadState::Cancelled],
                "requeued",
                Some(Priority::High),
                now
            )
            .await
            .unwrap()
        );
        let got = get_job(&mut w, j.id).await.unwrap().unwrap();
        assert_eq!(got.state, DownloadState::Queued);
        assert_eq!(got.priority, Priority::High);
        assert!(got.finished_at.is_none());
        // Progress and response metadata round trip.
        set_progress(&mut w, j.id, 512, Some(1024), now)
            .await
            .unwrap();
        set_response_meta(
            &mut w,
            j.id,
            &ResponseMeta {
                content_type: Some("audio/mpeg".into()),
                etag: Some("\"e\"".into()),
                last_modified: None,
                accept_ranges: Some(true),
                total_bytes: None,
                http_status: 206,
            },
            now,
        )
        .await
        .unwrap();
        let got = get_job(&mut w, j.id).await.unwrap().unwrap();
        assert_eq!((got.bytes_downloaded, got.total_bytes), (512, Some(1024)));
        assert_eq!(got.accept_ranges, Some(true));
        assert_eq!(got.last_http_status, Some(206));
        assert_eq!(got.percentage(), Some(50.0));
        reset_transfer(&mut w, j.id, now).await.unwrap();
        let got = get_job(&mut w, j.id).await.unwrap().unwrap();
        assert_eq!(
            (got.bytes_downloaded, got.total_bytes, got.etag),
            (0, None, None)
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn attempts_control_listing_and_counts() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s, 3).await;
        let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let jobs: Vec<DownloadJob> = (0..3)
            .map(|i| {
                sample_job(
                    p,
                    eps[i],
                    Priority::Normal,
                    now + TimeDuration::seconds(i64::try_from(i).unwrap()),
                )
            })
            .collect();
        let mut tx = s.begin().await.unwrap();
        for j in &jobs {
            insert_job(&mut tx, j).await.unwrap();
        }
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();
        assert!(claim(&mut w, jobs[1].id, now).await.unwrap());
        let a = DownloadAttempt {
            id: AttemptId::new(),
            job_id: jobs[1].id,
            attempt_no: 1,
            started_at: now,
            finished_at: None,
            source_url: jobs[1].source_url.clone(),
            range_start: 0,
            http_status: None,
            bytes_received: 0,
            duration_ms: 0,
            avg_rate_bps: None,
            outcome: None,
            error_kind: None,
            error_detail: None,
            next_attempt_at: None,
        };
        assert_eq!(insert_attempt(&mut w, &a).await.unwrap(), 1);
        finish_attempt(
            &mut w,
            a.id,
            &AttemptEnd {
                finished_at: now + TimeDuration::seconds(2),
                http_status: Some(503),
                bytes_received: 100,
                duration_ms: 2000,
                avg_rate_bps: Some(50),
                outcome: AttemptOutcome::RetryScheduled,
                error_kind: Some(DownloadErrorKind::Http),
                error_detail: Some("503".into()),
                next_attempt_at: Some(now + TimeDuration::minutes(1)),
            },
        )
        .await
        .unwrap();
        let list = list_attempts(&mut w, jobs[1].id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].outcome, Some(AttemptOutcome::RetryScheduled));
        assert_eq!(list[0].http_status, Some(503));
        // Open attempts are closed by recovery.
        // A user retry resets `attempt_count` to 0; the attempt log keeps
        // counting up, so the unique index never collides.
        let a2 = DownloadAttempt {
            id: AttemptId::new(),
            attempt_no: 1,
            ..a.clone()
        };
        assert_eq!(insert_attempt(&mut w, &a2).await.unwrap(), 2);
        assert_eq!(
            list_attempts(&mut w, jobs[1].id)
                .await
                .unwrap()
                .iter()
                .map(|x| x.attempt_no)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            close_open_attempts(&mut w, jobs[1].id, AttemptOutcome::Interrupted, now)
                .await
                .unwrap(),
            1
        );

        // Counts and listing (newest first, keyset).
        let counts = count_by_state(&mut w).await.unwrap();
        assert_eq!(counts[&DownloadState::Queued], 2);
        assert_eq!(counts[&DownloadState::Downloading], 1);
        let page1 = list_jobs(&mut w, &JobFilter::default(), None, 2)
            .await
            .unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].id, jobs[2].id);
        let last = page1.last().unwrap();
        let page2 = list_jobs(
            &mut w,
            &JobFilter::default(),
            Some((last.created_at, last.id)),
            2,
        )
        .await
        .unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].id, jobs[0].id);
        let only_queued = list_jobs(
            &mut w,
            &JobFilter {
                state: Some(DownloadState::Queued),
                podcast_id: Some(p),
            },
            None,
            10,
        )
        .await
        .unwrap();
        assert_eq!(only_queued.len(), 2);
        assert_eq!(job_ids_owning_parts(&mut w).await.unwrap().len(), 3);
        assert_eq!(
            get_job_by_episode(&mut w, eps[2])
                .await
                .unwrap()
                .unwrap()
                .id,
            jobs[2].id
        );

        // requeue_where with a reason filter.
        let t = Transition {
            to: DownloadState::Paused,
            reason: Some("paused_all"),
            ..Transition::default()
        };
        assert!(
            transition(&mut w, jobs[1].id, &[DownloadState::Downloading], &t, now)
                .await
                .unwrap()
        );
        let t = Transition {
            to: DownloadState::Paused,
            reason: Some("user"),
            ..Transition::default()
        };
        assert!(
            transition(&mut w, jobs[0].id, &[DownloadState::Queued], &t, now)
                .await
                .unwrap()
        );
        let changed = requeue_where(
            &mut w,
            &[DownloadState::Paused],
            Some(&["paused_all", "disk_full"]),
            "resumed",
            now,
        )
        .await
        .unwrap();
        assert_eq!(changed, vec![jobs[1].id]);
        assert_eq!(
            get_job(&mut w, jobs[0].id).await.unwrap().unwrap().state,
            DownloadState::Paused,
            "a user pause is not lifted by resume-all"
        );
        assert_eq!(
            jobs_in_states(&mut w, &[DownloadState::Paused])
                .await
                .unwrap()
                .len(),
            1
        );

        // Control row.
        let c = control_get(&mut w).await.unwrap();
        assert!(!c.paused);
        let c = control_set(&mut w, true, Some(PauseAllReason::DiskFull), now)
            .await
            .unwrap();
        assert!(c.paused);
        assert_eq!(c.paused_reason, Some(PauseAllReason::DiskFull));
        assert_eq!(c.paused_at, Some(now));
        let c = control_set(&mut w, false, None, now).await.unwrap();
        assert!(!c.paused && c.paused_reason.is_none() && c.paused_at.is_none());

        // finalizing CAS
        assert!(claim(&mut w, jobs[2].id, now).await.unwrap());
        assert!(
            set_finalizing(&mut w, jobs[2].id, "ab", 1024, Some("mp3"), now)
                .await
                .unwrap()
        );
        assert!(
            !set_finalizing(&mut w, jobs[2].id, "ab", 1024, Some("mp3"), now)
                .await
                .unwrap()
        );
        let got = get_job(&mut w, jobs[2].id).await.unwrap().unwrap();
        assert_eq!(got.state, DownloadState::Finalizing);
        assert_eq!(got.hash_value.as_deref(), Some("ab"));
        assert_eq!(got.total_bytes, Some(1024));
    }
}
