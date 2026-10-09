//! `feed_fetches` repository: the rolling per-podcast fetch log.

use sqlx::{FromRow, SqliteConnection};
use uguisu_core::feed::{
    EpisodeCounts, FeedFetch, FetchErrorKind, HttpSummary, NotModifiedReason, RefreshOutcome,
};
use uguisu_core::ids::{FetchId, PodcastId, SourceId};

use crate::row::{self, i64_from_u64, opt_url, to_json};
use crate::{Result, to_db_ts};

const TABLE: &str = "feed_fetches";

#[derive(FromRow)]
struct FetchRow {
    id: String,
    podcast_id: String,
    source_id: String,
    fetched_at: String,
    outcome: String,
    http_status: Option<i64>,
    etag: Option<String>,
    last_modified: Option<String>,
    bytes: Option<i64>,
    duration_ms: i64,
    error_kind: Option<String>,
    error_detail: Option<String>,
    partial: i64,
    truncated: i64,
    fingerprint_changed: i64,
    items_seen: i64,
    items_added: i64,
    items_updated: i64,
    items_unchanged: i64,
    items_malformed: i64,
    items_removed: i64,
    items_ambiguous: i64,
    podcast_changed: i64,
    url_change_detected: i64,
    warnings: String,
}

fn count(n: i64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl FetchRow {
    fn into_model(self) -> Result<FeedFetch> {
        let rid = self.id.as_str();
        let outcome = match self.outcome.as_str() {
            "fetched" => RefreshOutcome::Fetched,
            "not_modified" => RefreshOutcome::NotModified {
                reason: if self.http_status == Some(304) {
                    NotModifiedReason::Http304
                } else {
                    NotModifiedReason::Fingerprint
                },
            },
            "failed" => RefreshOutcome::Failed {
                kind: self
                    .error_kind
                    .as_deref()
                    .and_then(FetchErrorKind::parse)
                    .unwrap_or(FetchErrorKind::NetworkError),
                detail: self.error_detail.clone().unwrap_or_default(),
            },
            other => {
                return Err(row::corrupt(
                    TABLE,
                    rid,
                    format!("unknown outcome `{other}`"),
                ));
            }
        };
        Ok(FeedFetch {
            id: row::id(TABLE, rid, rid)?,
            podcast_id: row::id(TABLE, rid, &self.podcast_id)?,
            source_id: row::id(TABLE, rid, &self.source_id)?,
            fetched_at: row::ts(TABLE, rid, &self.fetched_at)?,
            outcome,
            http: HttpSummary {
                status: self.http_status.and_then(|n| u16::try_from(n).ok()),
                final_url: None,
                redirects: 0,
                etag_changed: false,
                etag: self.etag.clone(),
                last_modified: self.last_modified.clone(),
                bytes: row::u64_from(self.bytes),
                conditional: false,
            },
            duration_ms: u64::try_from(self.duration_ms).unwrap_or(0),
            partial: self.partial != 0,
            truncated: self.truncated != 0,
            fingerprint_changed: self.fingerprint_changed != 0,
            episodes: EpisodeCounts {
                seen: count(self.items_seen),
                added: count(self.items_added),
                updated: count(self.items_updated),
                unchanged: count(self.items_unchanged),
                malformed: count(self.items_malformed),
                removed_detected: count(self.items_removed),
                ambiguous: count(self.items_ambiguous),
            },
            podcast_changed: self.podcast_changed != 0,
            url_change_detected: self.url_change_detected != 0,
            warnings: row::json(TABLE, "warnings", rid, &self.warnings)?,
        })
    }
}

/// Largest `IN (…)` list, well inside SQLite's variable limit.
const CHUNK: usize = 500;

const COLUMNS: &str = "id, podcast_id, source_id, fetched_at, outcome, http_status, etag, last_modified, \
    bytes, duration_ms, error_kind, error_detail, partial, truncated, fingerprint_changed, items_seen, \
    items_added, items_updated, items_unchanged, items_malformed, items_removed, items_ambiguous, \
    podcast_changed, url_change_detected, warnings";

/// Inserts a fetch record.
pub async fn insert(conn: &mut SqliteConnection, f: &FeedFetch) -> Result<()> {
    let (outcome, error_kind, error_detail) = match &f.outcome {
        RefreshOutcome::Fetched => ("fetched", None, None),
        RefreshOutcome::NotModified { .. } => ("not_modified", None, None),
        RefreshOutcome::Failed { kind, detail } => {
            ("failed", Some(kind.as_str()), Some(detail.as_str()))
        }
    };
    sqlx::query(&format!(
        "INSERT INTO feed_fetches ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)"
    ))
    .bind(f.id.to_string())
    .bind(f.podcast_id.to_string())
    .bind(f.source_id.to_string())
    .bind(to_db_ts(f.fetched_at))
    .bind(outcome)
    .bind(f.http.status.map(i64::from))
    .bind(&f.http.etag)
    .bind(&f.http.last_modified)
    .bind(f.http.bytes.map(i64_from_u64))
    .bind(i64_from_u64(f.duration_ms))
    .bind(error_kind)
    .bind(error_detail)
    .bind(i64::from(f.partial))
    .bind(i64::from(f.truncated))
    .bind(i64::from(f.fingerprint_changed))
    .bind(i64::from(f.episodes.seen))
    .bind(i64::from(f.episodes.added))
    .bind(i64::from(f.episodes.updated))
    .bind(i64::from(f.episodes.unchanged))
    .bind(i64::from(f.episodes.malformed))
    .bind(i64::from(f.episodes.removed_detected))
    .bind(i64::from(f.episodes.ambiguous))
    .bind(i64::from(f.podcast_changed))
    .bind(i64::from(f.url_change_detected))
    .bind(to_json(&f.warnings))
    .execute(conn)
    .await?;
    Ok(())
}

/// Keeps only the newest `keep` rows of a podcast.
pub async fn trim(conn: &mut SqliteConnection, podcast_id: PodcastId, keep: u32) -> Result<u64> {
    let n = sqlx::query(
        "DELETE FROM feed_fetches WHERE podcast_id = ?1 AND id NOT IN \
         (SELECT id FROM feed_fetches WHERE podcast_id = ?1 ORDER BY fetched_at DESC, id DESC LIMIT ?2)",
    )
    .bind(podcast_id.to_string())
    .bind(i64::from(keep))
    .execute(conn)
    .await?
    .rows_affected();
    Ok(n)
}

/// Newest fetches of a podcast.
pub async fn list(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    limit: u32,
) -> Result<Vec<FeedFetch>> {
    let rows: Vec<FetchRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM feed_fetches WHERE podcast_id = ?1 ORDER BY fetched_at DESC, id DESC LIMIT ?2"
    ))
    .bind(podcast_id.to_string())
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(FetchRow::into_model).collect()
}

