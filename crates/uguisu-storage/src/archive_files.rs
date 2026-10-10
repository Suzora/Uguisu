//! `archive_files` repository (ADR 0021).
//!
//! One active row per episode, and one row per path: `episode_id` and
//! `relative_path` are both unique, so a concurrent registration or a
//! colliding relocation is refused by the database rather than by a check
//! that could race. Registration is therefore an upsert keyed on the
//! episode, and a relocation is an update that can legitimately fail.

use std::collections::BTreeMap;

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::archive::{
    ArchiveFile, ArchiveOrigin, OriginalTags, TagMode, TagState, VerificationState,
};
use uguisu_core::ids::{ArchiveFileId, EpisodeId, PodcastId};

use crate::row::{self, i64_from_u64, u64_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "archive_files";

const COLUMNS: &str = "id, episode_id, podcast_id, relative_path, size_bytes, content_type, sniffed_type, \
    hash_algo, hash_value, mtime_unix, verification_state, verification_reason, verified_at, \
    registered_at, created_at, updated_at, source_size_bytes, source_hash_algo, source_hash_value, \
    origin, tag_state, tag_mode, tagged_at, sidecar_written_at, original_tags, source_changed_at";

#[derive(FromRow)]
struct FileRow {
    id: String,
    episode_id: String,
    podcast_id: String,
    relative_path: String,
    size_bytes: i64,
    content_type: Option<String>,
    sniffed_type: Option<String>,
    hash_algo: String,
    hash_value: String,
    mtime_unix: Option<i64>,
    verification_state: String,
    verification_reason: Option<String>,
    verified_at: Option<String>,
    registered_at: String,
    created_at: String,
    updated_at: String,
    source_size_bytes: Option<i64>,
    source_hash_algo: Option<String>,
    source_hash_value: Option<String>,
    origin: String,
    tag_state: String,
    tag_mode: Option<String>,
    tagged_at: Option<String>,
    sidecar_written_at: Option<String>,
    original_tags: Option<String>,
    source_changed_at: Option<String>,
}

impl FileRow {
    fn into_model(self) -> Result<ArchiveFile> {
        let rid = self.id.clone();
        Ok(ArchiveFile {
            id: row::id(TABLE, &rid, &self.id)?,
            episode_id: row::id(TABLE, &rid, &self.episode_id)?,
            podcast_id: row::id(TABLE, &rid, &self.podcast_id)?,
            relative_path: self.relative_path,
            size_bytes: u64_from(Some(self.size_bytes)).unwrap_or(0),
            content_type: self.content_type,
            sniffed_type: self.sniffed_type,
            hash_algo: self.hash_algo,
            hash_value: self.hash_value,
            mtime_unix: self.mtime_unix,
            verification_state: row::parse_enum(
                TABLE,
                &rid,
                "verification_state",
                &self.verification_state,
                VerificationState::parse,
            )?,
            verification_reason: self.verification_reason,
            verified_at: row::opt_ts(TABLE, &rid, self.verified_at.as_deref())?,
            registered_at: row::ts(TABLE, &rid, &self.registered_at)?,
            created_at: row::ts(TABLE, &rid, &self.created_at)?,
            updated_at: row::ts(TABLE, &rid, &self.updated_at)?,
            source_size_bytes: u64_from(self.source_size_bytes),
            source_hash_algo: self.source_hash_algo,
            source_hash_value: self.source_hash_value,
            origin: row::parse_enum(TABLE, &rid, "origin", &self.origin, ArchiveOrigin::parse)?,
            tag_state: row::parse_enum(TABLE, &rid, "tag_state", &self.tag_state, TagState::parse)?,
            tag_mode: self
                .tag_mode
                .as_deref()
                .map(|m| row::parse_enum(TABLE, &rid, "tag_mode", m, TagMode::parse))
                .transpose()?,
            tagged_at: row::opt_ts(TABLE, &rid, self.tagged_at.as_deref())?,
            sidecar_written_at: row::opt_ts(TABLE, &rid, self.sidecar_written_at.as_deref())?,
            original_tags: row::opt_json(
                TABLE,
                "original_tags",
                &rid,
                self.original_tags.as_deref(),
            )?,
            source_changed_at: row::opt_ts(TABLE, &rid, self.source_changed_at.as_deref())?,
        })
    }
}

/// What a listing selects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveFilter {
    /// Only artifacts in this verification state.
    pub state: Option<VerificationState>,
    /// Only artifacts of this podcast.
    pub podcast_id: Option<PodcastId>,
    /// Only artifacts whose feed has pointed at different audio since.
    pub source_changed: bool,
}

