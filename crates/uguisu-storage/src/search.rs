//! The full-text index: its state, its rebuild and the queries over it
//! (ADR 0029).
//!
//! Keeping the index in step is the database's job — migration 0005's
//! triggers do it for every writer, including an import and a person with
//! the `sqlite3` CLI. What lives here is everything the triggers cannot
//! do: build the index for rows that existed before it did, say whether
//! it can be trusted, and read it.
//!
//! The rebuild runs in batches, one transaction each, because the writer
//! pool has one connection: a hundred thousand episodes in a single
//! transaction would hold it for the whole build.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_core::model::ArchiveState;
use uguisu_core::search::{
    EpisodeHit, IndexState, PodcastHit, SEARCH_EXCERPT_CHARS, SearchIndexStatus,
};

use crate::row;
use crate::{Result, to_db_ts};

const TABLE: &str = "search_index_state";
const ROW: &str = "1";

/// Rows indexed per transaction during a rebuild.
pub const BATCH: u32 = 1000;

/// Column weights for `bm25`: a term in the title means more than the
/// same term in an hour of shownotes.
const EPISODE_WEIGHTS: &str = "0.0, 0.0, 10.0, 4.0, 1.0";
const PODCAST_WEIGHTS: &str = "0.0, 10.0, 4.0, 1.0";

/// How wide a snippet is, in tokens.
const SNIPPET_TOKENS: i64 = 20;

/// Which column a snippet is taken from. `-1` leaves it to FTS5, which
/// picks the column the match is actually in — naming `description_text`
/// would hand back the first words of the shownotes for a title match,
/// and a result list would show text that has nothing to do with the
/// search.
const SNIPPET_COLUMN: i64 = -1;

/// Whether the index can be trusted, and what it holds now.
///
/// Counted from the index, not read from the row: the triggers keep the
/// index current between builds, so a count stored by the last build goes
/// stale with the next episode. The row's `podcasts` and `episodes` columns
/// are unused. Episodes are counted through the id map, which holds one row
/// per indexed episode and costs a fraction of a scan of the FTS table.
pub async fn state(conn: &mut SqliteConnection) -> Result<SearchIndexStatus> {
    let (state, built_at, podcasts, episodes, detail, updated_at): (
        String,
        Option<String>,
        i64,
        i64,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT state, built_at, (SELECT count(*) FROM podcasts_fts), \
                (SELECT count(*) FROM episode_search_ids), detail, updated_at \
         FROM search_index_state WHERE id = 1",
    )
    .fetch_one(conn)
    .await?;
    Ok(SearchIndexStatus {
        state: row::parse_enum(TABLE, ROW, "state", &state, IndexState::parse)?,
        built_at: row::opt_ts(TABLE, ROW, built_at.as_deref())?,
        podcasts: u64::try_from(podcasts).unwrap_or(0),
        episodes: u64::try_from(episodes).unwrap_or(0),
        detail,
        updated_at: row::ts(TABLE, ROW, &updated_at)?,
    })
}

/// Records what the index is doing. `built_at` is set only when it
/// finished, so a `ready` row always carries the moment it became true.
pub async fn set_state(
    conn: &mut SqliteConnection,
    state: IndexState,
    detail: Option<&str>,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "UPDATE search_index_state SET state = ?1, detail = ?2, \
         built_at = CASE WHEN ?1 = 'ready' THEN ?3 ELSE built_at END, \
         updated_at = ?3 WHERE id = 1",
    )
    .bind(state.as_str())
    .bind(detail)
    .bind(to_db_ts(now))
    .execute(conn)
    .await?;
    Ok(())
}

/// Empties the index.
///
/// The episode side is cleared through the id map, whose `AFTER DELETE`
/// trigger removes the matching FTS rows: one rule, one place, whether
/// the row goes because a podcast was deleted or because the index is
/// being rebuilt.
pub async fn clear(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("DELETE FROM episode_search_ids")
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM podcasts_fts")
        .execute(conn)
        .await?;
    Ok(())
}