/// The newest fetch of each source in `ids`, keyed by source.
///
/// SQLite's documented bare-column-with-`max()` form: in a `GROUP BY` whose
/// select list holds exactly one `max()`, the other columns come from the row
/// that held the maximum. One statement for a whole page, and it needs
/// `idx_fetches_source` (0006) — before that index existed, asking this per
/// podcast scanned the entire fetch log once per row of the podcast list.
pub async fn latest_for_sources(
    conn: &mut SqliteConnection,
    ids: &[SourceId],
) -> Result<std::collections::HashMap<SourceId, FeedFetch>> {
    let mut out = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    for chunk in ids.chunks(CHUNK) {
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            // `max(fetched_at)` is aliased and never read: it is there so
            // SQLite picks the maximum row's values for the bare columns.
            "SELECT {COLUMNS}, max(fetched_at) AS newest FROM feed_fetches \
             WHERE source_id IN ({placeholders}) GROUP BY source_id"
        );
        let mut q = sqlx::query_as::<_, FetchRow>(&sql);
        for id in chunk {
            q = q.bind(id.to_string());
        }
        for row in q.fetch_all(&mut *conn).await? {
            let fetch = FetchRow::into_model(row)?;
            out.insert(fetch.source_id, fetch);
        }
    }
    Ok(out)
}

/// Newest fetch of a source.
pub async fn latest_for_source(
    conn: &mut SqliteConnection,
    source_id: SourceId,
) -> Result<Option<FeedFetch>> {
    let row: Option<FetchRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM feed_fetches WHERE source_id = ?1 ORDER BY fetched_at DESC, id DESC LIMIT 1"
    ))
    .bind(source_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(FetchRow::into_model).transpose()
}

/// Loads one fetch record.
pub async fn get(conn: &mut SqliteConnection, id: FetchId) -> Result<Option<FeedFetch>> {
    let row: Option<FetchRow> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM feed_fetches WHERE id = ?1"))
            .bind(id.to_string())
            .fetch_optional(conn)
            .await?;
    row.map(FetchRow::into_model).transpose()
}

#[allow(dead_code)]
fn _unused(_: Option<url::Url>) {
    let _ = opt_url(None);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Storage;

    fn fetch(podcast_id: PodcastId, source_id: SourceId, i: u32) -> FeedFetch {
        FeedFetch {
            id: FetchId::new(),
            podcast_id,
            source_id,
            fetched_at: time::OffsetDateTime::now_utc() + time::Duration::seconds(i64::from(i)),
            outcome: if i.is_multiple_of(3) {
                RefreshOutcome::Failed {
                    kind: FetchErrorKind::Timeout,
                    detail: "slow".into(),
                }
            } else {
                RefreshOutcome::Fetched
            },
            http: HttpSummary {
                status: Some(200),
                bytes: Some(10),
                ..HttpSummary::default()
            },
            duration_ms: 5,
            partial: false,
            truncated: false,
            fingerprint_changed: true,
            episodes: EpisodeCounts {
                seen: 3,
                added: 1,
                ..EpisodeCounts::default()
            },
            podcast_changed: false,
            url_change_detected: false,
            warnings: vec!["w".into()],
        }
    }

    #[tokio::test]
    async fn insert_list_trim_latest() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let src = crate::sources::sample(p.id, "https://example.test/f");
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        crate::sources::insert(&mut tx, &src).await.unwrap();
        for i in 0..7 {
            insert(&mut tx, &fetch(p.id, src.id, i)).await.unwrap();
        }
        let removed = trim(&mut tx, p.id, 5).await.unwrap();
        assert_eq!(removed, 2);
        tx.commit().await.unwrap();
        let mut r = s.reader().await.unwrap();
        let rows = list(&mut r, p.id, 10).await.unwrap();
        assert_eq!(rows.len(), 5);
        assert!(rows[0].fetched_at >= rows[1].fetched_at, "newest first");
        assert_eq!(rows[0].episodes.added, 1);
        assert_eq!(rows[0].warnings, vec!["w"]);
        let latest = latest_for_source(&mut r, src.id).await.unwrap().unwrap();
        assert_eq!(latest.id, rows[0].id);
        let failed = rows
            .iter()
            .find(|f| matches!(f.outcome, RefreshOutcome::Failed { .. }))
            .unwrap();
        assert!(matches!(
            &failed.outcome,
            RefreshOutcome::Failed { kind: FetchErrorKind::Timeout, detail } if detail == "slow"
        ));
        assert!(get(&mut r, latest.id).await.unwrap().is_some());
    }
}
