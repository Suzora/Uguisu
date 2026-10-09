//! Database maintenance (ADR 0056): the schema version, a consistent copy,
//! the integrity checks and `VACUUM`. Nothing here runs on its own.

use std::path::Path;

use sqlx::SqliteConnection;

use crate::Result;

/// The migrations applied to a database, newest last; empty for a database
/// no migration has touched yet.
pub async fn applied_versions(conn: &mut SqliteConnection) -> Result<Vec<i64>> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&mut *conn)
    .await?;
    if exists == 0 {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations WHERE success = 1 ORDER BY version",
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Writes a consistent, compacted copy of the database to `path`, which must
/// not exist. The caller owns making it durable and moving it into place.
pub async fn copy_into(conn: &mut SqliteConnection, path: &Path) -> Result<()> {
    let target = path.to_str().ok_or_else(|| {
        crate::StorageError::Config(format!("{} is not valid UTF-8", path.display()))
    })?;
    sqlx::query("VACUUM INTO ?1")
        .bind(target)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// What `PRAGMA integrity_check` and `PRAGMA foreign_key_check` report, at
/// most `limit` lines each; both empty means the database is sound.
pub async fn check(conn: &mut SqliteConnection, limit: u32) -> Result<(Vec<String>, Vec<String>)> {
    let integrity: Vec<String> = sqlx::query_scalar(&format!("PRAGMA integrity_check({limit})"))
        .fetch_all(&mut *conn)
        .await?;
    let integrity = if integrity == ["ok"] {
        Vec::new()
    } else {
        integrity
    };
    let rows: Vec<(String, Option<i64>, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&mut *conn)
        .await?;
    let foreign_keys = rows
        .into_iter()
        .take(limit as usize)
        .map(|(table, rowid, parent, fk)| {
            format!(
                "{table} row {} references a missing {parent} (constraint {fk})",
                rowid.map_or_else(|| "?".to_owned(), |r| r.to_string())
            )
        })
        .collect();
    Ok((integrity, foreign_keys))
}

/// The database's size in bytes, as its pages count it.
pub async fn size(conn: &mut SqliteConnection) -> Result<u64> {
    let pages: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(&mut *conn)
        .await?;
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(&mut *conn)
        .await?;
    Ok(u64::try_from(pages.saturating_mul(page_size)).unwrap_or(0))
}

/// Rebuilds the database file without its free pages. Must run outside a
/// transaction, on the writer.
pub async fn vacuum(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("VACUUM").execute(&mut *conn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::OffsetDateTime;

    use super::*;
    use crate::{Storage, settings};

    #[tokio::test]
    async fn opening_records_the_schema() {
        let s = Storage::open_temp().await.unwrap();
        let first = s.schema().clone();
        assert_eq!(first.found, None);
        assert_eq!(first.applied.last().copied(), Some(first.now));
        let path = s.path().to_path_buf();
        s.close().await;
        let again = Storage::open_path(&path).await.unwrap();
        assert_eq!(again.schema().found, Some(first.now));
        assert!(again.schema().applied.is_empty());
    }

    #[tokio::test]
    async fn copy_keeps_every_row() {
        let s = Storage::open_temp().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        settings::set(&mut tx, "UGUISU_X", "kept", None, OffsetDateTime::now_utc())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("copy.db");
        copy_into(&mut s.reader().await.unwrap(), &copy)
            .await
            .unwrap();

        let opened = Storage::open_path(&copy).await.unwrap();
        assert!(opened.schema().applied.is_empty(), "{:?}", opened.schema());
        let kept = settings::get(&mut opened.reader().await.unwrap(), "UGUISU_X")
            .await
            .unwrap();
        assert_eq!(kept.map(|s| s.value), Some("kept".to_owned()));
        // The copy refuses a file that is already there.
        assert!(
            copy_into(&mut s.reader().await.unwrap(), &copy)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn broken_reference_is_reported() {
        let s = Storage::open_temp().await.unwrap();
        assert_eq!(
            check(&mut s.reader().await.unwrap(), 100).await.unwrap(),
            (Vec::new(), Vec::new())
        );
        let mut w = s.writer().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *w)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO archive_policies (podcast_id, mode, updated_at) \
             VALUES ('01J00000000000000000000000', 'auto', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut *w)
        .await
        .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *w)
            .await
            .unwrap();
        drop(w);
        let (integrity, foreign_keys) = check(&mut s.reader().await.unwrap(), 100).await.unwrap();
        assert!(integrity.is_empty(), "{integrity:?}");
        assert_eq!(foreign_keys.len(), 1, "{foreign_keys:?}");
        assert!(
            foreign_keys[0].starts_with("archive_policies row"),
            "{foreign_keys:?}"
        );
    }

    #[tokio::test]
    async fn vacuum_frees_deleted_pages() {
        let s = Storage::open_temp().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        let big = "x".repeat(4096);
        for i in 0..400 {
            settings::set(
                &mut tx,
                &format!("K{i}"),
                &big,
                None,
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        }
        settings::set(&mut tx, "KEPT", "yes", None, OffsetDateTime::now_utc())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();
        sqlx::query("DELETE FROM settings WHERE key LIKE 'K%' AND key <> 'KEPT'")
            .execute(&mut *w)
            .await
            .unwrap();
        let before = size(&mut w).await.unwrap();
        vacuum(&mut w).await.unwrap();
        let after = size(&mut w).await.unwrap();
        assert!(after < before, "{before} -> {after}");
        drop(w);
        let kept = settings::get(&mut s.reader().await.unwrap(), "KEPT")
            .await
            .unwrap();
        assert_eq!(kept.map(|s| s.value), Some("yes".to_owned()));
    }
}
