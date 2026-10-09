//! `scheduler_control` repository (ADR 0027).
//!
//! One row, like `download_control`, and deliberately not that row: a full
//! disk pauses transfers by itself, and feeds must keep being refreshed
//! when transfers stop. The pause survives a restart, because "I paused
//! the scheduler" is a decision, not a runtime detail.

use sqlx::SqliteConnection;
use time::OffsetDateTime;
use uguisu_core::schedule::SchedulerControl;

use crate::row;
use crate::{Result, to_db_ts};

const TABLE: &str = "scheduler_control";
const ROW: &str = "1";

/// The control row, which migration 0005 seeded.
pub async fn control_get(conn: &mut SqliteConnection) -> Result<SchedulerControl> {
    let (paused, paused_reason, paused_at, last_maintenance_at, updated_at): (
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT paused, paused_reason, paused_at, last_maintenance_at, updated_at \
         FROM scheduler_control WHERE id = 1",
    )
    .fetch_one(conn)
    .await?;
    Ok(SchedulerControl {
        paused: paused != 0,
        paused_reason,
        paused_at: row::opt_ts(TABLE, ROW, paused_at.as_deref())?,
        last_maintenance_at: row::opt_ts(TABLE, ROW, last_maintenance_at.as_deref())?,
        updated_at: row::ts(TABLE, ROW, &updated_at)?,
    })
}

/// Pauses or resumes automatic refreshing, and returns the row as it now
/// reads.
///
/// `paused_at` keeps the instant of the *first* pause across a repeated
/// one, so "paused since" stays true when an operator pauses an already
/// paused scheduler; resuming clears it.
pub async fn control_set(
    conn: &mut SqliteConnection,
    paused: bool,
    reason: Option<&str>,
    now: OffsetDateTime,
) -> Result<SchedulerControl> {
    sqlx::query(
        "UPDATE scheduler_control SET paused = ?1, paused_reason = ?2, \
         paused_at = CASE WHEN ?1 = 1 THEN COALESCE(paused_at, ?3) ELSE NULL END, \
         updated_at = ?3 WHERE id = 1",
    )
    .bind(i64::from(paused))
    .bind(reason.filter(|_| paused))
    .bind(to_db_ts(now))
    .execute(&mut *conn)
    .await?;
    control_get(conn).await
}

/// Records that the maintenance pass ran, so a restart can work out when
/// the next one is due instead of running it on every boot.
pub async fn set_last_maintenance(conn: &mut SqliteConnection, at: OffsetDateTime) -> Result<()> {
    sqlx::query(
        "UPDATE scheduler_control SET last_maintenance_at = ?1, updated_at = ?1 WHERE id = 1",
    )
    .bind(to_db_ts(at))
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Storage;

    #[tokio::test]
    async fn a_fresh_database_remembers_a_pause() {
        let store = Storage::open_temp().await.unwrap();
        let mut r = store.reader().await.unwrap();
        let control = control_get(&mut r).await.unwrap();
        assert!(!control.paused, "a new installation refreshes");
        assert!(control.paused_reason.is_none());
        assert!(control.last_maintenance_at.is_none());

        let t0 = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let mut w = store.writer().await.unwrap();
        let control = control_set(&mut w, true, Some("moving house"), t0)
            .await
            .unwrap();
        assert!(control.paused);
        assert_eq!(control.paused_reason.as_deref(), Some("moving house"));
        assert_eq!(control.paused_at, Some(t0));

        // Pausing again keeps "paused since" honest.
        let later = t0 + time::Duration::hours(2);
        let control = control_set(&mut w, true, Some("still moving"), later)
            .await
            .unwrap();
        assert_eq!(control.paused_at, Some(t0));
        assert_eq!(control.paused_reason.as_deref(), Some("still moving"));

        let control = control_set(&mut w, false, None, later).await.unwrap();
        assert!(!control.paused);
        assert!(control.paused_at.is_none() && control.paused_reason.is_none());

        set_last_maintenance(&mut w, later).await.unwrap();
        assert_eq!(
            control_get(&mut w).await.unwrap().last_maintenance_at,
            Some(later)
        );
        // Still one row, whatever anyone writes.
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM scheduler_control")
            .fetch_one(&mut *w)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }
}