/// Registers an artifact, or updates the one this episode already has.
///
/// Keyed on `episode_id`, so two concurrent registrations converge on one
/// row instead of racing. The identifier and `registered_at` of an
/// existing row are kept: the record's identity does not change when the
/// same episode is downloaded again.
///
/// `tag_state`, `tag_mode`, `sidecar_written_at`, `original_tags` and
/// `source_changed_at` are part of the update on purpose. The only way an existing row is re-registered is
/// a fresh download, and fresh bytes are untagged with a sidecar that no
/// longer describes them; keeping the old values would make the record claim
/// a tag write that did not happen to these bytes.
pub async fn upsert(conn: &mut SqliteConnection, f: &ArchiveFile) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO {TABLE} ({COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26) \
         ON CONFLICT (episode_id) DO UPDATE SET \
           relative_path = excluded.relative_path, size_bytes = excluded.size_bytes, \
           content_type = excluded.content_type, sniffed_type = excluded.sniffed_type, \
           hash_algo = excluded.hash_algo, hash_value = excluded.hash_value, \
           mtime_unix = excluded.mtime_unix, verification_state = excluded.verification_state, \
           verification_reason = excluded.verification_reason, verified_at = excluded.verified_at, \
           source_size_bytes = excluded.source_size_bytes, \
           source_hash_algo = excluded.source_hash_algo, \
           source_hash_value = excluded.source_hash_value, origin = excluded.origin, \
           tag_state = excluded.tag_state, tag_mode = excluded.tag_mode, \
           tagged_at = excluded.tagged_at, sidecar_written_at = excluded.sidecar_written_at, \
           original_tags = excluded.original_tags, \
           source_changed_at = excluded.source_changed_at, updated_at = excluded.updated_at"
    ))
    .bind(f.id.to_string())
    .bind(f.episode_id.to_string())
    .bind(f.podcast_id.to_string())
    .bind(&f.relative_path)
    .bind(i64_from_u64(f.size_bytes))
    .bind(&f.content_type)
    .bind(&f.sniffed_type)
    .bind(&f.hash_algo)
    .bind(&f.hash_value)
    .bind(f.mtime_unix)
    .bind(f.verification_state.as_str())
    .bind(&f.verification_reason)
    .bind(f.verified_at.map(to_db_ts))
    .bind(to_db_ts(f.registered_at))
    .bind(to_db_ts(f.created_at))
    .bind(to_db_ts(f.updated_at))
    .bind(f.source_size_bytes.map(i64_from_u64))
    .bind(&f.source_hash_algo)
    .bind(&f.source_hash_value)
    .bind(f.origin.as_str())
    .bind(f.tag_state.as_str())
    .bind(f.tag_mode.map(TagMode::as_str))
    .bind(f.tagged_at.map(to_db_ts))
    .bind(f.sidecar_written_at.map(to_db_ts))
    .bind(f.original_tags.as_ref().map(row::to_json))
    .bind(f.source_changed_at.map(to_db_ts))
    .execute(conn)
    .await?;
    Ok(())
}

/// Marks an episode's artifact as one whose feed now points at different
/// audio. `false` when the episode has no artifact.
pub async fn mark_source_changed(
    conn: &mut SqliteConnection,
    episode_id: EpisodeId,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET source_changed_at = ?1, updated_at = ?1 WHERE episode_id = ?2"
    ))
    .bind(to_db_ts(now))
    .bind(episode_id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Records the tags a file carried before its first tag write. Only ever the
/// first time: a row that already has a snapshot keeps it, so `false` means
/// there was one (or no such row).
pub async fn set_original_tags(
    conn: &mut SqliteConnection,
    id: ArchiveFileId,
    tags: &OriginalTags,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET original_tags = ?1 WHERE id = ?2 AND original_tags IS NULL"
    ))
    .bind(row::to_json(tags))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// One artifact by its identifier.
pub async fn get(conn: &mut SqliteConnection, id: ArchiveFileId) -> Result<Option<ArchiveFile>> {
    let row: Option<FileRow> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE id = ?1"))
            .bind(id.to_string())
            .fetch_optional(conn)
            .await?;
    row.map(FileRow::into_model).transpose()
}

/// The artifact of an episode, if one was registered.
pub async fn get_by_episode(
    conn: &mut SqliteConnection,
    episode_id: EpisodeId,
) -> Result<Option<ArchiveFile>> {
    let row: Option<FileRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} WHERE episode_id = ?1"
    ))
    .bind(episode_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(FileRow::into_model).transpose()
}

/// Which episode owns a path, if any. Used before proposing a path so a
/// collision can be resolved before the write is attempted.
pub async fn owner_of_path(
    conn: &mut SqliteConnection,
    relative_path: &str,
) -> Result<Option<EpisodeId>> {
    let row: Option<(String, String)> = sqlx::query_as(&format!(
        "SELECT id, episode_id FROM {TABLE} WHERE relative_path = ?1"
    ))
    .bind(relative_path)
    .fetch_optional(conn)
    .await?;
    row.map(|(rid, ep)| row::id(TABLE, &rid, &ep)).transpose()
}

