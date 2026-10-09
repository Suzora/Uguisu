//! `podcast_sources` repository: feed URLs with history and fetch state.
//!
//! A row is current (one per podcast), replaced (`replaced_by_source_id`
//! set), or announced: neither, a URL the feed names as its new home that
//! has not passed the same-show check (ADR 0052). At most one per podcast.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::feed::FetchErrorKind;
use uguisu_core::ids::{PodcastId, SourceId};
use uguisu_core::model::{FetchState, FetchStatus, PodcastSource, ReplacementReason};
use url::Url;

use crate::row::{self, i64_from_u64, opt_url, u64_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "podcast_sources";

#[derive(FromRow)]
struct SourceRow {
    id: String,
    podcast_id: String,
    feed_url: String,
    canonical_url: Option<String>,
    website_url: Option<String>,
    provider: String,
    provider_ref: Option<String>,
    discovered_at: String,
    verified_at: Option<String>,
    is_current: i64,
    replaced_by_source_id: Option<String>,
    replacement_reason: Option<String>,
    fetch_state: String,
    last_attempt_at: Option<String>,
    last_success_at: Option<String>,
    last_not_modified_at: Option<String>,
    last_error_at: Option<String>,
    consecutive_failures: i64,
    last_http_status: Option<i64>,
    last_error_kind: Option<String>,
    last_error_detail: Option<String>,
    http_etag: Option<String>,
    http_last_modified: Option<String>,
    content_fingerprint: Option<String>,
    last_content_length: Option<i64>,
    created_at: String,
    updated_at: String,
}

impl SourceRow {
    fn into_model(self) -> Result<PodcastSource> {
        let rid = self.id.as_str();
        let replacement_reason = self
            .replacement_reason
            .as_deref()
            .map(|v| {
                row::parse_enum(
                    TABLE,
                    rid,
                    "replacement_reason",
                    v,
                    ReplacementReason::parse,
                )
            })
            .transpose()?;
        let last_error_kind = self
            .last_error_kind
            .as_deref()
            .map(|v| row::parse_enum(TABLE, rid, "last_error_kind", v, FetchErrorKind::parse))
            .transpose()?;
        Ok(PodcastSource {
            id: row::id(TABLE, rid, rid)?,
            podcast_id: row::id(TABLE, rid, &self.podcast_id)?,
            feed_url: row::req_url(TABLE, rid, &self.feed_url)?,
            canonical_url: opt_url(self.canonical_url.as_deref()),
            website_url: opt_url(self.website_url.as_deref()),
            provider: self.provider.clone(),
            provider_ref: self.provider_ref.clone(),
            discovered_at: row::ts(TABLE, rid, &self.discovered_at)?,
            verified_at: row::opt_ts(TABLE, rid, self.verified_at.as_deref())?,
            is_current: self.is_current != 0,
            replaced_by_source_id: row::opt_id(TABLE, rid, self.replaced_by_source_id.as_deref())?,
            replacement_reason,
            fetch: FetchStatus {
                state: row::parse_enum(
                    TABLE,
                    rid,
                    "fetch_state",
                    &self.fetch_state,
                    FetchState::parse,
                )?,
                last_attempt_at: row::opt_ts(TABLE, rid, self.last_attempt_at.as_deref())?,
                last_success_at: row::opt_ts(TABLE, rid, self.last_success_at.as_deref())?,
                last_not_modified_at: row::opt_ts(
                    TABLE,
                    rid,
                    self.last_not_modified_at.as_deref(),
                )?,
                last_error_at: row::opt_ts(TABLE, rid, self.last_error_at.as_deref())?,
                consecutive_failures: u32::try_from(self.consecutive_failures).unwrap_or(u32::MAX),
                last_http_status: self.last_http_status.and_then(|n| u16::try_from(n).ok()),
                last_error_kind,
                last_error_detail: self.last_error_detail.clone(),
                etag: self.http_etag.clone(),
                last_modified: self.http_last_modified.clone(),
                content_fingerprint: self.content_fingerprint.clone(),
                last_content_length: u64_from(self.last_content_length),
            },
            created_at: row::ts(TABLE, rid, &self.created_at)?,
            updated_at: row::ts(TABLE, rid, &self.updated_at)?,
        })
    }
}

/// Largest `IN (…)` list, well inside SQLite's variable limit.
const CHUNK: usize = 500;

/// The rows that are announced, not current and not replaced.
const ANNOUNCED: &str = "is_current = 0 AND replaced_by_source_id IS NULL";

const COLUMNS: &str = "id, podcast_id, feed_url, canonical_url, website_url, provider, provider_ref, \
    discovered_at, verified_at, is_current, replaced_by_source_id, replacement_reason, fetch_state, \
    last_attempt_at, last_success_at, last_not_modified_at, last_error_at, consecutive_failures, \
    last_http_status, last_error_kind, last_error_detail, http_etag, http_last_modified, \
    content_fingerprint, last_content_length, created_at, updated_at";

/// Inserts a source row.
pub async fn insert(conn: &mut SqliteConnection, s: &PodcastSource) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO podcast_sources ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)"
    ))
    .bind(s.id.to_string())
    .bind(s.podcast_id.to_string())
    .bind(s.feed_url.as_str())
    .bind(s.canonical_url.as_ref().map(Url::as_str))
    .bind(s.website_url.as_ref().map(Url::as_str))
    .bind(&s.provider)
    .bind(&s.provider_ref)
    .bind(to_db_ts(s.discovered_at))
    .bind(s.verified_at.map(to_db_ts))
    .bind(i64::from(s.is_current))
    .bind(s.replaced_by_source_id.map(|i| i.to_string()))
    .bind(s.replacement_reason.map(ReplacementReason::as_str))
    .bind(s.fetch.state.as_str())
    .bind(s.fetch.last_attempt_at.map(to_db_ts))
    .bind(s.fetch.last_success_at.map(to_db_ts))
    .bind(s.fetch.last_not_modified_at.map(to_db_ts))
    .bind(s.fetch.last_error_at.map(to_db_ts))
    .bind(i64::from(s.fetch.consecutive_failures))
    .bind(s.fetch.last_http_status.map(i64::from))
    .bind(s.fetch.last_error_kind.map(FetchErrorKind::as_str))
    .bind(&s.fetch.last_error_detail)
    .bind(&s.fetch.etag)
    .bind(&s.fetch.last_modified)
    .bind(&s.fetch.content_fingerprint)
    .bind(s.fetch.last_content_length.map(i64_from_u64))
    .bind(to_db_ts(s.created_at))
    .bind(to_db_ts(s.updated_at))
    .execute(conn)
    .await?;
    Ok(())
}

