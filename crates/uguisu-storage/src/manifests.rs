//! `archive_manifests` repository (ADR 0024).
//!
//! The manifest file is derived data; this row is what says whether it
//! still matches the index. [`mark_stale`] runs **inside** the transaction
//! that changes an artifact, so the flag and the fact commit together: a
//! crash can only ever leave "marked stale but actually fresh", never a
//! manifest that silently disagrees with the archive.
//!
//! Nothing here reads or writes a file. Rendering belongs to
//! `uguisu-archive`, the write to the engine.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::archive::ArchiveManifest;
use uguisu_core::ids::PodcastId;

use crate::row::{self, i64_from_u64, u64_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "archive_manifests";

const COLUMNS: &str = "podcast_id, relative_path, entries, hash_value, stale, generated_at, \
    stale_since, updated_at";

#[derive(FromRow)]
struct ManifestRow {
    podcast_id: String,
    relative_path: String,
    entries: i64,
    hash_value: Option<String>,
    stale: i64,
    generated_at: Option<String>,
    stale_since: Option<String>,
    updated_at: String,
}

impl ManifestRow {
    fn into_model(self) -> Result<ArchiveManifest> {
        let rid = self.podcast_id.clone();
        Ok(ArchiveManifest {
            podcast_id: row::id(TABLE, &rid, &self.podcast_id)?,
            relative_path: self.relative_path,
            entries: u64_from(Some(self.entries)).unwrap_or(0),
            hash_value: self.hash_value,
            stale: self.stale != 0,
            generated_at: row::opt_ts(TABLE, &rid, self.generated_at.as_deref())?,
            stale_since: row::opt_ts(TABLE, &rid, self.stale_since.as_deref())?,
            updated_at: row::ts(TABLE, &rid, &self.updated_at)?,
        })
    }
}

/// Marks a podcast's manifest as out of date, creating the row if this is
/// the first artifact the podcast has.
///
/// Idempotent and O(1): marking an already-stale manifest keeps the
/// original `stale_since`, so "how long has this been behind" stays
/// answerable. Call it in the same transaction as the change that caused
/// it — registration, relocation, import or a tag write.
pub async fn mark_stale(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    relative_path: &str,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO {TABLE} (podcast_id, relative_path, entries, hash_value, stale, \
            generated_at, stale_since, updated_at) \
         VALUES (?1, ?2, 0, NULL, 1, NULL, ?3, ?3) \
         ON CONFLICT (podcast_id) DO UPDATE SET \
           relative_path = excluded.relative_path, stale = 1, \
           stale_since = COALESCE({TABLE}.stale_since, excluded.stale_since), \
           updated_at = excluded.updated_at"
    ))
    .bind(podcast_id.to_string())
    .bind(relative_path)
    .bind(to_db_ts(now))
    .execute(conn)
    .await?;
    Ok(())
}

/// Records that the manifest file now matches the index.
///
/// Clears `stale` only if nothing marked it again in the meantime: the
/// guard on `updated_at` means a registration that landed while the file
/// was being written leaves the manifest stale, and the next flush picks
/// it up. Returns whether the row was cleared.
pub async fn mark_written(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    relative_path: &str,
    entries: u64,
    hash_value: &str,
    marked_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET relative_path = ?2, entries = ?3, hash_value = ?4, stale = 0, \
           generated_at = ?5, stale_since = NULL, updated_at = ?5 \
         WHERE podcast_id = ?1 AND updated_at <= ?6"
    ))
    .bind(podcast_id.to_string())
    .bind(relative_path)
    .bind(i64_from_u64(entries))
    .bind(hash_value)
    .bind(to_db_ts(now))
    .bind(to_db_ts(marked_at))
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// One podcast's manifest state, if it has one.
pub async fn get(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Option<ArchiveManifest>> {
    let row: Option<ManifestRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} WHERE podcast_id = ?1"
    ))
    .bind(podcast_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(ManifestRow::into_model).transpose()
}