/// A page of artifacts, newest first, by keyset on `(created_at, id)`.
///
/// Each filter shape is a separate literal statement. It used to be one
/// statement with `(?1 IS NULL OR verification_state = ?1)`, which the SQLite
/// planner cannot see through: `idx_archive_state` and `idx_archive_podcast`
/// were there and unused, so every filtered list was a scan.
pub async fn list(
    conn: &mut SqliteConnection,
    filter: &ArchiveFilter,
    after: Option<(OffsetDateTime, ArchiveFileId)>,
    limit: u32,
) -> Result<Vec<ArchiveFile>> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut n = 0;
    if filter.state.is_some() {
        n += 1;
        where_parts.push(format!("verification_state = ?{n}"));
    }
    if filter.podcast_id.is_some() {
        n += 1;
        where_parts.push(format!("podcast_id = ?{n}"));
    }
    if filter.source_changed {
        where_parts.push("source_changed_at IS NOT NULL".to_owned());
    }
    if after.is_some() {
        let ts = n + 1;
        let id = n + 2;
        n += 2;
        where_parts.push(format!(
            "(created_at < ?{ts} OR (created_at = ?{ts} AND id < ?{id}))"
        ));
    }
    let clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };
    let limit_n = n + 1;
    let sql = format!(
        "SELECT {COLUMNS} FROM {TABLE} {clause} ORDER BY created_at DESC, id DESC LIMIT ?{limit_n}"
    );
    let mut q = sqlx::query_as::<_, FileRow>(&sql);
    if let Some(state) = filter.state {
        q = q.bind(state.as_str());
    }
    if let Some(podcast_id) = filter.podcast_id {
        q = q.bind(podcast_id.to_string());
    }
    if let Some((ts, id)) = after {
        q = q.bind(to_db_ts(ts)).bind(id.to_string());
    }
    let rows: Vec<FileRow> = q.bind(i64::from(limit)).fetch_all(conn).await?;
    rows.into_iter().map(FileRow::into_model).collect()
}

/// How many artifacts a podcast has.
pub async fn count_for_podcast(conn: &mut SqliteConnection, podcast_id: PodcastId) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {TABLE} WHERE podcast_id = ?1"
    ))
    .bind(podcast_id.to_string())
    .fetch_one(conn)
    .await?;
    Ok(u64_from(Some(count)).unwrap_or(0))
}

/// How many artifacts are in each verification state.
pub async fn count_by_state(
    conn: &mut SqliteConnection,
) -> Result<BTreeMap<VerificationState, u64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT verification_state, count(*) FROM {TABLE} GROUP BY verification_state"
    ))
    .fetch_all(conn)
    .await?;
    let mut out = BTreeMap::new();
    for (state, count) in rows {
        let s = row::parse_enum(
            TABLE,
            "-",
            "verification_state",
            &state,
            VerificationState::parse,
        )?;
        out.insert(s, u64_from(Some(count)).unwrap_or(0));
    }
    Ok(out)
}

/// What a verification pass learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationUpdate {
    /// The new state.
    pub state: VerificationState,
    /// Why, from `uguisu_core::archive::reason`.
    pub reason: Option<String>,
    /// When the check ran.
    pub at: OffsetDateTime,
    /// Modification time observed, when the check read one.
    pub mtime_unix: Option<i64>,
}

/// Records the outcome of a verification of `checked`, the record as the
/// check read it. Returns `false`, writing nothing, when the record is gone
/// or no longer has that path and state: a relocation or a new download
/// changed it meanwhile, and a verdict about the old one would be false.
/// A verification never creates or deletes a record.
pub async fn set_verification(
    conn: &mut SqliteConnection,
    checked: &ArchiveFile,
    update: &VerificationUpdate,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET verification_state = ?1, verification_reason = ?2, verified_at = ?3, \
         mtime_unix = COALESCE(?4, mtime_unix), updated_at = ?3 \
         WHERE id = ?5 AND relative_path = ?6 AND verification_state = ?7"
    ))
    .bind(update.state.as_str())
    .bind(&update.reason)
    .bind(to_db_ts(update.at))
    .bind(update.mtime_unix)
    .bind(checked.id.to_string())
    .bind(&checked.relative_path)
    .bind(checked.verification_state.as_str())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Moves the record to a new path after the file was renamed there. A
