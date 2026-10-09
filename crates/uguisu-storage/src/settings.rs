//! `settings` repository: the persisted configuration layer (ADR 0028).
//!
//! Values are stored in exactly the syntax the matching environment
//! variable uses, so there is one vocabulary and one parser. Nothing here
//! validates: `Config::from_layers` does, over the merged view, because a
//! value can only be wrong in company (`per_host <= global`).
//!
//! A row is never deleted to tidy up. It is the only copy of what
//! somebody meant, so a value Uguisu cannot use is reported and ignored.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::settings::Setting;

use crate::row;
use crate::{Result, to_db_ts};

const TABLE: &str = "settings";

#[derive(FromRow)]
struct SettingRow {
    key: String,
    value: String,
    updated_at: String,
    updated_by: Option<String>,
}

impl SettingRow {
    fn into_model(self) -> Result<Setting> {
        Ok(Setting {
            updated_at: row::ts(TABLE, &self.key, &self.updated_at)?,
            key: self.key,
            value: self.value,
            updated_by: self.updated_by,
        })
    }
}

/// Every stored setting, by key.
///
/// Unknown keys come back too: the engine reports them rather than
/// silently dropping them, because a typo the user cannot see is a typo
/// they cannot fix.
pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<Setting>> {
    let rows: Vec<SettingRow> =
        sqlx::query_as("SELECT key, value, updated_at, updated_by FROM settings ORDER BY key")
            .fetch_all(conn)
            .await?;
    rows.into_iter().map(SettingRow::into_model).collect()
}

/// One stored setting.
pub async fn get(conn: &mut SqliteConnection, key: &str) -> Result<Option<Setting>> {
    let row: Option<SettingRow> =
        sqlx::query_as("SELECT key, value, updated_at, updated_by FROM settings WHERE key = ?1")
            .bind(key)
            .fetch_optional(conn)
            .await?;
    row.map(SettingRow::into_model).transpose()
}

/// Writes a setting, replacing any previous value.
pub async fn set(
    conn: &mut SqliteConnection,
    key: &str,
    value: &str,
    updated_by: Option<&str>,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (key) DO UPDATE SET value = ?2, updated_at = ?3, updated_by = ?4",
    )
    .bind(key)
    .bind(value)
    .bind(to_db_ts(now))
    .bind(updated_by)
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes a setting. `false` when there was nothing to remove.
///
/// The only caller is an explicit "clear this key": nothing deletes a row
/// on its own, not even one that no longer parses.
pub async fn delete(conn: &mut SqliteConnection, key: &str) -> Result<bool> {
    let n = sqlx::query("DELETE FROM settings WHERE key = ?1")
        .bind(key)
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
    async fn settings_round_trip_and_replace_by_key() {
        let s = Storage::open_temp().await.unwrap();
        let t0 = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let mut tx = s.begin().await.unwrap();
        set(
            &mut tx,
            "UGUISU_FEED_REFRESH_CONCURRENCY",
            "16",
            Some("api"),
            t0,
        )
        .await
        .unwrap();
        set(&mut tx, "UGUISU_ARCHIVE_AUTO_DOWNLOAD", "true", None, t0)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut r = s.reader().await.unwrap();
        let all = list(&mut r).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].key, "UGUISU_ARCHIVE_AUTO_DOWNLOAD");
        let one = get(&mut r, "UGUISU_FEED_REFRESH_CONCURRENCY")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(one.value, "16");
        assert_eq!(one.updated_by.as_deref(), Some("api"));
        assert_eq!(one.updated_at, t0);
        assert!(get(&mut r, "UGUISU_NOTHING").await.unwrap().is_none());

        // The map the configuration layer takes.
        let map = uguisu_core::settings::to_map(&all);
        assert_eq!(
            map.get("UGUISU_FEED_REFRESH_CONCURRENCY")
                .map(String::as_str),
            Some("16")
        );

        let t1 = t0 + time::Duration::minutes(5);
        let mut w = s.writer().await.unwrap();
        set(
            &mut w,
            "UGUISU_FEED_REFRESH_CONCURRENCY",
            "4",
            Some("cli"),
            t1,
        )
        .await
        .unwrap();
        let one = get(&mut w, "UGUISU_FEED_REFRESH_CONCURRENCY")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(one.value, "4");
        assert_eq!(one.updated_by.as_deref(), Some("cli"));
        assert_eq!(list(&mut w).await.unwrap().len(), 2, "replaced, not added");

        assert!(
            delete(&mut w, "UGUISU_FEED_REFRESH_CONCURRENCY")
                .await
                .unwrap()
        );
        assert!(
            !delete(&mut w, "UGUISU_FEED_REFRESH_CONCURRENCY")
                .await
                .unwrap()
        );
        assert_eq!(list(&mut w).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_unknown_key_is_stored_anyway() {
        let s = Storage::open_temp().await.unwrap();
        let mut w = s.writer().await.unwrap();
        set(
            &mut w,
            "UGUISU_TYPO_NOBODY_READS",
            "1",
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
        let all = list(&mut w).await.unwrap();
        assert_eq!(all[0].key, "UGUISU_TYPO_NOBODY_READS");
        assert!(
            uguisu_core::settings::spec(&all[0].key).is_none(),
            "the repository stores what it is given; knowing the key is the \
             configuration layer's job"
        );
    }
}