/// Indexes up to [`BATCH`] podcasts after `after`, in id order.
///
/// Returns how many were indexed and the last id, which is the cursor for
/// the next batch: paging by id rather than by offset, so a row inserted
/// during the rebuild cannot make the walk skip one.
pub async fn index_podcasts(
    conn: &mut SqliteConnection,
    after: Option<PodcastId>,
    limit: u32,
) -> Result<(u64, Option<PodcastId>)> {
    let cursor = after.map(|id| id.to_string()).unwrap_or_default();
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM podcasts WHERE id > ?1 ORDER BY id LIMIT ?2")
            .bind(&cursor)
            .bind(i64::from(limit))
            .fetch_all(&mut *conn)
            .await?;
    let Some(last) = ids.last().cloned() else {
        return Ok((0, None));
    };
    let n = sqlx::query(
        "INSERT INTO podcasts_fts (podcast_id, title, author, description_text) \
         SELECT id, title, author, substr(description_text, 1, ?3) FROM podcasts \
          WHERE id > ?1 ORDER BY id LIMIT ?2",
    )
    .bind(&cursor)
    .bind(i64::from(limit))
    .bind(i64::try_from(SEARCH_EXCERPT_CHARS).unwrap_or(i64::MAX))
    .execute(conn)
    .await?
    .rows_affected();
    Ok((n, Some(row::id("podcasts", &last, &last)?)))
}

/// Indexes up to [`BATCH`] episodes after `after`, in id order.
pub async fn index_episodes(
    conn: &mut SqliteConnection,
    after: Option<EpisodeId>,
    limit: u32,
) -> Result<(u64, Option<EpisodeId>)> {
    let cursor = after.map(|id| id.to_string()).unwrap_or_default();
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM episodes WHERE id > ?1 ORDER BY id LIMIT ?2")
            .bind(&cursor)
            .bind(i64::from(limit))
            .fetch_all(&mut *conn)
            .await?;
    let Some(last) = ids.last().cloned() else {
        return Ok((0, None));
    };
    // The map first, so every episode in the batch has a stable rowid.
    // `OR IGNORE` is safe here, unlike inside a trigger body, because
    // this is the outer statement and its conflict policy is its own.
    sqlx::query(
        "INSERT OR IGNORE INTO episode_search_ids (episode_id) \
         SELECT id FROM episodes WHERE id > ?1 ORDER BY id LIMIT ?2",
    )
    .bind(&cursor)
    .bind(i64::from(limit))
    .execute(&mut *conn)
    .await?;
    let n = sqlx::query(
        "INSERT INTO episodes_fts (rowid, episode_id, podcast_id, title, subtitle, description_text) \
         SELECT m.seq, e.id, e.podcast_id, e.title, e.subtitle, substr(e.description_text, 1, ?3) \
           FROM episodes e JOIN episode_search_ids m ON m.episode_id = e.id \
          WHERE e.id > ?1 ORDER BY e.id LIMIT ?2",
    )
    .bind(&cursor)
    .bind(i64::from(limit))
    .bind(i64::try_from(SEARCH_EXCERPT_CHARS).unwrap_or(i64::MAX))
    .execute(conn)
    .await?
    .rows_affected();
    Ok((n, Some(row::id("episodes", &last, &last)?)))
}

/// Merges the index's b-trees, once a rebuild is done.
pub async fn optimize(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("INSERT INTO episodes_fts (episodes_fts) VALUES ('optimize')")
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO podcasts_fts (podcasts_fts) VALUES ('optimize')")
        .execute(conn)
        .await?;
    Ok(())
}

/// How many rows the index holds.
pub async fn counts(conn: &mut SqliteConnection) -> Result<(u64, u64)> {
    let podcasts: i64 = sqlx::query_scalar("SELECT count(*) FROM podcasts_fts")
        .fetch_one(&mut *conn)
        .await?;
    let episodes: i64 = sqlx::query_scalar("SELECT count(*) FROM episodes_fts")
        .fetch_one(conn)
        .await?;
    Ok((
        u64::try_from(podcasts).unwrap_or(0),
        u64::try_from(episodes).unwrap_or(0),
    ))
}

#[derive(FromRow)]
struct EpisodeHitRow {
    episode_id: String,
    podcast_id: String,
    podcast_title: String,
    title: String,
    snippet: String,
    published_at: Option<String>,
    duration_secs: Option<i64>,
    archive_state: String,
    relevance: f64,
}