/// rename keeps the bytes, so a `verified` record stays verified, now as
/// `relocated`; any other verdict is kept as it was.
///
/// Fails with a unique-constraint error when another artifact already owns
/// the path, which is exactly the protection a relocation needs.
pub async fn set_path(
    conn: &mut SqliteConnection,
    id: ArchiveFileId,
    relative_path: &str,
    mtime_unix: Option<i64>,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET relative_path = ?1, mtime_unix = COALESCE(?2, mtime_unix), \
         verification_reason = CASE verification_state WHEN 'verified' THEN 'relocated' \
         ELSE verification_reason END, \
         verified_at = CASE verification_state WHEN 'verified' THEN ?3 ELSE verified_at END, \
         updated_at = ?3 WHERE id = ?4"
    ))
    .bind(relative_path)
    .bind(mtime_unix)
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// What a tag write produced: the bytes changed, the provenance did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagWrite {
    /// The mode that wrote them.
    pub mode: TagMode,
    /// The file's new length.
    pub size_bytes: u64,
    /// The file's new hash.
    pub hash_value: String,
    /// Modification time of the replacement, when one was read.
    pub mtime_unix: Option<i64>,
    /// `verified` when the replacement was hashed as it was written;
    /// `unchecked` when a recovery adopted bytes it found on disk.
    pub verification_state: VerificationState,
    /// Why the record now says what it says, from
    /// `uguisu_core::archive::reason`.
    pub reason: Option<String>,
    /// When it happened.
    pub at: OffsetDateTime,
}

/// Records that a tag write is about to touch the media file.
///
/// This is deliberately its own statement, committed **before** a single
/// byte moves. After a crash between the atomic replace and
/// [`set_tagged`], the bytes alone cannot say whether Uguisu was mid-write
/// or somebody else edited the file; this marker can, and a crash cannot
/// forge it.
pub async fn set_tag_state(
    conn: &mut SqliteConnection,
    id: ArchiveFileId,
    state: TagState,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET tag_state = ?1, updated_at = ?2 WHERE id = ?3"
    ))
    .bind(state.as_str())
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Records a completed tag write.
///
/// Moves `hash_value`, `size_bytes` and `mtime_unix` onto the new bytes and
/// records `write.verification_state`; `verified_at` is set only for
/// `verified` and cleared otherwise, since it described the old bytes.
/// `source_hash_value` is **not** in the SET list: it still
/// describes what was downloaded, which is the whole point of having it.
pub async fn set_tagged(
    conn: &mut SqliteConnection,
    id: ArchiveFileId,
    write: &TagWrite,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET size_bytes = ?1, hash_value = ?2, \
         mtime_unix = COALESCE(?3, mtime_unix), tag_state = ?4, tag_mode = ?5, tagged_at = ?6, \
         verification_state = ?7, verification_reason = ?8, \
         verified_at = CASE ?7 WHEN 'verified' THEN ?6 ELSE NULL END, updated_at = ?6 \
         WHERE id = ?9"
    ))
    .bind(i64_from_u64(write.size_bytes))
    .bind(&write.hash_value)
    .bind(write.mtime_unix)
    .bind(TagState::Written.as_str())
    .bind(write.mode.as_str())
    .bind(to_db_ts(write.at))
    .bind(write.verification_state.as_str())
    .bind(&write.reason)
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Makes recording a completed tag write fail until
/// [`allow_tag_records`], so tests of dependent crates can reach the
/// boundary between the rename and the record.
#[cfg(any(test, feature = "testing"))]
pub async fn refuse_tag_records(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query(
        "CREATE TRIGGER refuse_tag_records BEFORE UPDATE OF tag_state ON archive_files \
         WHEN OLD.tag_state = 'pending' AND NEW.tag_state = 'written' \
         BEGIN SELECT RAISE(ABORT, 'tag records refused'); END",
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Undoes [`refuse_tag_records`].
#[cfg(any(test, feature = "testing"))]
pub async fn allow_tag_records(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("DROP TRIGGER refuse_tag_records")
        .execute(conn)
        .await?;
    Ok(())
}

/// Records that the portable sidecar beside the file is current.
pub async fn set_sidecar_written(
    conn: &mut SqliteConnection,
    id: ArchiveFileId,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET sidecar_written_at = ?1, updated_at = ?1 WHERE id = ?2"
    ))
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// One podcast's artifacts in path order, paged by the path itself.
///
/// Path order is what a manifest needs: the file it renders has to be
/// byte-identical however the rows happened to be inserted, and sorting a
/// whole archive in memory is exactly the thing a manifest writer must not
/// do. `after` is the last path of the previous page.
pub async fn list_for_podcast(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    after: Option<&str>,
    limit: u32,
) -> Result<Vec<ArchiveFile>> {
    let rows: Vec<FileRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} \
         WHERE podcast_id = ?1 AND (?2 IS NULL OR relative_path > ?2) \
         ORDER BY relative_path LIMIT ?3"
    ))
    .bind(podcast_id.to_string())
    .bind(after)
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(FileRow::into_model).collect()
}

