//! `discovery_cache` and `discovery_records` (ADR 0030).
//!
//! Both are derived data: the cache is a copy of what a provider said,
//! and a record is a note of what a resolution decided. Losing either
//! costs a round trip and some history, never a podcast — nothing here is
//! ever read back as a feed source.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::ids::{DiscoveryRecordId, PodcastId};
use uguisu_core::provider::{DiscoveryRecord, ResolutionOutcome};

use crate::row::{self, to_json};
use crate::{Result, to_db_ts};

const TABLE: &str = "discovery_records";

/// A cache row that has not expired.
pub struct CachedRow {
    /// The payload as it was stored.
    pub payload: String,
    /// When it stops being usable.
    pub expires_at: OffsetDateTime,
}

/// Reads a cache entry, expired ones included — the caller decides, and
/// the engine's port checks `expires_at` before using one.
pub async fn get_cache(
    conn: &mut SqliteConnection,
    provider: &str,
    kind: &str,
    query_key: &str,
) -> Result<Option<CachedRow>> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT payload, expires_at FROM discovery_cache \
          WHERE provider = ?1 AND kind = ?2 AND query_key = ?3",
    )
    .bind(provider)
    .bind(kind)
    .bind(query_key)
    .fetch_optional(conn)
    .await?;
    row.map(|(payload, expires_at)| {
        Ok(CachedRow {
            payload,
            expires_at: row::ts("discovery_cache", query_key, &expires_at)?,
        })
    })
    .transpose()
}

/// Writes a cache entry, replacing any previous one for the same key.
pub async fn put_cache(
    conn: &mut SqliteConnection,
    provider: &str,
    kind: &str,
    query_key: &str,
    payload: &str,
    fetched_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO discovery_cache (provider, kind, query_key, payload, fetched_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (provider, kind, query_key) \
         DO UPDATE SET payload = ?4, fetched_at = ?5, expires_at = ?6",
    )
    .bind(provider)
    .bind(kind)
    .bind(query_key)
    .bind(payload)
    .bind(to_db_ts(fetched_at))
    .bind(to_db_ts(expires_at))
    .execute(conn)
    .await?;
    Ok(())
}

/// Deletes cache rows whose time has passed.
///
/// Expiry is enforced on read as well (a row that outlived its
/// `expires_at` is a miss), so this is housekeeping rather than
/// correctness: without it a cache of searches nobody repeats would grow
/// for ever.
pub async fn expire_cache(conn: &mut SqliteConnection, now: OffsetDateTime) -> Result<u64> {
    let n = sqlx::query("DELETE FROM discovery_cache WHERE expires_at <= ?1")
        .bind(to_db_ts(now))
        .execute(conn)
        .await?
        .rows_affected();
    Ok(n)
}

#[derive(FromRow)]
struct RecordRow {
    id: String,
    input: String,
    provider: Option<String>,
    provider_ref: Option<String>,
    feed_url: Option<String>,
    website: Option<String>,
    status: String,
    detail: Option<String>,
    steps: String,
    warnings: String,
    podcast_id: Option<String>,
    resolved_at: String,
    created_at: String,
}

impl RecordRow {
    fn into_model(self) -> Result<DiscoveryRecord> {
        let rid = self.id.as_str();
        Ok(DiscoveryRecord {
            id: row::id(TABLE, rid, rid)?,
            input: self.input,
            provider: self.provider,
            provider_ref: self.provider_ref,
            feed_url: self.feed_url,
            website: self.website,
            status: row::parse_enum(TABLE, rid, "status", &self.status, ResolutionOutcome::parse)?,
            detail: self.detail,
            steps: row::json(TABLE, "steps", rid, &self.steps)?,
            warnings: row::json(TABLE, "warnings", rid, &self.warnings)?,
            podcast_id: row::opt_id(TABLE, rid, self.podcast_id.as_deref())?,
            resolved_at: row::ts(TABLE, rid, &self.resolved_at)?,
            created_at: row::ts(TABLE, rid, &self.created_at)?,
        })
    }
}

const RECORD_COLUMNS: &str = "id, input, provider, provider_ref, feed_url, website, status, \
    detail, steps, warnings, podcast_id, resolved_at, created_at";

/// Records what a resolution decided.
///
/// Provenance only: nothing reads these rows back as a feed source, and
/// a row never turns into a podcast by itself.
pub async fn insert_record(conn: &mut SqliteConnection, record: &DiscoveryRecord) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO discovery_records ({RECORD_COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
    ))
    .bind(record.id.to_string())
    .bind(&record.input)
    .bind(&record.provider)
    .bind(&record.provider_ref)
    .bind(&record.feed_url)
    .bind(&record.website)
    .bind(record.status.as_str())
    .bind(&record.detail)
    .bind(to_json(&record.steps))
    .bind(to_json(&record.warnings))
    .bind(record.podcast_id.map(|id| id.to_string()))
    .bind(to_db_ts(record.resolved_at))
    .bind(to_db_ts(record.created_at))
    .execute(conn)
    .await?;
    Ok(())
}

/// The most recent resolutions, newest first.
pub async fn list_records(conn: &mut SqliteConnection, limit: u32) -> Result<Vec<DiscoveryRecord>> {
    let rows: Vec<RecordRow> = sqlx::query_as(&format!(
        "SELECT {RECORD_COLUMNS} FROM discovery_records ORDER BY resolved_at DESC, id DESC LIMIT ?1"
    ))
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(RecordRow::into_model).collect()
}

/// One resolution by id.
pub async fn get_record(
    conn: &mut SqliteConnection,
    id: DiscoveryRecordId,
) -> Result<Option<DiscoveryRecord>> {
    let row: Option<RecordRow> = sqlx::query_as(&format!(
        "SELECT {RECORD_COLUMNS} FROM discovery_records WHERE id = ?1"
    ))
    .bind(id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(RecordRow::into_model).transpose()
}

/// Links a resolution to the podcast it produced.
pub async fn attach_podcast(
    conn: &mut SqliteConnection,
    id: DiscoveryRecordId,
    podcast_id: PodcastId,
) -> Result<bool> {
    let n = sqlx::query("UPDATE discovery_records SET podcast_id = ?2 WHERE id = ?1")
        .bind(id.to_string())
        .bind(podcast_id.to_string())
        .execute(conn)
        .await?
        .rows_affected();
    Ok(n == 1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Storage;

    #[tokio::test]
    async fn expiry_removes_only_what_has_expired() {
        let store = Storage::open_temp().await.unwrap();
        let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let mut writer = store.writer().await.unwrap();
        for (key, expires) in [
            ("stale", now - time::Duration::hours(1)),
            ("exactly_now", now),
            ("fresh", now + time::Duration::hours(1)),
        ] {
            sqlx::query(
                "INSERT INTO discovery_cache (provider, kind, query_key, payload, fetched_at, expires_at) \
                 VALUES ('apple', 'search', ?1, '{}', ?2, ?3)",
            )
            .bind(key)
            .bind(to_db_ts(now - time::Duration::hours(2)))
            .bind(to_db_ts(expires))
            .execute(&mut *writer)
            .await
            .unwrap();
        }
        assert_eq!(expire_cache(&mut writer, now).await.unwrap(), 2);
        let left: Vec<String> = sqlx::query_scalar("SELECT query_key FROM discovery_cache")
            .fetch_all(&mut *writer)
            .await
            .unwrap();
        assert_eq!(left, vec!["fresh"]);
    }
}
