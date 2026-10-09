//! `podcasts` repository.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::ids::PodcastId;
use uguisu_core::model::{FeedKind, Podcast, PodcastStatus};

use crate::row::{self, bool_from, i64_from_u64, opt_url, to_json, u64_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "podcasts";

#[derive(FromRow)]
struct PodcastRow {
    id: String,
    title: String,
    sort_title: String,
    subtitle: Option<String>,
    author: Option<String>,
    publisher: Option<String>,
    owner_name: Option<String>,
    owner_email: Option<String>,
    description_html: Option<String>,
    description_text: Option<String>,
    website: Option<String>,
    artwork_url: Option<String>,
    language: Option<String>,
    categories: String,
    explicit: Option<i64>,
    copyright: Option<String>,
    podcast_guid: Option<String>,
    feed_kind: String,
    status: String,
    refresh_interval_secs: Option<i64>,
    next_refresh_at: Option<String>,
    last_refresh_at: Option<String>,
    last_error: Option<String>,
    directory_name: Option<String>,
    metadata_hash: String,
    created_at: String,
    updated_at: String,
}

impl PodcastRow {
    fn into_model(self) -> Result<Podcast> {
        let rid = self.id.as_str();
        Ok(Podcast {
            id: row::id(TABLE, rid, rid)?,
            title: self.title.clone(),
            sort_title: self.sort_title.clone(),
            subtitle: self.subtitle.clone(),
            author: self.author.clone(),
            publisher: self.publisher.clone(),
            owner_name: self.owner_name.clone(),
            owner_email: self.owner_email.clone(),
            description_html: self.description_html.clone(),
            description_text: self.description_text.clone(),
            website: opt_url(self.website.as_deref()),
            artwork_url: opt_url(self.artwork_url.as_deref()),
            language: self.language.clone(),
            categories: row::json(TABLE, "categories", rid, &self.categories)?,
            explicit: bool_from(self.explicit),
            copyright: self.copyright.clone(),
            podcast_guid: self.podcast_guid.clone(),
            feed_kind: row::parse_enum(TABLE, rid, "feed_kind", &self.feed_kind, FeedKind::parse)?,
            status: row::parse_enum(TABLE, rid, "status", &self.status, PodcastStatus::parse)?,
            refresh_interval_secs: u64_from(self.refresh_interval_secs),
            next_refresh_at: row::opt_ts(TABLE, rid, self.next_refresh_at.as_deref())?,
            last_refresh_at: row::opt_ts(TABLE, rid, self.last_refresh_at.as_deref())?,
            last_error: self.last_error.clone(),
            directory_name: self.directory_name.clone(),
            metadata_hash: self.metadata_hash.clone(),
            created_at: row::ts(TABLE, rid, &self.created_at)?,
            updated_at: row::ts(TABLE, rid, &self.updated_at)?,
        })
    }
}

const COLUMNS: &str = "id, title, sort_title, subtitle, author, publisher, owner_name, owner_email, \
    description_html, description_text, website, artwork_url, language, categories, explicit, \
    copyright, podcast_guid, feed_kind, status, refresh_interval_secs, next_refresh_at, \
    last_refresh_at, last_error, directory_name, metadata_hash, created_at, updated_at";

/// Inserts a podcast.
pub async fn insert(conn: &mut SqliteConnection, p: &Podcast) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO podcasts ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)"
    ))
    .bind(p.id.to_string())
    .bind(&p.title)
    .bind(&p.sort_title)
    .bind(&p.subtitle)
    .bind(&p.author)
    .bind(&p.publisher)
    .bind(&p.owner_name)
    .bind(&p.owner_email)
    .bind(&p.description_html)
    .bind(&p.description_text)
    .bind(p.website.as_ref().map(Url::as_str))
    .bind(p.artwork_url.as_ref().map(Url::as_str))
    .bind(&p.language)
    .bind(to_json(&p.categories))
    .bind(p.explicit.map(i64::from))
    .bind(&p.copyright)
    .bind(&p.podcast_guid)
    .bind(p.feed_kind.as_str())
    .bind(p.status.as_str())
    .bind(p.refresh_interval_secs.map(i64_from_u64))
    .bind(p.next_refresh_at.map(to_db_ts))
    .bind(p.last_refresh_at.map(to_db_ts))
    .bind(&p.last_error)
    .bind(&p.directory_name)
    .bind(&p.metadata_hash)
    .bind(to_db_ts(p.created_at))
    .bind(to_db_ts(p.updated_at))
    .execute(conn)
    .await?;
    Ok(())
}

use url::Url;

