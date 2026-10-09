//! `events` repository: the persisted event log (ADR 0010).

use sqlx::{FromRow, SqliteConnection};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::EventId;

use crate::row::{self, to_json};
use crate::{Result, to_db_ts};

const TABLE: &str = "events";

#[derive(FromRow)]
struct EventRow {
    id: String,
    occurred_at: String,
    podcast_id: Option<String>,
    episode_id: Option<String>,
    payload: String,
}

impl EventRow {
    fn into_model(self) -> Result<Event> {
        let rid = self.id.as_str();
        let kind: EventKind = row::json(TABLE, "payload", rid, &self.payload)?;
        Ok(Event {
            schema: Event::SCHEMA,
            id: row::id(TABLE, rid, rid)?,
            occurred_at: row::ts(TABLE, rid, &self.occurred_at)?,
            podcast_id: row::opt_id(TABLE, rid, self.podcast_id.as_deref())?,
            episode_id: row::opt_id(TABLE, rid, self.episode_id.as_deref())?,
            kind,
        })
    }
}

/// Appends events.
pub async fn insert_all(conn: &mut SqliteConnection, events: &[Event]) -> Result<()> {
    for e in events {
        sqlx::query(
            "INSERT INTO events (id, occurred_at, kind, podcast_id, episode_id, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(e.id.to_string())
        .bind(to_db_ts(e.occurred_at))
        .bind(e.name())
        .bind(e.podcast_id.map(|i| i.to_string()))
        .bind(e.episode_id.map(|i| i.to_string()))
        .bind(to_json(&e.kind))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Events newer than `after` (exclusive), oldest first.
pub async fn list_after(
    conn: &mut SqliteConnection,
    after: Option<EventId>,
    limit: u32,
) -> Result<Vec<Event>> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, occurred_at, podcast_id, episode_id, payload FROM events \
         WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2",
    )
    .bind(after.map(|i| i.to_string()))
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(EventRow::into_model).collect()
}

/// The newest `limit` events, oldest first.
pub async fn list_latest(conn: &mut SqliteConnection, limit: u32) -> Result<Vec<Event>> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, occurred_at, podcast_id, episode_id, payload FROM \
         (SELECT * FROM events ORDER BY id DESC LIMIT ?1) ORDER BY id",
    )
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(EventRow::into_model).collect()
}

/// Events of one podcast, newest first.
pub async fn list_for_podcast(
    conn: &mut SqliteConnection,
    podcast_id: uguisu_core::ids::PodcastId,
    limit: u32,
) -> Result<Vec<Event>> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, occurred_at, podcast_id, episode_id, payload FROM events \
         WHERE podcast_id = ?1 ORDER BY id DESC LIMIT ?2",
    )
    .bind(podcast_id.to_string())
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(EventRow::into_model).collect()
}

/// Deletes events older than `before`, keeping at most `max_rows` newest ones.
pub async fn prune(
    conn: &mut SqliteConnection,
    before: time::OffsetDateTime,
    max_rows: u64,
) -> Result<u64> {
    let a = sqlx::query("DELETE FROM events WHERE occurred_at < ?1")
        .bind(to_db_ts(before))
        .execute(&mut *conn)
        .await?
        .rows_affected();
    let b = sqlx::query(
        "DELETE FROM events WHERE id NOT IN (SELECT id FROM events ORDER BY id DESC LIMIT ?1)",
    )
    .bind(i64::try_from(max_rows).unwrap_or(i64::MAX))
    .execute(conn)
    .await?
    .rows_affected();
    Ok(a + b)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Storage;
    use uguisu_core::ids::PodcastId;

    #[tokio::test]
    async fn append_and_page_events() {
        let s = Storage::open_temp().await.unwrap();
        let p = PodcastId::new();
        let events: Vec<Event> = (0..3)
            .map(|i| {
                Event::now(
                    Some(p),
                    None,
                    EventKind::PodcastMetadataUpdated {
                        fields: vec![format!("f{i}")],
                    },
                )
            })
            .collect();
        let mut tx = s.begin().await.unwrap();
        insert_all(&mut tx, &events).await.unwrap();
        tx.commit().await.unwrap();
        let mut r = s.reader().await.unwrap();
        let first = list_after(&mut r, None, 2).await.unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].id, events[0].id);
        assert_eq!(first[0].kind, events[0].kind);
        assert_eq!(
            first[0].occurred_at.unix_timestamp(),
            events[0].occurred_at.unix_timestamp()
        );
        let rest = list_after(&mut r, Some(first[1].id), 10).await.unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].name(), "podcast.metadata.updated");
        let mine = list_for_podcast(&mut r, p, 10).await.unwrap();
        assert_eq!(mine.len(), 3);
        assert_eq!(mine[0].id, events[2].id);
        let mut w = s.writer().await.unwrap();
        let removed = prune(
            &mut w,
            time::OffsetDateTime::now_utc() - time::Duration::days(1),
            1,
        )
        .await
        .unwrap();
        assert_eq!(removed, 2);
    }
}