/// Episodes matching `expression`, best first.
///
/// One query returns everything a result list renders, so there is no
/// second round trip per hit. `bm25()` is negative (more negative is
/// better), which is inverted here so that "higher is better" holds
/// everywhere above this line.
pub async fn search_episodes(
    conn: &mut SqliteConnection,
    expression: &str,
    limit: u32,
) -> Result<Vec<EpisodeHit>> {
    let sql = format!(
        "SELECT f.episode_id AS episode_id, e.podcast_id AS podcast_id, \
                p.title AS podcast_title, e.title AS title, \
                snippet(episodes_fts, {SNIPPET_COLUMN}, '[', ']', '…', {SNIPPET_TOKENS}) AS snippet, \
                e.published_at AS published_at, e.duration_secs AS duration_secs, \
                e.archive_state AS archive_state, \
                -bm25(episodes_fts, {EPISODE_WEIGHTS}) AS relevance \
           FROM episodes_fts f \
           JOIN episodes e ON e.id = f.episode_id \
           JOIN podcasts p ON p.id = e.podcast_id \
          WHERE episodes_fts MATCH ?1 \
          ORDER BY relevance DESC, e.sort_at DESC, e.id DESC LIMIT ?2"
    );
    let rows: Vec<EpisodeHitRow> = sqlx::query_as(&sql)
        .bind(expression)
        .bind(i64::from(limit))
        .fetch_all(conn)
        .await?;
    rows.into_iter()
        .map(|r| {
            let rid = r.episode_id.as_str();
            Ok(EpisodeHit {
                episode_id: row::id("episodes", rid, rid)?,
                podcast_id: row::id("episodes", rid, &r.podcast_id)?,
                podcast_title: r.podcast_title,
                title: r.title,
                snippet: r.snippet,
                published_at: row::opt_ts("episodes", rid, r.published_at.as_deref())?,
                duration_secs: r.duration_secs.and_then(|n| u32::try_from(n).ok()),
                archive_state: row::parse_enum(
                    "episodes",
                    rid,
                    "archive_state",
                    &r.archive_state,
                    ArchiveState::parse,
                )?,
                relevance: r.relevance,
            })
        })
        .collect()
}

#[derive(FromRow)]
struct PodcastHitRow {
    podcast_id: String,
    title: String,
    author: Option<String>,
    snippet: String,
    relevance: f64,
}