/// Updates every mutable column of a podcast.
pub async fn update(conn: &mut SqliteConnection, p: &Podcast) -> Result<()> {
    let n = sqlx::query(
        "UPDATE podcasts SET title = ?2, sort_title = ?3, subtitle = ?4, author = ?5, publisher = ?6, \
         owner_name = ?7, owner_email = ?8, description_html = ?9, description_text = ?10, website = ?11, \
         artwork_url = ?12, language = ?13, categories = ?14, explicit = ?15, copyright = ?16, \
         podcast_guid = ?17, feed_kind = ?18, status = ?19, refresh_interval_secs = ?20, \
         next_refresh_at = ?21, last_refresh_at = ?22, last_error = ?23, directory_name = ?24, \
         metadata_hash = ?25, updated_at = ?26 WHERE id = ?1",
    )
    .bind(p.id.to_string())
    .bind(&p.title)
    .bind(&p.sort_title)
    .bind(&p.subtitle)
    .bind(&p.author)
    .bind(&p.publisher)
    .bind(&p.owner_name)
    .bind(&p.owner_email)
    .bind(&p.description_html)
    .bind(&p.description_text)
    .bind(p.website.as_ref().map(Url::as_str))
    .bind(p.artwork_url.as_ref().map(Url::as_str))
    .bind(&p.language)
    .bind(to_json(&p.categories))
    .bind(p.explicit.map(i64::from))
    .bind(&p.copyright)
    .bind(&p.podcast_guid)
    .bind(p.feed_kind.as_str())
    .bind(p.status.as_str())
    .bind(p.refresh_interval_secs.map(i64_from_u64))
    .bind(p.next_refresh_at.map(to_db_ts))
    .bind(p.last_refresh_at.map(to_db_ts))
    .bind(&p.last_error)
    .bind(&p.directory_name)
    .bind(&p.metadata_hash)
    .bind(to_db_ts(p.updated_at))
    .execute(conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(sqlx::Error::RowNotFound.into());
    }
    Ok(())
}

/// Loads one podcast.
pub async fn get(conn: &mut SqliteConnection, id: PodcastId) -> Result<Option<Podcast>> {
    let row: Option<PodcastRow> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM podcasts WHERE id = ?1"))
            .bind(id.to_string())
            .fetch_optional(conn)
            .await?;
    row.map(PodcastRow::into_model).transpose()
}

/// Lists every podcast ordered by sort title.
pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<Podcast>> {
    let rows: Vec<PodcastRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM podcasts ORDER BY sort_title, id"
    ))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(PodcastRow::into_model).collect()
}

/// How a page of podcasts is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PodcastOrder {
    /// By sort title, then id — what a library list reads as.
    #[default]
    Title,
    /// Newest first. ULIDs are time-ordered, so this needs no column and no
    /// index of its own.
    Added,
    /// Most recently refreshed first, whatever that refresh's outcome; never
    /// refreshed last; ties by id.
    Refreshed,
    /// Most episodes first; ties by id.
    Episodes,
}

/// What `Refreshed` sorts on: `to_db_ts` is fixed-width UTC, so text order
/// is time order, and `''` puts a podcast never refreshed after every other.
const REFRESHED: &str = "coalesce(last_refresh_at, '')";

/// What `Episodes` sorts on: every episode row, as `episodes::counts_many`
/// counts `episodes_total`, kept by triggers (migration 0009).
const EPISODES: &str = "episode_count";

/// The statement for one page; its placeholders are bound in the order
/// [`page`] binds them.
///
/// Every predicate is spelled out or left out rather than written as
/// `(? IS NULL OR …)`: the SQLite planner cannot see through that pattern,
/// which is how two other list queries ended up not using their indexes.
fn page_sql(status: bool, title: bool, order: PodcastOrder, after: bool) -> String {
    let mut conds = Vec::new();
    if status {
        conds.push("status = ?".to_owned());
    }
    if title {
        // A scan, not an index: no index answers a substring, and a library
        // is hundreds of rows, not the forty thousand episodes FTS is for
        // (ADR 0029). `lower()` folds ASCII only; `sort_title` is already
        // folded in Rust, which covers the rest of Unicode.
        conds.push("(instr(lower(title), ?) > 0 OR instr(sort_title, ?) > 0)".to_owned());
    }
    if after {
        conds.push(match order {
            PodcastOrder::Title => "(sort_title > ? OR (sort_title = ? AND id > ?))".to_owned(),
            PodcastOrder::Added => "id < ?".to_owned(),
            PodcastOrder::Refreshed => format!("({REFRESHED} < ? OR ({REFRESHED} = ? AND id > ?))"),
            PodcastOrder::Episodes => {
                let cursor = "(SELECT episode_count FROM podcasts WHERE id = ?)";
                format!("({EPISODES} < {cursor} OR ({EPISODES} = {cursor} AND id > ?))")
            }
        });
    }
    let filter = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {} ", conds.join(" AND "))
    };
    // `Title` rides idx_podcasts_page (0006) and `Added` the ULID primary
    // key. `Refreshed` and `Episodes` have no index and sort the filtered
    // rows, which at library scale costs less than a migration would.
    let by = match order {
        PodcastOrder::Title => "sort_title, id".to_owned(),
        PodcastOrder::Added => "id DESC".to_owned(),
        PodcastOrder::Refreshed => format!("{REFRESHED} DESC, id"),
        PodcastOrder::Episodes => format!("{EPISODES} DESC, id"),
    };
    format!("SELECT {COLUMNS} FROM podcasts {filter}ORDER BY {by} LIMIT ?")
}