/// Updates the fetch bookkeeping, validators and verification time of a source.
pub async fn update_fetch(
    conn: &mut SqliteConnection,
    id: SourceId,
    fetch: &FetchStatus,
    verified_at: Option<OffsetDateTime>,
    canonical_url: Option<&Url>,
    now: OffsetDateTime,
) -> Result<()> {
    let n = sqlx::query(
        "UPDATE podcast_sources SET fetch_state = ?2, last_attempt_at = ?3, last_success_at = ?4, \
         last_not_modified_at = ?5, last_error_at = ?6, consecutive_failures = ?7, last_http_status = ?8, \
         last_error_kind = ?9, last_error_detail = ?10, http_etag = ?11, http_last_modified = ?12, \
         content_fingerprint = ?13, last_content_length = ?14, \
         verified_at = COALESCE(?15, verified_at), canonical_url = COALESCE(?16, canonical_url), \
         updated_at = ?17 WHERE id = ?1",
    )
    .bind(id.to_string())
    .bind(fetch.state.as_str())
    .bind(fetch.last_attempt_at.map(to_db_ts))
    .bind(fetch.last_success_at.map(to_db_ts))
    .bind(fetch.last_not_modified_at.map(to_db_ts))
    .bind(fetch.last_error_at.map(to_db_ts))
    .bind(i64::from(fetch.consecutive_failures))
    .bind(fetch.last_http_status.map(i64::from))
    .bind(fetch.last_error_kind.map(FetchErrorKind::as_str))
    .bind(&fetch.last_error_detail)
    .bind(&fetch.etag)
    .bind(&fetch.last_modified)
    .bind(&fetch.content_fingerprint)
    .bind(fetch.last_content_length.map(i64_from_u64))
    .bind(verified_at.map(to_db_ts))
    .bind(canonical_url.map(Url::as_str))
    .bind(to_db_ts(now))
    .execute(conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(sqlx::Error::RowNotFound.into());
    }
    Ok(())
}