/// Podcasts matching `expression`, best first.
pub async fn search_podcasts(
    conn: &mut SqliteConnection,
    expression: &str,
    limit: u32,
) -> Result<Vec<PodcastHit>> {
    let sql = format!(
        "SELECT f.podcast_id AS podcast_id, p.title AS title, p.author AS author, \
                snippet(podcasts_fts, {SNIPPET_COLUMN}, '[', ']', '…', {SNIPPET_TOKENS}) AS snippet, \
                -bm25(podcasts_fts, {PODCAST_WEIGHTS}) AS relevance \
           FROM podcasts_fts f \
           JOIN podcasts p ON p.id = f.podcast_id \
          WHERE podcasts_fts MATCH ?1 \
          ORDER BY relevance DESC, p.sort_title ASC, p.id DESC LIMIT ?2"
    );
    let rows: Vec<PodcastHitRow> = sqlx::query_as(&sql)
        .bind(expression)
        .bind(i64::from(limit))
        .fetch_all(conn)
        .await?;
    rows.into_iter()
        .map(|r| {
            let rid = r.podcast_id.as_str();
            Ok(PodcastHit {
                podcast_id: row::id("podcasts", rid, rid)?,
                title: r.title,
                author: r.author,
                snippet: r.snippet,
                relevance: r.relevance,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Storage;
    use uguisu_core::search::parse_query;

    async fn seed(store: &Storage) -> (PodcastId, Vec<EpisodeId>) {
        let mut tx = store.begin().await.unwrap();
        let mut podcast = crate::podcasts::sample("The Rust Show");
        podcast.description_text = Some("A show about systems programming".to_owned());
        crate::podcasts::insert(&mut tx, &podcast).await.unwrap();
        let mut ids = Vec::new();
        let published = OffsetDateTime::now_utc();
        for (n, (title, description)) in [
            ("Ownership and borrowing", "Lifetimes explained slowly"),
            ("Async runtimes", "Tokio, smol and the executor zoo"),
            ("Unsafe code", "When to reach for raw pointers"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut episode =
                crate::episodes::sample(podcast.id, &format!("e{n}"), title, published);
            episode.description_text = Some(description.to_owned());
            crate::episodes::upsert_all(&mut tx, std::slice::from_ref(&episode))
                .await
                .unwrap();
            ids.push(episode.id);
        }
        tx.commit().await.unwrap();
        (podcast.id, ids)
    }

    fn expression(input: &str) -> String {
        parse_query(input, false).unwrap().match_expression()
    }

    #[tokio::test]
    async fn the_triggers_index_what_is_written() {
        let store = Storage::open_temp().await.unwrap();
        let (podcast, episodes) = seed(&store).await;
        let mut reader = store.reader().await.unwrap();
        // No rebuild ran: everything here was indexed by the triggers.
        assert_eq!(counts(&mut reader).await.unwrap(), (1, 3));

        let hits = search_episodes(&mut reader, &expression("borrowing"), 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].episode_id, episodes[0]);
        assert_eq!(hits[0].podcast_title, "The Rust Show");
        assert!(
            hits[0].snippet.contains("[borrowing]"),
            "the snippet must mark the match, wherever it is: {}",
            hits[0].snippet
        );
        let hits = search_episodes(&mut reader, &expression("pointers"), 10)
            .await
            .unwrap();
        assert!(
            hits[0].snippet.contains("[pointers]"),
            "and in the shownotes too: {}",
            hits[0].snippet
        );

        let hits = search_podcasts(&mut reader, &expression("systems"), 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].podcast_id, podcast);
    }

    #[tokio::test]
    async fn a_title_match_outranks_the_shownotes() {
        let store = Storage::open_temp().await.unwrap();
        let (podcast, _) = seed(&store).await;
        let mut tx = store.begin().await.unwrap();
        let mut buried = crate::episodes::sample(
            podcast,
            "buried",
            "Something else entirely",
            OffsetDateTime::now_utc(),
        );
        buried.description_text = Some("we mention unsafe once, in passing".to_owned());
        crate::episodes::upsert_all(&mut tx, std::slice::from_ref(&buried))
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut reader = store.reader().await.unwrap();
        let hits = search_episodes(&mut reader, &expression("unsafe"), 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].title, "Unsafe code",
            "the title weight must decide this"
        );
        assert!(hits[0].relevance > hits[1].relevance);
    }

    #[tokio::test]
    async fn a_rebuild_is_idempotent() {
        let store = Storage::open_temp().await.unwrap();
        seed(&store).await;
        let mut writer = store.writer().await.unwrap();
        let before = counts(&mut writer).await.unwrap();

        clear(&mut writer).await.unwrap();
        assert_eq!(counts(&mut writer).await.unwrap(), (0, 0));

        // One row per batch, so the cursor is exercised rather than
        // assumed.
        let mut cursor = None;
        let mut episodes = 0;
        loop {
            let (n, last) = index_episodes(&mut writer, cursor, 1).await.unwrap();
            episodes += n;
            if last.is_none() {
                break;
            }
            cursor = last;
        }
        let (podcasts, _) = index_podcasts(&mut writer, None, BATCH).await.unwrap();
        assert_eq!((podcasts, episodes), before);
        assert_eq!(counts(&mut writer).await.unwrap(), before);
        optimize(&mut writer).await.unwrap();

        let hits = search_episodes(&mut writer, &expression("tokio"), 10)
            .await
            .unwrap();
        assert_eq!(
            hits.len(),
            1,
            "the rebuilt index answers like the built one"
        );
    }

    #[tokio::test]
    async fn the_state_row_says_its_worth() {
        let store = Storage::open_temp().await.unwrap();
        let mut writer = store.writer().await.unwrap();
        let fresh = state(&mut writer).await.unwrap();
        assert_eq!(fresh.state, IndexState::Stale);
        assert!(fresh.built_at.is_none());

        let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        set_state(&mut writer, IndexState::Building, None, now)
            .await
            .unwrap();
        assert!(state(&mut writer).await.unwrap().built_at.is_none());

        set_state(&mut writer, IndexState::Ready, None, now)
            .await
            .unwrap();
        let ready = state(&mut writer).await.unwrap();
        assert_eq!(ready.state, IndexState::Ready);
        assert_eq!(ready.built_at, Some(now));

        let later = now + time::Duration::minutes(5);
        set_state(&mut writer, IndexState::Stale, Some("interrupted"), later)
            .await
            .unwrap();
        let stale = state(&mut writer).await.unwrap();
        assert_eq!(stale.detail.as_deref(), Some("interrupted"));
        assert_eq!(
            stale.built_at,
            Some(now),
            "when it was last true is not erased by it stopping being true"
        );
    }

    #[tokio::test]
    async fn counts_follow_the_library() {
        let store = Storage::open_temp().await.unwrap();
        let now = OffsetDateTime::now_utc();
        let mut writer = store.writer().await.unwrap();
        set_state(&mut writer, IndexState::Ready, None, now)
            .await
            .unwrap();
        drop(writer);
        // Built empty, then the library grew: only the triggers indexed it.
        let (podcast, _) = seed(&store).await;
        let mut writer = store.writer().await.unwrap();
        let status = state(&mut writer).await.unwrap();
        assert_eq!((status.podcasts, status.episodes), (1, 3));

        sqlx::query("DELETE FROM podcasts WHERE id = ?1")
            .bind(podcast.to_string())
            .execute(&mut *writer)
            .await
            .unwrap();
        let status = state(&mut writer).await.unwrap();
        assert_eq!((status.podcasts, status.episodes), (0, 0));
    }
}