/// Whether `p` is a row [`page`] would return for `query`: the same test
/// as its SQL predicate.
#[must_use]
pub fn title_matches(p: &Podcast, query: &str) -> bool {
    let needle = query.to_lowercase();
    p.title.to_ascii_lowercase().contains(&needle) || p.sort_title.contains(&needle)
}

/// One page of podcasts, keyset-paged.
///
/// `title` keeps the podcasts whose title contains it, case-insensitively
/// (see [`title_matches`]). `after` is the last row of the previous
/// page, already read so its sort key is known; under `Episodes` its count
/// is read in the same statement as the page.
pub async fn page(
    conn: &mut SqliteConnection,
    status: Option<PodcastStatus>,
    title: Option<&str>,
    order: PodcastOrder,
    after: Option<&Podcast>,
    limit: u32,
) -> Result<Vec<Podcast>> {
    let sql = page_sql(status.is_some(), title.is_some(), order, after.is_some());
    let mut q = sqlx::query_as::<_, PodcastRow>(&sql);
    if let Some(status) = status {
        q = q.bind(status.as_str());
    }
    if let Some(query) = title {
        let needle = query.to_lowercase();
        q = q.bind(needle.clone()).bind(needle);
    }
    if let Some(cursor) = after {
        let id = cursor.id.to_string();
        q = match order {
            PodcastOrder::Title => q
                .bind(cursor.sort_title.clone())
                .bind(cursor.sort_title.clone())
                .bind(id),
            PodcastOrder::Added => q.bind(id),
            PodcastOrder::Refreshed => {
                let at = cursor.last_refresh_at.map(to_db_ts).unwrap_or_default();
                q.bind(at.clone()).bind(at).bind(id)
            }
            PodcastOrder::Episodes => q.bind(id.clone()).bind(id.clone()).bind(id),
        };
    }
    let rows: Vec<PodcastRow> = q.bind(i64::from(limit)).fetch_all(conn).await?;
    rows.into_iter().map(PodcastRow::into_model).collect()
}

/// Number of podcasts, optionally in one status.
pub async fn count_where(
    conn: &mut SqliteConnection,
    status: Option<PodcastStatus>,
) -> Result<u64> {
    let n: i64 = match status {
        None => {
            sqlx::query_scalar("SELECT count(*) FROM podcasts")
                .fetch_one(conn)
                .await?
        }
        Some(status) => {
            sqlx::query_scalar("SELECT count(*) FROM podcasts WHERE status = ?1")
                .bind(status.as_str())
                .fetch_one(conn)
                .await?
        }
    };
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Podcasts carrying a given `podcast:guid`.
pub async fn find_by_guid(conn: &mut SqliteConnection, guid: &str) -> Result<Vec<Podcast>> {
    let rows: Vec<PodcastRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM podcasts WHERE podcast_guid = ?1 ORDER BY id"
    ))
    .bind(guid)
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(PodcastRow::into_model).collect()
}

/// Number of podcasts.
pub async fn count(conn: &mut SqliteConnection) -> Result<u64> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM podcasts")
        .fetch_one(conn)
        .await?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Ids of podcasts in `active` or `error` status (refresh candidates).
pub async fn refreshable_ids(conn: &mut SqliteConnection) -> Result<Vec<PodcastId>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM podcasts WHERE status IN ('active', 'error') ORDER BY sort_title, id",
    )
    .fetch_all(conn)
    .await?;
    ids.iter().map(|s| row::id(TABLE, s, s)).collect()
}

/// The status predicate the scheduler's queries share.
///
/// Spelled with `OR` rather than `IN`, and repeated verbatim in every
/// query: SQLite decides whether a partial index is usable by matching
/// the query's `WHERE` against the index's, and `idx_podcasts_due`
/// (migration 0005) is written the same way.
const DUE_STATUS: &str = "(status = 'active' OR status = 'error')";