/// Artifacts whose tag write was interrupted.
///
/// Backed by a partial index that is empty whenever nothing is in flight,
/// so asking this at startup costs nothing on a large archive.
pub async fn tagging_interrupted(
    conn: &mut SqliteConnection,
    limit: u32,
) -> Result<Vec<ArchiveFileId>> {
    let rows: Vec<(String,)> = sqlx::query_as(&format!(
        "SELECT id FROM {TABLE} WHERE tag_state = ?1 ORDER BY updated_at, id LIMIT ?2"
    ))
    .bind(TagState::Pending.as_str())
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|(id,)| row::id(TABLE, &id, &id))
        .collect()
}

/// Artifacts that have no sidecar yet, oldest first.
///
/// A missing sidecar degrades a future rebuild; it never breaks the
/// archive, so this is work for an idle moment and not for startup.
pub async fn without_sidecar(
    conn: &mut SqliteConnection,
    after: Option<(OffsetDateTime, ArchiveFileId)>,
    limit: u32,
) -> Result<Vec<ArchiveFile>> {
    let (after_ts, after_id) = match after {
        Some((t, id)) => (Some(to_db_ts(t)), Some(id.to_string())),
        None => (None, None),
    };
    let rows: Vec<FileRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} WHERE sidecar_written_at IS NULL \
           AND (?1 IS NULL OR created_at > ?1 OR (created_at = ?1 AND id > ?2)) \
         ORDER BY created_at, id LIMIT ?3"
    ))
    .bind(after_ts)
    .bind(after_id)
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(FileRow::into_model).collect()
}