/// The manifests that need rewriting, longest-stale first, bounded.
///
/// Backed by a partial index, so asking costs nothing when everything is
/// current — which is the normal state and must stay cheap to confirm.
pub async fn stale(conn: &mut SqliteConnection, limit: u32) -> Result<Vec<ArchiveManifest>> {
    let rows: Vec<ManifestRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} WHERE stale = 1 \
         ORDER BY stale_since, podcast_id LIMIT ?1"
    ))
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(ManifestRow::into_model).collect()
}

/// Every manifest, for `archive manifest status`.
pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<ArchiveManifest>> {
    let rows: Vec<ManifestRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} ORDER BY podcast_id"
    ))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(ManifestRow::into_model).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::Duration as TimeDuration;

    use super::*;
    use crate::{Storage, podcasts};

    #[tokio::test]
    async fn marking_keeps_the_first_moment() {
        let s = Storage::open_temp().await.unwrap();
        let p = podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        podcasts::insert(&mut tx, &p).await.unwrap();
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();

        let path = format!(".uguisu/manifests/{}/manifest.sha256", p.id);
        // Stored timestamps have second precision, so compare against a
        // value that survives the round trip.
        let t0 = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        mark_stale(&mut w, p.id, &path, t0).await.unwrap();
        mark_stale(&mut w, p.id, &path, t0 + TimeDuration::seconds(30))
            .await
            .unwrap();
        let m = get(&mut w, p.id).await.unwrap().unwrap();
        assert!(m.stale);
        assert_eq!(m.entries, 0);
        assert_eq!(m.hash_value, None);
        assert_eq!(
            m.stale_since.unwrap(),
            t0,
            "how long it has been behind stays answerable"
        );
        assert_eq!(stale(&mut w, 10).await.unwrap().len(), 1);
        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn a_change_mid_write_leaves_stale() {
        let s = Storage::open_temp().await.unwrap();
        let p = podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        podcasts::insert(&mut tx, &p).await.unwrap();
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();

        let path = format!(".uguisu/manifests/{}/manifest.sha256", p.id);
        let t0 = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        mark_stale(&mut w, p.id, &path, t0).await.unwrap();

        // A registration lands while the file is being rendered.
        let during = t0 + TimeDuration::seconds(1);
        mark_stale(&mut w, p.id, &path, during).await.unwrap();

        // The writer only saw the state as of `t0`, so it must not claim
        // the manifest is current.
        let after = t0 + TimeDuration::seconds(2);
        assert!(
            !mark_written(&mut w, p.id, &path, 3, "abc", t0, after)
                .await
                .unwrap(),
            "the row moved under the writer"
        );
        assert!(get(&mut w, p.id).await.unwrap().unwrap().stale);

        // The next flush sees the later mark and succeeds.
        let ok = mark_written(&mut w, p.id, &path, 4, "def", during, after)
            .await
            .unwrap();
        assert!(ok);
        let m = get(&mut w, p.id).await.unwrap().unwrap();
        assert!(!m.stale);
        assert_eq!(m.entries, 4);
        assert_eq!(m.hash_value.as_deref(), Some("def"));
        assert_eq!(m.stale_since, None);
        assert_eq!(m.generated_at.unwrap(), after);
        assert!(stale(&mut w, 10).await.unwrap().is_empty());
        assert_eq!(list(&mut w).await.unwrap().len(), 1);
        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn removing_the_podcast_removes_its_manifest_row() {
        let s = Storage::open_temp().await.unwrap();
        let p = podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        podcasts::insert(&mut tx, &p).await.unwrap();
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();
        mark_stale(&mut w, p.id, "x", OffsetDateTime::now_utc())
            .await
            .unwrap();
        drop(w);

        let mut tx = s.begin().await.unwrap();
        sqlx::query("DELETE FROM podcasts WHERE id = ?1")
            .bind(p.id.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut r = s.reader().await.unwrap();
        assert!(get(&mut r, p.id).await.unwrap().is_none());
        drop(r);
        s.close().await;
    }
}