/// `id NOT IN (…)` over `count` placeholders starting at `first`, or
/// nothing at all when there is nobody to exclude.
fn exclusion(first: usize, count: usize) -> String {
    if count == 0 {
        return String::new();
    }
    let placeholders = (0..count)
        .map(|i| format!("?{}", first + i))
        .collect::<Vec<_>>()
        .join(", ");
    format!("AND id NOT IN ({placeholders})")
}

/// Podcasts that have never been fetched. Placeholders: the exclusion
/// list, then the limit.
fn never_refreshed_sql(excluded: usize) -> String {
    format!(
        "SELECT id FROM podcasts INDEXED BY idx_podcasts_due \
         WHERE {DUE_STATUS} AND next_refresh_at IS NULL {} \
         ORDER BY next_refresh_at ASC, id ASC LIMIT ?{}",
        exclusion(1, excluded),
        excluded + 1
    )
}

/// Podcasts whose planned time has come. Placeholders: `now`, the
/// exclusion list, then the limit.
fn overdue_sql(excluded: usize) -> String {
    format!(
        "SELECT id FROM podcasts INDEXED BY idx_podcasts_due \
         WHERE {DUE_STATUS} AND next_refresh_at <= ?1 {} \
         ORDER BY next_refresh_at ASC, id ASC LIMIT ?{}",
        exclusion(2, excluded),
        excluded + 2
    )
}

/// Ids of podcasts due for a refresh at `now`, most overdue first.
///
/// `excluded` drops podcasts the caller is already refreshing *inside the
/// query*, so a `LIMIT` worth of capacity comes back filled rather than
/// half spent on work already in flight — the same trick
/// [`crate::downloads::select_claimable`] plays with saturated hosts. It
/// is an optimisation, never a lock: two callers may still be handed the
/// same podcast, and the engine's coalescer is what makes that harmless.
///
/// **Two statements, not one.** "Never fetched or overdue" reads as one
/// `OR`, but SQLite cannot answer it from one ordered index seek: the
/// `OR` over the status column already splits the search, and adding a
/// second `OR` over the time column makes the planner fall back to
/// `idx_podcasts_status_next` plus a sort of every active podcast — on
/// every wake, for the life of the process. Asked separately, each half
/// is a seek into `idx_podcasts_due` that returns rows already in order.
/// The halves are disjoint by construction (`NULL` or not), so
/// concatenating them needs no de-duplication, and "never fetched" comes
/// first because that is what maximally overdue means.
pub async fn due_ids(
    conn: &mut SqliteConnection,
    now: OffsetDateTime,
    excluded: &[PodcastId],
    limit: u32,
) -> Result<Vec<PodcastId>> {
    let limit = limit as usize;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let sql = never_refreshed_sql(excluded.len());
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for id in excluded {
        q = q.bind(id.to_string());
    }
    let mut rows: Vec<String> = q
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&mut *conn)
        .await?;
    if rows.len() < limit {
        let sql = overdue_sql(excluded.len());
        let mut q = sqlx::query_scalar::<_, String>(&sql).bind(to_db_ts(now));
        for id in excluded {
            q = q.bind(id.to_string());
        }
        let rest = i64::try_from(limit - rows.len()).unwrap_or(i64::MAX);
        rows.extend(q.bind(rest).fetch_all(conn).await?);
    }
    rows.iter().map(|s| row::id(TABLE, s, s)).collect()
}

/// The earliest planned refresh of any refreshable podcast, if one is
/// planned.
///
/// No index hint here, deliberately: `idx_podcasts_status_next` is
/// `(status, next_refresh_at)`, so SQLite answers this with one covering
/// seek per status — which is better than anything the partial index can
/// do for a `min()`. A podcast that has never been fetched has no planned
/// time to report and is due now, so it shows up in [`due_ids`] while
/// this stays `None`.
pub async fn next_due_at(conn: &mut SqliteConnection) -> Result<Option<OffsetDateTime>> {
    let at: Option<String> = sqlx::query_scalar(&format!(
        "SELECT min(next_refresh_at) FROM podcasts WHERE {DUE_STATUS}"
    ))
    .fetch_one(conn)
    .await?;
    row::opt_ts(TABLE, "min", at.as_deref())
}

