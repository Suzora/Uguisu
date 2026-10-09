//! `episode_changes` repository: append-only field change log (ADR 0015).

use sqlx::{FromRow, SqliteConnection};
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_core::model::EpisodeChange;

use crate::row;
use crate::{Result, to_db_ts};

const TABLE: &str = "episode_changes";
/// Values longer than this are truncated before storage.
pub const MAX_VALUE_BYTES: usize = 1024;

#[derive(FromRow)]
struct ChangeRow {
    id: String,
    episode_id: String,
    podcast_id: String,
    fetch_id: Option<String>,
    changed_at: String,
    field: String,
    old_value: Option<String>,
    new_value: Option<String>,
}

impl ChangeRow {
    fn into_model(self) -> Result<EpisodeChange> {
        let rid = self.id.as_str();
        Ok(EpisodeChange {
            id: row::id(TABLE, rid, rid)?,
            episode_id: row::id(TABLE, rid, &self.episode_id)?,
            podcast_id: row::id(TABLE, rid, &self.podcast_id)?,
            fetch_id: row::opt_id(TABLE, rid, self.fetch_id.as_deref())?,
            changed_at: row::ts(TABLE, rid, &self.changed_at)?,
            field: self.field.clone(),
            old_value: self.old_value.clone(),
            new_value: self.new_value.clone(),
        })
    }
}

fn truncate(v: Option<&String>) -> Option<String> {
    v.map(|s| {
        if s.len() <= MAX_VALUE_BYTES {
            s.clone()
        } else {
            let mut end = MAX_VALUE_BYTES;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &s[..end])
        }
    })
}

/// Appends change records.
pub async fn insert_all(conn: &mut SqliteConnection, changes: &[EpisodeChange]) -> Result<()> {
    for c in changes {
        sqlx::query(
            "INSERT INTO episode_changes (id, episode_id, podcast_id, fetch_id, changed_at, field, old_value, new_value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(c.id.to_string())
        .bind(c.episode_id.to_string())
        .bind(c.podcast_id.to_string())
        .bind(c.fetch_id.map(|i| i.to_string()))
        .bind(to_db_ts(c.changed_at))
        .bind(&c.field)
        .bind(truncate(c.old_value.as_ref()))
        .bind(truncate(c.new_value.as_ref()))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Changes of one episode, oldest first.
pub async fn list_for_episode(
    conn: &mut SqliteConnection,
    episode_id: EpisodeId,
) -> Result<Vec<EpisodeChange>> {
    let rows: Vec<ChangeRow> = sqlx::query_as(
        "SELECT id, episode_id, podcast_id, fetch_id, changed_at, field, old_value, new_value \
         FROM episode_changes WHERE episode_id = ?1 ORDER BY changed_at, id",
    )
    .bind(episode_id.to_string())
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(ChangeRow::into_model).collect()
}

/// Newest changes of one podcast.
pub async fn list_for_podcast(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    limit: u32,
) -> Result<Vec<EpisodeChange>> {
    let rows: Vec<ChangeRow> = sqlx::query_as(
        "SELECT id, episode_id, podcast_id, fetch_id, changed_at, field, old_value, new_value \
         FROM episode_changes WHERE podcast_id = ?1 ORDER BY changed_at DESC, id DESC LIMIT ?2",
    )
    .bind(podcast_id.to_string())
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(ChangeRow::into_model).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn long_values_cut_on_char_boundary() {
        let long = "ä".repeat(MAX_VALUE_BYTES); // 2 bytes each
        let t = truncate(Some(&long)).unwrap();
        assert!(t.ends_with('…'));
        assert!(t.len() <= MAX_VALUE_BYTES + '…'.len_utf8());
        assert_eq!(
            truncate(Some(&"short".to_owned())).as_deref(),
            Some("short")
        );
        assert_eq!(truncate(None), None);
    }
}