/// Sets a source's fetch state alone: `disabled` when its podcast is
/// archived, `never_fetched` when it is resumed (ADR 0055).
pub async fn set_state(
    conn: &mut SqliteConnection,
    id: SourceId,
    state: FetchState,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query("UPDATE podcast_sources SET fetch_state = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id.to_string())
        .bind(state.as_str())
        .bind(to_db_ts(now))
        .execute(conn)
        .await?;
    Ok(())
}

/// Marks a source as being fetched right now.
pub async fn mark_fetching(
    conn: &mut SqliteConnection,
    id: SourceId,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "UPDATE podcast_sources SET fetch_state = 'fetching', last_attempt_at = ?2, updated_at = ?2 WHERE id = ?1",
    )
    .bind(id.to_string())
    .bind(to_db_ts(now))
    .execute(conn)
    .await?;
    Ok(())
}

/// Replaces the current source of a podcast: `old` stops being current and
/// points at `new`, which is inserted as current. Must run in a transaction.
pub async fn replace_current(
    conn: &mut SqliteConnection,
    old: SourceId,
    new: &PodcastSource,
    reason: ReplacementReason,
    now: OffsetDateTime,
) -> Result<()> {
    let n = sqlx::query(
        "UPDATE podcast_sources SET is_current = 0, replaced_by_source_id = ?2, replacement_reason = ?3, \
         updated_at = ?4 WHERE id = ?1 AND is_current = 1",
    )
    .bind(old.to_string())
    .bind(new.id.to_string())
    .bind(reason.as_str())
    .bind(to_db_ts(now))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(sqlx::Error::RowNotFound.into());
    }
    let mut current = new.clone();
    current.is_current = true;
    insert(conn, &current).await
}

/// Loads one source.
pub async fn get(conn: &mut SqliteConnection, id: SourceId) -> Result<Option<PodcastSource>> {
    let row: Option<SourceRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM podcast_sources WHERE id = ?1"
    ))
    .bind(id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(SourceRow::into_model).transpose()
}

/// The current source of each podcast in `ids`, keyed by podcast.
///
/// One statement for a whole page, because the podcast list used to ask this
/// once per row. Rides `idx_sources_current`, the partial unique index.
pub async fn current_many(
    conn: &mut SqliteConnection,
    ids: &[PodcastId],
) -> Result<std::collections::HashMap<PodcastId, PodcastSource>> {
    many(conn, ids, "is_current = 1").await
}

/// The announced source of each podcast in `ids` that has one.
pub async fn announced_many(
    conn: &mut SqliteConnection,
    ids: &[PodcastId],
) -> Result<std::collections::HashMap<PodcastId, PodcastSource>> {
    many(conn, ids, ANNOUNCED).await
}

/// The row matching `which` of each podcast in `ids`; `which` selects at
/// most one row per podcast.
async fn many(
    conn: &mut SqliteConnection,
    ids: &[PodcastId],
    which: &str,
) -> Result<std::collections::HashMap<PodcastId, PodcastSource>> {
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
            "SELECT {COLUMNS} FROM podcast_sources \
             WHERE podcast_id IN ({placeholders}) AND {which}"
        );
        let mut q = sqlx::query_as::<_, SourceRow>(&sql);
        for id in chunk {
            q = q.bind(id.to_string());
        }
        for row in q.fetch_all(&mut *conn).await? {
            let source = SourceRow::into_model(row)?;
            out.insert(source.podcast_id, source);
        }
    }
    Ok(out)
}

/// The current source of a podcast.
pub async fn current(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Option<PodcastSource>> {
    let row: Option<SourceRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM podcast_sources WHERE podcast_id = ?1 AND is_current = 1"
    ))
    .bind(podcast_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(SourceRow::into_model).transpose()
}