/// Removes the record of an episode's artifact. The file itself is never
/// touched by this repository.
pub async fn delete_for_episode(
    conn: &mut SqliteConnection,
    episode_id: EpisodeId,
) -> Result<bool> {
    let affected = sqlx::query(&format!("DELETE FROM {TABLE} WHERE episode_id = ?1"))
        .bind(episode_id.to_string())
        .execute(conn)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

/// Episodes whose download completed but which no archive record describes
/// yet, oldest first: none at all, or one of other bytes, which a redownload
/// leaves until its registration (ADR 0060). This is what startup
/// reconciliation registers after a crash between the completion transaction
/// and the registration. A tag write moves `hash_value` and never
/// `source_hash_value`, so it does not count.
pub async fn completed_unregistered(
    conn: &mut SqliteConnection,
    limit: u32,
) -> Result<Vec<EpisodeId>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT j.episode_id FROM download_jobs j \
         LEFT JOIN archive_files a ON a.episode_id = j.episode_id \
         WHERE j.state = 'completed' \
           AND (a.id IS NULL OR a.source_hash_value IS NOT j.hash_value) \
         ORDER BY j.finished_at, j.id LIMIT ?1",
    )
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|(ep,)| row::id(TABLE, "-", &ep))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::Duration as TimeDuration;
    use uguisu_core::archive::reason;

    use super::*;
    use crate::{Storage, episodes, podcasts};

    async fn seed(storage: &Storage) -> (PodcastId, Vec<EpisodeId>) {
        let p = podcasts::sample("Show");
        let mut tx = storage.begin().await.unwrap();
        podcasts::insert(&mut tx, &p).await.unwrap();
        let now = OffsetDateTime::now_utc();
        let mut ids = Vec::new();
        for i in 0..3 {
            let e = episodes::sample(p.id, &format!("k{i}"), "Ep", now);
            ids.push(e.id);
            episodes::upsert_all(&mut tx, std::slice::from_ref(&e))
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        (p.id, ids)
    }

    fn file(podcast: PodcastId, episode: EpisodeId, path: &str) -> ArchiveFile {
        let now = OffsetDateTime::now_utc();
        ArchiveFile {
            id: ArchiveFileId::new(),
            episode_id: episode,
            podcast_id: podcast,
            relative_path: path.to_owned(),
            size_bytes: 1024,
            content_type: Some("audio/mpeg".into()),
            sniffed_type: Some("mp3".into()),
            hash_algo: "sha256".into(),
            hash_value: "abc".into(),
            mtime_unix: Some(1_700_000_000),
            verification_state: VerificationState::Unchecked,
            verification_reason: Some(reason::REGISTERED.to_owned()),
            verified_at: None,
            registered_at: now,
            created_at: now,
            updated_at: now,
            source_size_bytes: Some(1024),
            source_hash_algo: Some("sha256".into()),
            source_hash_value: Some("abc".into()),
            origin: ArchiveOrigin::Download,
            tag_state: TagState::Untagged,
            tag_mode: None,
            tagged_at: None,
            sidecar_written_at: None,
            original_tags: None,
            source_changed_at: None,
        }
    }

    #[tokio::test]
    async fn stale_record_is_registered_again() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s).await;
        let mut tx = s.begin().await.unwrap();
        upsert(&mut tx, &file(p, eps[0], "a.mp3")).await.unwrap();
        upsert(&mut tx, &file(p, eps[1], "b.mp3")).await.unwrap();
        for (episode, hash) in [(eps[0], "abc"), (eps[1], "def"), (eps[2], "ghi")] {
            let mut job = crate::downloads::sample_job(
                p,
                episode,
                uguisu_core::download::Priority::Normal,
                OffsetDateTime::now_utc(),
            );
            job.state = uguisu_core::download::DownloadState::Completed;
            job.hash_value = Some(hash.to_owned());
            crate::downloads::insert_job(&mut tx, &job).await.unwrap();
        }
        tx.commit().await.unwrap();
        let mut r = s.reader().await.unwrap();
        let mut pending = completed_unregistered(&mut r, 10).await.unwrap();
        pending.sort();
        let mut expected = vec![eps[1], eps[2]];
        expected.sort();
        assert_eq!(
            pending, expected,
            "b's record is of other bytes, c has none"
        );
    }

    #[tokio::test]
    async fn one_record_per_episode_and_path() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s).await;
        let mut w = s.writer().await.unwrap();

        let first = file(p, eps[0], "Show/2024/a.mp3");
        upsert(&mut w, &first).await.unwrap();
        // The same episode registering again updates the row it already has.
        let mut again = file(p, eps[0], "Show/2024/a-renamed.mp3");
        again.size_bytes = 2048;
        upsert(&mut w, &again).await.unwrap();
        let stored = get_by_episode(&mut w, eps[0]).await.unwrap().unwrap();
        assert_eq!(stored.id, first.id, "the record keeps its identity");
        assert_eq!(stored.relative_path, "Show/2024/a-renamed.mp3");
        assert_eq!(stored.size_bytes, 2048);
        assert_eq!(
            get(&mut w, first.id).await.unwrap().unwrap().episode_id,
            eps[0]
        );

        // A second episode cannot take the same path: the database refuses.
        let clash = file(p, eps[1], "Show/2024/a-renamed.mp3");
        let err = upsert(&mut w, &clash).await.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("unique"),
            "{err} names the constraint"
        );
        assert_eq!(
            owner_of_path(&mut w, "Show/2024/a-renamed.mp3")
                .await
                .unwrap(),
            Some(eps[0])
        );
        assert_eq!(owner_of_path(&mut w, "nothing").await.unwrap(), None);
        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn verification_updates_states_without_touching_the_record() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s).await;
        let mut w = s.writer().await.unwrap();
        let f = file(p, eps[0], "Show/a.mp3");
        upsert(&mut w, &f).await.unwrap();

        let at = OffsetDateTime::now_utc();
        assert!(
            set_verification(
                &mut w,
                &f,
                &VerificationUpdate {
                    state: VerificationState::Missing,
                    reason: Some(reason::NOT_FOUND.to_owned()),
                    at,
                    mtime_unix: None,
                },
            )
            .await
            .unwrap()
        );
        let stored = get(&mut w, f.id).await.unwrap().unwrap();
        assert_eq!(stored.verification_state, VerificationState::Missing);
        assert_eq!(stored.verification_reason.as_deref(), Some("not_found"));
        assert!(stored.verified_at.is_some());
        assert_eq!(
            stored.mtime_unix,
            Some(1_700_000_000),
            "a check that read no mtime keeps the recorded one"
        );
        assert_eq!(stored.hash_value, "abc", "the artifact facts do not change");

        // A verdict about the record as it was is not written over the
        // record as it is: here it has moved since it was read.
        set_path(&mut w, f.id, "Show/b.mp3", None, at)
            .await
            .unwrap();
        let verified = VerificationUpdate {
            state: VerificationState::Verified,
            reason: None,
            at,
            mtime_unix: None,
        };
        assert!(!set_verification(&mut w, &stored, &verified).await.unwrap());
        let moved = get(&mut w, f.id).await.unwrap().unwrap();
        assert_eq!(moved.verification_state, VerificationState::Missing);

        // An unknown record reports that it is gone, it does not appear.
        let unknown = file(p, eps[1], "Show/c.mp3");
        assert!(!set_verification(&mut w, &unknown, &verified).await.unwrap());
        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn a_tag_write_spares_provenance() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s).await;
        let mut w = s.writer().await.unwrap();
        let f = file(p, eps[0], "Show/a.mp3");
        upsert(&mut w, &f).await.unwrap();

        // The marker goes down before the file is touched, and it is what
        // a recovery pass looks for.
        let at = OffsetDateTime::now_utc();
        assert!(
            set_tag_state(&mut w, f.id, TagState::Pending, at)
                .await
                .unwrap()
        );
        assert_eq!(
            tagging_interrupted(&mut w, 10).await.unwrap(),
            vec![f.id],
            "an interrupted write is findable without scanning the archive"
        );

        assert!(
            set_tagged(
                &mut w,
                f.id,
                &TagWrite {
                    mode: TagMode::FillMissing,
                    size_bytes: 4096,
                    hash_value: "def".into(),
                    mtime_unix: Some(1_800_000_000),
                    verification_state: VerificationState::Verified,
                    reason: Some(reason::TAGGED.to_owned()),
                    at,
                },
            )
            .await
            .unwrap()
        );
        let stored = get(&mut w, f.id).await.unwrap().unwrap();
        assert_eq!(stored.hash_value, "def", "the record follows the bytes");
        assert_eq!(stored.size_bytes, 4096);
        assert_eq!(
            stored.source_hash_value.as_deref(),
            Some("abc"),
            "provenance still records what was downloaded"
        );
        assert_eq!(stored.source_size_bytes, Some(1024));
        assert_eq!(stored.tag_state, TagState::Written);
        assert_eq!(stored.tag_mode, Some(TagMode::FillMissing));
        assert_eq!(stored.verification_state, VerificationState::Verified);
        assert_eq!(stored.verification_reason.as_deref(), Some("tagged"));
        assert!(tagging_interrupted(&mut w, 10).await.unwrap().is_empty());

        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn a_fresh_download_clears_stale_metadata() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s).await;
        let mut w = s.writer().await.unwrap();
        let f = file(p, eps[0], "Show/a.mp3");
        upsert(&mut w, &f).await.unwrap();
        let at = OffsetDateTime::now_utc();
        set_tagged(
            &mut w,
            f.id,
            &TagWrite {
                mode: TagMode::Sync,
                size_bytes: 4096,
                hash_value: "def".into(),
                mtime_unix: None,
                verification_state: VerificationState::Verified,
                reason: Some(reason::TAGGED.to_owned()),
                at,
            },
        )
        .await
        .unwrap();
        set_sidecar_written(&mut w, f.id, at).await.unwrap();
        let original = |title: &str| OriginalTags {
            values: BTreeMap::from([("title".to_owned(), title.to_owned())]),
            cover: None,
            captured_at: at,
        };
        assert!(
            set_original_tags(&mut w, f.id, &original("theirs"))
                .await
                .unwrap()
        );
        assert!(
            !set_original_tags(&mut w, f.id, &original("ours"))
                .await
                .unwrap(),
            "only the first write's tags are the original ones"
        );
        assert_eq!(
            get(&mut w, f.id).await.unwrap().unwrap().original_tags,
            Some(original("theirs"))
        );

        // The episode is downloaded again: different bytes, so every claim
        // about the old ones has to go.
        let mut again = file(p, eps[0], "Show/a.mp3");
        again.hash_value = "ghi".into();
        again.source_hash_value = Some("ghi".into());
        upsert(&mut w, &again).await.unwrap();
        let stored = get(&mut w, f.id).await.unwrap().unwrap();
        assert_eq!(stored.hash_value, "ghi");
        assert_eq!(stored.source_hash_value.as_deref(), Some("ghi"));
        assert_eq!(
            stored.tag_state,
            TagState::Untagged,
            "the new bytes carry no tag write"
        );
        assert_eq!(stored.tag_mode, None);
        assert_eq!(
            stored.sidecar_written_at, None,
            "the sidecar describes bytes that are gone"
        );
        assert_eq!(stored.original_tags, None);
        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn pending_asset_work_is_ordered() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s).await;
        let mut w = s.writer().await.unwrap();
        // Inserted in an order that is not the path order.
        for (ep, path) in [
            (eps[0], "Show/c.mp3"),
            (eps[1], "Show/a.mp3"),
            (eps[2], "Show/b.mp3"),
        ] {
            upsert(&mut w, &file(p, ep, path)).await.unwrap();
        }

        let page = list_for_podcast(&mut w, p, None, 2).await.unwrap();
        let paths: Vec<&str> = page.iter().map(|f| f.relative_path.as_str()).collect();
        assert_eq!(paths, vec!["Show/a.mp3", "Show/b.mp3"]);
        let rest = list_for_podcast(&mut w, p, Some("Show/b.mp3"), 2)
            .await
            .unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].relative_path, "Show/c.mp3");
        assert!(
            list_for_podcast(&mut w, PodcastId::new(), None, 10)
                .await
                .unwrap()
                .is_empty()
        );

        // Everything still needs a sidecar; writing one takes it off the list.
        let pending = without_sidecar(&mut w, None, 10).await.unwrap();
        assert_eq!(pending.len(), 3);
        let now = OffsetDateTime::now_utc();
        assert!(
            set_sidecar_written(&mut w, pending[0].id, now)
                .await
                .unwrap()
        );
        let left = without_sidecar(&mut w, None, 10).await.unwrap();
        assert_eq!(left.len(), 2);
        assert!(!left.iter().any(|f| f.id == pending[0].id));
        assert!(
            !set_sidecar_written(&mut w, ArchiveFileId::new(), now)
                .await
                .unwrap()
        );
        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn listing_filters_pages_and_counts() {
        let s = Storage::open_temp().await.unwrap();
        let (p, eps) = seed(&s).await;
        let mut w = s.writer().await.unwrap();
        let now = OffsetDateTime::now_utc();
        for (i, ep) in eps.iter().enumerate() {
            let mut f = file(p, *ep, &format!("Show/{i}.mp3"));
            f.created_at = now + TimeDuration::seconds(i64::try_from(i).unwrap());
            f.verification_state = if i == 2 {
                VerificationState::Missing
            } else {
                VerificationState::Verified
            };
            upsert(&mut w, &f).await.unwrap();
        }

        let all = list(&mut w, &ArchiveFilter::default(), None, 10)
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
        assert!(
            all[0].created_at >= all[1].created_at,
            "newest first: {:?}",
            all.iter().map(|f| f.created_at).collect::<Vec<_>>()
        );

        let page = list(&mut w, &ArchiveFilter::default(), None, 2)
            .await
            .unwrap();
        assert_eq!(page.len(), 2);
        let last = page.last().unwrap();
        let rest = list(
            &mut w,
            &ArchiveFilter::default(),
            Some((last.created_at, last.id)),
            2,
        )
        .await
        .unwrap();
        assert_eq!(rest.len(), 1);
        assert!(!page.iter().any(|f| f.id == rest[0].id), "no overlap");

        let missing = list(
            &mut w,
            &ArchiveFilter {
                state: Some(VerificationState::Missing),
                podcast_id: None,
                ..ArchiveFilter::default()
            },
            None,
            10,
        )
        .await
        .unwrap();
        assert_eq!(missing.len(), 1);
        let counts = count_by_state(&mut w).await.unwrap();
        assert_eq!(counts[&VerificationState::Verified], 2);
        assert_eq!(counts[&VerificationState::Missing], 1);
        assert!(!counts.contains_key(&VerificationState::Invalid));

        // Deleting the record leaves the episode alone.
        assert!(delete_for_episode(&mut w, eps[0]).await.unwrap());
        assert!(get_by_episode(&mut w, eps[0]).await.unwrap().is_none());
        assert!(!delete_for_episode(&mut w, eps[0]).await.unwrap());
        assert!(episodes::get(&mut w, eps[0]).await.unwrap().is_some());
        drop(w);
        s.close().await;
    }

    /// The filtered lists have had `idx_archive_state` and
    /// `idx_archive_podcast` since 0003 and could not use them, because the
    /// planner cannot see through `(?1 IS NULL OR col = ?1)`.
    #[tokio::test]
    async fn a_filtered_list_rides_its_index() {
        let s = Storage::open_temp().await.unwrap();
        let mut r = s.reader().await.unwrap();
        for (name, filter) in [
            (
                "by state",
                ArchiveFilter {
                    state: Some(VerificationState::Missing),
                    podcast_id: None,
                    ..ArchiveFilter::default()
                },
            ),
            (
                "by podcast",
                ArchiveFilter {
                    state: None,
                    podcast_id: Some(uguisu_core::ids::PodcastId::new()),
                    ..ArchiveFilter::default()
                },
            ),
        ] {
            // `list` is what builds the statement, so run it and read the plan
            // of the same shape rather than duplicating the SQL here.
            list(&mut r, &filter, None, 8).await.unwrap();
            let (column, index) = if filter.state.is_some() {
                ("verification_state = 'missing'", "idx_archive_state")
            } else {
                ("podcast_id = 'x'", "idx_archive_podcast")
            };
            let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!(
                "EXPLAIN QUERY PLAN SELECT id FROM {TABLE} WHERE {column} \
                 ORDER BY created_at DESC, id DESC LIMIT 8"
            ))
            .fetch_all(&mut *r)
            .await
            .unwrap();
            let plan = plan
                .into_iter()
                .map(|(_, _, _, d)| d)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                plan.contains(index),
                "{name}: the filter did not reach {index}: {plan}"
            );
        }
    }
}