/// Sets the status of one podcast. `false` when no such row exists.
///
/// One column rather than [`update`]'s twenty-six: pausing a podcast must
/// not write back a copy of everything else that a refresh running at the
/// same moment is in the middle of changing.
pub async fn set_status(
    conn: &mut SqliteConnection,
    id: PodcastId,
    status: PodcastStatus,
    now: OffsetDateTime,
) -> Result<bool> {
    let n = sqlx::query("UPDATE podcasts SET status = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id.to_string())
        .bind(status.as_str())
        .bind(to_db_ts(now))
        .execute(conn)
        .await?
        .rows_affected();
    Ok(n == 1)
}

/// Deletes a podcast and, through the schema's cascades, every row that
/// belongs to it (ADR 0055). Touches no file. `false` when no such row
/// exists.
pub async fn delete(conn: &mut SqliteConnection, id: PodcastId) -> Result<bool> {
    let n = sqlx::query("DELETE FROM podcasts WHERE id = ?1")
        .bind(id.to_string())
        .execute(conn)
        .await?
        .rows_affected();
    Ok(n == 1)
}

/// Sets when one podcast is next due. `None` means "as soon as possible",
/// which is what resuming a paused podcast and a manual reschedule both
/// want. `false` when no such row exists.
pub async fn set_next_refresh_at(
    conn: &mut SqliteConnection,
    id: PodcastId,
    at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Result<bool> {
    let n = sqlx::query("UPDATE podcasts SET next_refresh_at = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id.to_string())
        .bind(at.map(to_db_ts))
        .bind(to_db_ts(now))
        .execute(conn)
        .await?
        .rows_affected();
    Ok(n == 1)
}

/// A complete podcast row, for tests of this and dependent crates.
#[cfg(any(test, feature = "testing"))]
#[must_use]
pub fn sample(title: &str) -> Podcast {
    let now = time::OffsetDateTime::now_utc();
    Podcast {
        id: PodcastId::new(),
        title: title.to_owned(),
        sort_title: uguisu_core::model::sort_title(title),
        subtitle: None,
        author: Some("Host".into()),
        publisher: None,
        owner_name: None,
        owner_email: None,
        description_html: None,
        description_text: None,
        website: Url::parse("https://example.test/").ok(),
        artwork_url: None,
        language: Some("en".into()),
        categories: vec!["Technology".into()],
        explicit: Some(false),
        copyright: None,
        podcast_guid: None,
        feed_kind: FeedKind::Rss2,
        status: PodcastStatus::Active,
        refresh_interval_secs: None,
        next_refresh_at: None,
        last_refresh_at: None,
        last_error: None,
        directory_name: None,
        metadata_hash: "h".into(),
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
    async fn insert_get_update_list() {
        let s = Storage::open_temp().await.unwrap();
        let mut p = sample("The Daily");
        let mut tx = s.begin().await.unwrap();
        insert(&mut tx, &p).await.unwrap();
        tx.commit().await.unwrap();
        let mut r = s.reader().await.unwrap();
        let back = get(&mut r, p.id).await.unwrap().unwrap();
        assert_eq!(back.title, "The Daily");
        assert_eq!(back.sort_title, "daily");
        assert_eq!(back.categories, vec!["Technology"]);
        assert_eq!(
            back.website.as_ref().map(Url::as_str),
            Some("https://example.test/")
        );
        assert_eq!(to_db_ts(back.created_at), to_db_ts(p.created_at));
        p.title = "Daily".into();
        p.status = PodcastStatus::Error;
        p.podcast_guid = Some("g".into());
        let mut tx = s.begin().await.unwrap();
        update(&mut tx, &p).await.unwrap();
        tx.commit().await.unwrap();
        let back = get(&mut r, p.id).await.unwrap().unwrap();
        assert_eq!(back.title, "Daily");
        assert_eq!(back.status, PodcastStatus::Error);
        assert_eq!(find_by_guid(&mut r, "g").await.unwrap().len(), 1);
        assert_eq!(list(&mut r).await.unwrap().len(), 1);
        assert_eq!(count(&mut r).await.unwrap(), 1);
        assert_eq!(refreshable_ids(&mut r).await.unwrap(), vec![p.id]);
        assert!(get(&mut r, PodcastId::new()).await.unwrap().is_none());
        let mut w = s.writer().await.unwrap();
        let missing = sample("x");
        assert!(update(&mut w, &missing).await.is_err());
    }

    /// Seeds podcasts with the given statuses and due times.
    async fn seed(s: &Storage, rows: &[(PodcastStatus, Option<OffsetDateTime>)]) -> Vec<PodcastId> {
        let mut tx = s.begin().await.unwrap();
        let mut ids = Vec::new();
        for (n, (status, due)) in rows.iter().enumerate() {
            let mut p = sample(&format!("Show {n}"));
            p.status = *status;
            p.next_refresh_at = *due;
            insert(&mut tx, &p).await.unwrap();
            ids.push(p.id);
        }
        tx.commit().await.unwrap();
        ids
    }

    #[tokio::test]
    async fn the_due_query_orders_by_urgency() {
        let s = Storage::open_temp().await.unwrap();
        let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let ago = |m: i64| Some(now - time::Duration::minutes(m));
        let ids = seed(
            &s,
            &[
                (PodcastStatus::Active, ago(5)),
                (PodcastStatus::Active, None),
                (PodcastStatus::Error, ago(60)),
                (PodcastStatus::Active, Some(now + time::Duration::hours(1))),
                (PodcastStatus::Paused, ago(600)),
                (PodcastStatus::Archived, ago(600)),
            ],
        )
        .await;
        let mut r = s.reader().await.unwrap();
        let due = due_ids(&mut r, now, &[], 10).await.unwrap();
        assert_eq!(
            due,
            vec![ids[1], ids[2], ids[0]],
            "never fetched first, then the most overdue; not yet due, paused \
             and archived do not appear"
        );
        assert_eq!(
            due_ids(&mut r, now, &[], 2).await.unwrap(),
            vec![ids[1], ids[2]],
            "a limit is spent across both halves, never-fetched first"
        );

        // The exclusion happens in SQL, so a caller that is already busy
        // with one podcast still gets a full complement of others.
        let two = due_ids(&mut r, now, &[ids[1]], 2).await.unwrap();
        assert_eq!(two, vec![ids[2], ids[0]]);
        assert!(
            due_ids(&mut r, now, &[ids[0], ids[1], ids[2]], 10)
                .await
                .unwrap()
                .is_empty()
        );

        // `next_due_at` reports a plan, and a podcast that has never been
        // fetched has none to report — it is simply due now.
        assert_eq!(next_due_at(&mut r).await.unwrap(), ago(60));
        let s2 = Storage::open_temp().await.unwrap();
        seed(&s2, &[(PodcastStatus::Active, None)]).await;
        let mut r2 = s2.reader().await.unwrap();
        assert!(next_due_at(&mut r2).await.unwrap().is_none());
        assert_eq!(due_ids(&mut r2, now, &[], 10).await.unwrap().len(), 1);
    }

    /// A page of the library must come out of an index already ordered. A
    /// sort here is a sort of the whole table on every list request.
    #[tokio::test]
    async fn a_page_of_podcasts_rides_an_index() {
        let s = Storage::open_temp().await.unwrap();
        let mut r = s.reader().await.unwrap();
        for (name, order, after) in [
            ("first page", PodcastOrder::Title, false),
            ("later page", PodcastOrder::Title, true),
            ("newest first", PodcastOrder::Added, true),
        ] {
            let explained = format!(
                "EXPLAIN QUERY PLAN {}",
                page_sql(false, false, order, after)
            );
            let mut q = sqlx::query_as::<_, (i64, i64, i64, String)>(&explained);
            for _ in 0..explained.matches('?').count() {
                q = q.bind("x");
            }
            let plan = q
                .fetch_all(&mut *r)
                .await
                .unwrap()
                .into_iter()
                .map(|(_, _, _, d)| d)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                !plan.contains("TEMP B-TREE"),
                "{name}: ordering was a sort rather than an index walk: {plan}"
            );
        }
    }

    /// Every row of a filtered, ordered list, read `step` rows at a time.
    async fn walk(
        s: &Storage,
        status: Option<PodcastStatus>,
        title: Option<&str>,
        order: PodcastOrder,
        step: u32,
    ) -> Vec<PodcastId> {
        let mut r = s.reader().await.unwrap();
        let mut seen = Vec::new();
        let mut after: Option<Podcast> = None;
        loop {
            let rows = page(&mut r, status, title, order, after.as_ref(), step)
                .await
                .unwrap();
            seen.extend(rows.iter().map(|p| p.id));
            if rows.len() < step as usize {
                return seen;
            }
            after = rows.last().cloned();
        }
    }

    /// Ids in their tie-break order.
    fn by_id(mut ids: Vec<PodcastId>) -> Vec<PodcastId> {
        ids.sort_by_key(ToString::to_string);
        ids
    }

    /// Six podcasts with ties in every sort key, one of them paused:
    /// alpha, beta, the beta, delta, épsilon, zeta.
    async fn library() -> (Storage, Vec<PodcastId>) {
        let s = Storage::open_temp().await.unwrap();
        let at = |h: i64| Some(OffsetDateTime::UNIX_EPOCH + time::Duration::hours(h));
        let rows = [
            ("Alpha", at(2), 3, PodcastStatus::Active),
            ("Beta", at(1), 1, PodcastStatus::Active),
            ("The Beta", None, 3, PodcastStatus::Active),
            ("Delta", at(2), 0, PodcastStatus::Active),
            ("Épsilon", None, 1, PodcastStatus::Paused),
            ("Zeta", at(3), 0, PodcastStatus::Active),
        ];
        let mut tx = s.begin().await.unwrap();
        let mut ids = Vec::new();
        for (title, refreshed, episodes, status) in rows {
            let mut p = sample(title);
            p.last_refresh_at = refreshed;
            p.status = status;
            insert(&mut tx, &p).await.unwrap();
            let published = OffsetDateTime::UNIX_EPOCH;
            let eps: Vec<_> = (0..episodes)
                .map(|n| crate::episodes::sample(p.id, &format!("{title}{n}"), "e", published))
                .collect();
            crate::episodes::upsert_all(&mut tx, &eps).await.unwrap();
            ids.push(p.id);
        }
        tx.commit().await.unwrap();
        (s, ids)
    }

    /// The count the library sorts by follows every insert and delete, and
    /// does not stand in the way of removing the podcast.
    #[tokio::test]
    async fn episode_count_follows_episodes() {
        let (s, ids) = library().await;
        let alpha = ids[0];
        let mut tx = s.begin().await.unwrap();
        let count = async |tx: &mut sqlx::SqliteConnection| -> i64 {
            sqlx::query_scalar("SELECT episode_count FROM podcasts WHERE id = ?1")
                .bind(alpha.to_string())
                .fetch_one(tx)
                .await
                .unwrap()
        };
        assert_eq!(count(&mut tx).await, 3);

        // A refresh that finds a known episode updates it; nothing is added.
        let mut again =
            crate::episodes::sample(alpha, "Alpha0", "renamed", OffsetDateTime::UNIX_EPOCH);
        let known: String =
            sqlx::query_scalar("SELECT id FROM episodes WHERE podcast_id = ?1 AND guid = 'Alpha0'")
                .bind(alpha.to_string())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        again.id = known.parse().unwrap();
        crate::episodes::upsert_all(&mut tx, &[again])
            .await
            .unwrap();
        assert_eq!(count(&mut tx).await, 3);

        sqlx::query("DELETE FROM episodes WHERE id = (SELECT id FROM episodes WHERE podcast_id = ?1 LIMIT 1)")
            .bind(alpha.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count(&mut tx).await, 2);

        let gone = sqlx::query("DELETE FROM podcasts WHERE id = ?1")
            .bind(alpha.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(gone.rows_affected(), 1);
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn every_order_pages_without_gaps() {
        let (s, ids) = library().await;
        let [alpha, beta, the_beta, delta, epsilon, zeta] = ids[..] else {
            unreachable!()
        };
        let cases = [
            (
                PodcastOrder::Title,
                [
                    vec![alpha],
                    by_id(vec![beta, the_beta]),
                    vec![delta, zeta, epsilon],
                ]
                .concat(),
            ),
            (
                PodcastOrder::Added,
                by_id(ids.clone()).into_iter().rev().collect(),
            ),
            (
                PodcastOrder::Refreshed,
                [
                    vec![zeta],
                    by_id(vec![alpha, delta]),
                    vec![beta],
                    by_id(vec![the_beta, epsilon]),
                ]
                .concat(),
            ),
            (
                PodcastOrder::Episodes,
                [
                    by_id(vec![alpha, the_beta]),
                    by_id(vec![beta, epsilon]),
                    by_id(vec![delta, zeta]),
                ]
                .concat(),
            ),
        ];
        for (order, expected) in cases {
            for step in [1, 2, 50] {
                assert_eq!(
                    walk(&s, None, None, order, step).await,
                    expected,
                    "{order:?}, {step} at a time"
                );
            }
        }
    }

    #[tokio::test]
    async fn the_title_filter_pages_too() {
        let (s, ids) = library().await;
        let [alpha, beta, the_beta, delta, epsilon, zeta] = ids[..] else {
            unreachable!()
        };
        let cases = [
            (
                None,
                "eta",
                PodcastOrder::Refreshed,
                vec![zeta, beta, the_beta],
            ),
            (
                None,
                "ETA",
                PodcastOrder::Episodes,
                vec![the_beta, beta, zeta],
            ),
            (
                None,
                "eta",
                PodcastOrder::Title,
                [by_id(vec![beta, the_beta]), vec![zeta]].concat(),
            ),
            (None, "ÉPS", PodcastOrder::Title, vec![epsilon]),
            (None, "the b", PodcastOrder::Added, vec![the_beta]),
            (None, "%", PodcastOrder::Title, vec![]),
            (
                Some(PodcastStatus::Active),
                "a",
                PodcastOrder::Episodes,
                [
                    by_id(vec![alpha, the_beta]),
                    vec![beta],
                    by_id(vec![delta, zeta]),
                ]
                .concat(),
            ),
        ];
        for (status, title, order, expected) in cases {
            for step in [1, 2, 50] {
                assert_eq!(
                    walk(&s, status, Some(title), order, step).await,
                    expected,
                    "{order:?}, {title:?}, {status:?}, {step} at a time"
                );
            }
        }
        let mut r = s.reader().await.unwrap();
        let epsilon_row = get(&mut r, epsilon).await.unwrap().unwrap();
        for (query, matches) in [("ÉPS", true), ("silon", true), ("eta", false)] {
            assert_eq!(title_matches(&epsilon_row, query), matches, "{query}");
        }
    }

    #[tokio::test]
    async fn the_due_queries_seek_the_index() {
        // The scheduler runs these on every wake, for the life of the
        // process. A plan that scans `podcasts` or sorts afterwards would
        // make an idle daemon's cost grow with the library — which is
        // what happens the moment the two halves are merged into one `OR`
        // (measured: `idx_podcasts_status_next` plus a temp B-tree over
        // every active podcast), and what `INDEXED BY` now prevents.
        let s = Storage::open_temp().await.unwrap();
        seed(&s, &[(PodcastStatus::Active, None)]).await;
        let mut r = s.reader().await.unwrap();
        for excluded in [0_usize, 3] {
            for (name, sql, now_bind) in [
                ("never refreshed", never_refreshed_sql(excluded), false),
                ("overdue", overdue_sql(excluded), true),
            ] {
                let explained = format!("EXPLAIN QUERY PLAN {sql}");
                // The plan's four columns are (id, parent, notused,
                // detail); `detail` is the readable one.
                let mut q = sqlx::query_as::<_, (i64, i64, i64, String)>(&explained);
                if now_bind {
                    q = q.bind(to_db_ts(OffsetDateTime::now_utc()));
                }
                for _ in 0..excluded {
                    q = q.bind(PodcastId::new().to_string());
                }
                let plan = q
                    .bind(8_i64)
                    .fetch_all(&mut *r)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|(_, _, _, detail)| detail)
                    .collect::<Vec<_>>()
                    .join("; ");
                assert!(
                    plan.contains("SEARCH") && plan.contains("idx_podcasts_due"),
                    "{name} with {excluded} exclusions does not seek the index: {plan}"
                );
                assert!(
                    !plan.contains("SCAN podcasts"),
                    "{name}: the whole table is being read: {plan}"
                );
                assert!(
                    !plan.contains("TEMP B-TREE"),
                    "{name}: the order must come out of the index, not a sort: {plan}"
                );
            }
        }
        // `next_due_at` is answered from a covering index, without one.
        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!(
            "EXPLAIN QUERY PLAN SELECT min(next_refresh_at) FROM podcasts WHERE {DUE_STATUS}"
        ))
        .fetch_all(&mut *r)
        .await
        .unwrap();
        let plan = plan
            .into_iter()
            .map(|(_, _, _, d)| d)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(plan.contains("COVERING INDEX"), "{plan}");
        assert!(!plan.contains("SCAN podcasts"), "{plan}");
    }

    #[tokio::test]
    async fn the_narrow_writers_touch_one_column() {
        let s = Storage::open_temp().await.unwrap();
        let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let ids = seed(&s, &[(PodcastStatus::Active, Some(now))]).await;
        let mut w = s.writer().await.unwrap();
        let before = get(&mut w, ids[0]).await.unwrap().unwrap();

        let later = now + time::Duration::hours(1);
        assert!(
            set_status(&mut w, ids[0], PodcastStatus::Paused, later)
                .await
                .unwrap()
        );
        assert!(
            set_next_refresh_at(&mut w, ids[0], None, later)
                .await
                .unwrap()
        );
        let after = get(&mut w, ids[0]).await.unwrap().unwrap();
        assert_eq!(after.status, PodcastStatus::Paused);
        assert!(after.next_refresh_at.is_none());
        assert_eq!(after.updated_at, later);
        // Everything a refresh owns is untouched: these writers exist so a
        // pause cannot overwrite a refresh that is running right now.
        assert_eq!(
            (after.title, after.metadata_hash, after.last_refresh_at),
            (before.title, before.metadata_hash, before.last_refresh_at)
        );

        assert!(
            !set_status(&mut w, PodcastId::new(), PodcastStatus::Active, later)
                .await
                .unwrap()
        );
        assert!(
            !set_next_refresh_at(&mut w, PodcastId::new(), Some(later), later)
                .await
                .unwrap()
        );
    }
}