/// Where a podcast's feed came from: the current source first, then the
/// replaced ones, newest first. The announced source is not among them.
pub async fn history(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Vec<PodcastSource>> {
    let rows: Vec<SourceRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM podcast_sources WHERE podcast_id = ?1 AND NOT ({ANNOUNCED}) \
         ORDER BY is_current DESC, id DESC"
    ))
    .bind(podcast_id.to_string())
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(SourceRow::into_model).collect()
}

/// The URL a podcast's feed announces and Uguisu has not verified.
pub async fn announced(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Option<PodcastSource>> {
    let row: Option<SourceRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM podcast_sources WHERE podcast_id = ?1 AND {ANNOUNCED}"
    ))
    .bind(podcast_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(SourceRow::into_model).transpose()
}

/// Removes a podcast's announced source; returns how many rows went.
pub async fn clear_announced(conn: &mut SqliteConnection, podcast_id: PodcastId) -> Result<u64> {
    let n = sqlx::query(&format!(
        "DELETE FROM podcast_sources WHERE podcast_id = ?1 AND {ANNOUNCED}"
    ))
    .bind(podcast_id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(n)
}

/// Current sources whose feed URL or canonical URL equals one of `urls`
/// (exact string comparison; callers pass normalized variants).
pub async fn find_current_by_urls(
    conn: &mut SqliteConnection,
    urls: &[String],
) -> Result<Vec<PodcastSource>> {
    let mut out = Vec::new();
    for url in urls {
        let rows: Vec<SourceRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM podcast_sources WHERE is_current = 1 AND (feed_url = ?1 OR canonical_url = ?1)"
        ))
        .bind(url)
        .fetch_all(&mut *conn)
        .await?;
        for r in rows {
            let s = r.into_model()?;
            if !out.iter().any(|o: &PodcastSource| o.id == s.id) {
                out.push(s);
            }
        }
    }
    Ok(out)
}

