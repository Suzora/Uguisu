//! `archive_policies` repository (ADR 0023).
//!
//! A row exists only where a user changed something for one podcast;
//! every `NULL` column falls back to the configured global default, and a
//! missing row means "use the defaults entirely". The engine merges the
//! two into the effective policy, so removing a row is a clean reset.

use sqlx::{FromRow, SqliteConnection};
use uguisu_core::archive::{ArchivePolicy, PolicyMode};
use uguisu_core::download::Priority;
use uguisu_core::ids::PodcastId;

use crate::row::{self, u32_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "archive_policies";

const COLUMNS: &str = "podcast_id, mode, max_backlog, max_age_days, priority, updated_at";

#[derive(FromRow)]
struct PolicyRow {
    podcast_id: String,
    mode: String,
    max_backlog: Option<i64>,
    max_age_days: Option<i64>,
    priority: Option<i64>,
    updated_at: String,
}

impl PolicyRow {
    fn into_model(self) -> Result<ArchivePolicy> {
        let rid = self.podcast_id.clone();
        Ok(ArchivePolicy {
            podcast_id: row::id(TABLE, &rid, &self.podcast_id)?,
            mode: row::parse_enum(TABLE, &rid, "mode", &self.mode, PolicyMode::parse)?,
            max_backlog: u32_from(self.max_backlog),
            max_age_days: u32_from(self.max_age_days),
            priority: self.priority.and_then(Priority::from_i64),
            updated_at: row::ts(TABLE, &rid, &self.updated_at)?,
        })
    }
}

/// Stores a podcast's policy, replacing any previous one.
pub async fn set(conn: &mut SqliteConnection, p: &ArchivePolicy) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO {TABLE} ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (podcast_id) DO UPDATE SET \
           mode = excluded.mode, max_backlog = excluded.max_backlog, \
           max_age_days = excluded.max_age_days, priority = excluded.priority, \
           updated_at = excluded.updated_at"
    ))
    .bind(p.podcast_id.to_string())
    .bind(p.mode.as_str())
    .bind(p.max_backlog.map(i64::from))
    .bind(p.max_age_days.map(i64::from))
    .bind(p.priority.map(Priority::as_i64))
    .bind(to_db_ts(p.updated_at))
    .execute(conn)
    .await?;
    Ok(())
}

/// The policy of one podcast, if it has its own.
pub async fn get(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Option<ArchivePolicy>> {
    let row: Option<PolicyRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} WHERE podcast_id = ?1"
    ))
    .bind(podcast_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(PolicyRow::into_model).transpose()
}

/// Every stored policy, oldest change first.
pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<ArchivePolicy>> {
    let rows: Vec<PolicyRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} ORDER BY updated_at, podcast_id"
    ))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(PolicyRow::into_model).collect()
}

/// Drops a podcast's own policy so it follows the global defaults again.
pub async fn delete(conn: &mut SqliteConnection, podcast_id: PodcastId) -> Result<bool> {
    let affected = sqlx::query(&format!("DELETE FROM {TABLE} WHERE podcast_id = ?1"))
        .bind(podcast_id.to_string())
        .execute(conn)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::OffsetDateTime;

    use super::*;
    use crate::{Storage, podcasts};

    #[tokio::test]
    async fn policies_round_trip_and_reset() {
        let s = Storage::open_temp().await.unwrap();
        let a = podcasts::sample("A");
        let b = podcasts::sample("B");
        let mut tx = s.begin().await.unwrap();
        podcasts::insert(&mut tx, &a).await.unwrap();
        podcasts::insert(&mut tx, &b).await.unwrap();
        tx.commit().await.unwrap();

        let mut w = s.writer().await.unwrap();
        assert!(get(&mut w, a.id).await.unwrap().is_none());

        let now = OffsetDateTime::now_utc();
        let policy = ArchivePolicy {
            podcast_id: a.id,
            mode: PolicyMode::Auto,
            max_backlog: Some(5),
            max_age_days: None,
            priority: Some(Priority::High),
            updated_at: now,
        };
        set(&mut w, &policy).await.unwrap();
        let stored = get(&mut w, a.id).await.unwrap().unwrap();
        assert_eq!(stored.mode, PolicyMode::Auto);
        assert_eq!(stored.max_backlog, Some(5));
        assert_eq!(stored.max_age_days, None, "unset means: use the default");
        assert_eq!(stored.priority, Some(Priority::High));

        // Writing again replaces rather than duplicates.
        set(
            &mut w,
            &ArchivePolicy {
                mode: PolicyMode::Manual,
                max_backlog: None,
                ..policy.clone()
            },
        )
        .await
        .unwrap();
        let stored = get(&mut w, a.id).await.unwrap().unwrap();
        assert_eq!(stored.mode, PolicyMode::Manual);
        assert_eq!(stored.max_backlog, None);
        assert_eq!(list(&mut w).await.unwrap().len(), 1);

        set(
            &mut w,
            &ArchivePolicy {
                podcast_id: b.id,
                ..policy
            },
        )
        .await
        .unwrap();
        assert_eq!(list(&mut w).await.unwrap().len(), 2);

        assert!(delete(&mut w, a.id).await.unwrap());
        assert!(get(&mut w, a.id).await.unwrap().is_none());
        assert!(!delete(&mut w, a.id).await.unwrap());
        drop(w);
        s.close().await;
    }
}