/// The feed and canonical URL of every source, current, replaced or
/// announced, with its podcast.
pub async fn known_urls(conn: &mut SqliteConnection) -> Result<Vec<(PodcastId, Url)>> {
    let rows: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, podcast_id, feed_url, canonical_url FROM podcast_sources")
            .fetch_all(conn)
            .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (rid, podcast_id, feed_url, canonical_url) in rows {
        let podcast_id = row::id(TABLE, &rid, &podcast_id)?;
        out.push((podcast_id, row::req_url(TABLE, &rid, &feed_url)?));
        if let Some(canonical) = opt_url(canonical_url.as_deref()) {
            out.push((podcast_id, canonical));
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn sample(podcast_id: PodcastId, url: &str) -> PodcastSource {
    let now = OffsetDateTime::now_utc();
    PodcastSource {
        id: SourceId::new(),
        podcast_id,
        feed_url: Url::parse(url).unwrap_or_else(|_| unreachable!()),
        canonical_url: None,
        website_url: None,
        provider: "manual".into(),
        provider_ref: None,
        discovered_at: now,
        verified_at: None,
        is_current: true,
        replaced_by_source_id: None,
        replacement_reason: None,
        fetch: FetchStatus::default(),
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Storage;

    #[tokio::test]
    async fn one_current_source_per_podcast_and_replacement() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let src = sample(p.id, "https://example.test/feed.xml");
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        insert(&mut tx, &src).await.unwrap();
        // a second current source for the same podcast violates the partial unique index
        let dup = sample(p.id, "https://example.test/other.xml");
        assert!(insert(&mut tx, &dup).await.is_err());
        drop(tx);

        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        insert(&mut tx, &src).await.unwrap();
        tx.commit().await.unwrap();

        let mut r = s.reader().await.unwrap();
        let cur = current(&mut r, p.id).await.unwrap().unwrap();
        assert_eq!(cur.id, src.id);
        assert_eq!(cur.fetch.state, FetchState::NeverFetched);
        let found = find_current_by_urls(&mut r, &["https://example.test/feed.xml".to_owned()])
            .await
            .unwrap();
        assert_eq!(found.len(), 1);

        let now = OffsetDateTime::now_utc();
        let mut fetch = FetchStatus {
            state: FetchState::Fetched,
            last_attempt_at: Some(now),
            last_success_at: Some(now),
            etag: Some("\"abc\"".into()),
            last_content_length: Some(1234),
            ..FetchStatus::default()
        };
        let mut tx = s.begin().await.unwrap();
        update_fetch(&mut tx, src.id, &fetch, Some(now), None, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let cur = get(&mut r, src.id).await.unwrap().unwrap();
        assert_eq!(cur.fetch.etag.as_deref(), Some("\"abc\""));
        assert_eq!(cur.fetch.last_content_length, Some(1234));
        assert!(cur.verified_at.is_some());

        fetch.state = FetchState::Failed;
        fetch.last_error_kind = Some(FetchErrorKind::Timeout);
        fetch.consecutive_failures = 1;
        let mut tx = s.begin().await.unwrap();
        update_fetch(&mut tx, src.id, &fetch, None, None, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let cur = get(&mut r, src.id).await.unwrap().unwrap();
        assert_eq!(cur.fetch.last_error_kind, Some(FetchErrorKind::Timeout));
        assert!(
            cur.verified_at.is_some(),
            "COALESCE keeps the old verification time"
        );

        let new = sample(p.id, "https://example.test/new.xml");
        let mut tx = s.begin().await.unwrap();
        replace_current(&mut tx, src.id, &new, ReplacementReason::NewFeedUrl, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let cur = current(&mut r, p.id).await.unwrap().unwrap();
        assert_eq!(cur.id, new.id);
        let old = get(&mut r, src.id).await.unwrap().unwrap();
        assert!(!old.is_current);
        assert_eq!(old.replaced_by_source_id, Some(new.id));
        assert_eq!(old.replacement_reason, Some(ReplacementReason::NewFeedUrl));
        let hist = history(&mut r, p.id).await.unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].id, new.id);
        // replacing a non-current source fails
        let mut tx = s.begin().await.unwrap();
        let another = sample(p.id, "https://example.test/x.xml");
        assert!(
            replace_current(&mut tx, src.id, &another, ReplacementReason::Redirect, now)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn announced_source_stays_out_of_history() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let src = sample(p.id, "https://old.example/feed.xml");
        let mut announced_row = sample(p.id, "https://new.example/feed.xml");
        announced_row.is_current = false;
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        insert(&mut tx, &src).await.unwrap();
        insert(&mut tx, &announced_row).await.unwrap();
        tx.commit().await.unwrap();

        let mut r = s.reader().await.unwrap();
        assert_eq!(current(&mut r, p.id).await.unwrap().unwrap().id, src.id);
        assert_eq!(
            announced(&mut r, p.id).await.unwrap().map(|a| a.id),
            Some(announced_row.id)
        );
        assert_eq!(
            announced_many(&mut r, &[p.id]).await.unwrap()[&p.id].id,
            announced_row.id
        );
        let hist: Vec<_> = history(&mut r, p.id)
            .await
            .unwrap()
            .iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(hist, [src.id]);

        // A replaced row is history, not an announcement.
        let now = OffsetDateTime::now_utc();
        let next = sample(p.id, "https://next.example/feed.xml");
        let mut tx = s.begin().await.unwrap();
        replace_current(&mut tx, src.id, &next, ReplacementReason::Manual, now)
            .await
            .unwrap();
        assert_eq!(clear_announced(&mut tx, p.id).await.unwrap(), 1);
        tx.commit().await.unwrap();
        assert!(announced(&mut r, p.id).await.unwrap().is_none());
        assert_eq!(history(&mut r, p.id).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn known_urls_include_replaced_sources() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let mut first = sample(p.id, "https://old.example/feed.xml");
        first.canonical_url = Some(Url::parse("https://old.example/canonical").unwrap());
        let second = sample(p.id, "https://new.example/feed.xml");
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        insert(&mut tx, &first).await.unwrap();
        let now = OffsetDateTime::now_utc();
        replace_current(&mut tx, first.id, &second, ReplacementReason::Redirect, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut r = s.reader().await.unwrap();
        let mut known: Vec<_> = known_urls(&mut r)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, url)| (id, url.to_string()))
            .collect();
        known.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            known,
            [
                (p.id, "https://new.example/feed.xml".to_owned()),
                (p.id, "https://old.example/canonical".to_owned()),
                (p.id, "https://old.example/feed.xml".to_owned()),
            ]
        );
    }
}
