//! `podcast_artwork` repository (ADR 0026).
//!
//! Artwork is content-addressed: the file on disk is named after its hash,
//! so storing a replacement can never destroy the one before it and a
//! re-fetch of unchanged bytes is a no-op. Exactly one row per podcast
//! carries `is_current`, which a partial unique index enforces — the
//! database is the boundary, not a check that could race.
//!
//! It lives in its own table rather than in `archive_files` because that
//! table is for episode media: its `episode_id` is unique and NOT NULL, so
//! a podcast-level asset has nowhere to sit.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::archive::{ArtworkFormat, PodcastArtwork};
use uguisu_core::ids::{ArtworkId, PodcastId};

use crate::row::{self, i64_from_u64, u64_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "podcast_artwork";

const COLUMNS: &str = "id, podcast_id, source_url, relative_path, format, content_type, \
    size_bytes, hash_algo, hash_value, etag, last_modified, is_current, retrieved_at, \
    created_at, updated_at";

#[derive(FromRow)]
struct ArtworkRow {
    id: String,
    podcast_id: String,
    source_url: Option<String>,
    relative_path: String,
    format: String,
    content_type: Option<String>,
    size_bytes: i64,
    hash_algo: String,
    hash_value: String,
    etag: Option<String>,
    last_modified: Option<String>,
    is_current: i64,
    retrieved_at: String,
    created_at: String,
    updated_at: String,
}

impl ArtworkRow {
    fn into_model(self) -> Result<PodcastArtwork> {
        let rid = self.id.clone();
        Ok(PodcastArtwork {
            id: row::id(TABLE, &rid, &self.id)?,
            podcast_id: row::id(TABLE, &rid, &self.podcast_id)?,
            source_url: row::opt_url(self.source_url.as_deref()),
            relative_path: self.relative_path,
            format: row::parse_enum(TABLE, &rid, "format", &self.format, ArtworkFormat::parse)?,
            content_type: self.content_type,
            size_bytes: u64_from(Some(self.size_bytes)).unwrap_or(0),
            hash_algo: self.hash_algo,
            hash_value: self.hash_value,
            etag: self.etag,
            last_modified: self.last_modified,
            is_current: self.is_current != 0,
            retrieved_at: row::ts(TABLE, &rid, &self.retrieved_at)?,
            created_at: row::ts(TABLE, &rid, &self.created_at)?,
            updated_at: row::ts(TABLE, &rid, &self.updated_at)?,
        })
    }
}

/// Stores artwork and makes it the podcast's current one.
///
/// Keyed on `(podcast_id, hash_value)`, so re-fetching identical bytes
/// updates the validators of the row that already exists instead of
/// creating a second one. Call inside a transaction that first ran
/// [`clear_current`]: the partial unique index refuses two current rows,
/// which is exactly the protection wanted.
pub async fn upsert_current(conn: &mut SqliteConnection, a: &PodcastArtwork) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO {TABLE} ({COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) \
         ON CONFLICT (podcast_id, hash_value) DO UPDATE SET \
           source_url = excluded.source_url, content_type = excluded.content_type, \
           etag = excluded.etag, last_modified = excluded.last_modified, \
           is_current = excluded.is_current, retrieved_at = excluded.retrieved_at, \
           updated_at = excluded.updated_at"
    ))
    .bind(a.id.to_string())
    .bind(a.podcast_id.to_string())
    .bind(a.source_url.as_ref().map(ToString::to_string))
    .bind(&a.relative_path)
    .bind(a.format.as_str())
    .bind(&a.content_type)
    .bind(i64_from_u64(a.size_bytes))
    .bind(&a.hash_algo)
    .bind(&a.hash_value)
    .bind(&a.etag)
    .bind(&a.last_modified)
    .bind(i64::from(a.is_current))
    .bind(to_db_ts(a.retrieved_at))
    .bind(to_db_ts(a.created_at))
    .bind(to_db_ts(a.updated_at))
    .execute(conn)
    .await?;
    Ok(())
}

/// Demotes whatever artwork the podcast currently uses. The file stays on
/// disk: nothing here deletes.
pub async fn clear_current(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    now: OffsetDateTime,
) -> Result<u64> {
    Ok(sqlx::query(&format!(
        "UPDATE {TABLE} SET is_current = 0, updated_at = ?2 WHERE podcast_id = ?1 AND is_current = 1"
    ))
    .bind(podcast_id.to_string())
    .bind(to_db_ts(now))
    .execute(conn)
    .await?
    .rows_affected())
}

/// The artwork a podcast currently uses, if it has any.
pub async fn current(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Option<PodcastArtwork>> {
    let row: Option<ArtworkRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} WHERE podcast_id = ?1 AND is_current = 1"
    ))
    .bind(podcast_id.to_string())
    .fetch_optional(conn)
    .await?;
    row.map(ArtworkRow::into_model).transpose()
}

/// One artwork record by its identifier.
pub async fn get(conn: &mut SqliteConnection, id: ArtworkId) -> Result<Option<PodcastArtwork>> {
    let row: Option<ArtworkRow> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE id = ?1"))
            .bind(id.to_string())
            .fetch_optional(conn)
            .await?;
    row.map(ArtworkRow::into_model).transpose()
}

/// Every artwork stored for a podcast, newest fetch first. Superseded
/// images stay listed: they are still on disk, and nothing removes them.
pub async fn list_for_podcast(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Vec<PodcastArtwork>> {
    let rows: Vec<ArtworkRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM {TABLE} WHERE podcast_id = ?1 ORDER BY retrieved_at DESC, id DESC"
    ))
    .bind(podcast_id.to_string())
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(ArtworkRow::into_model).collect()
}

/// Records that a conditional request said the stored bytes are still
/// current. Only the validators and the fetch time move.
pub async fn touch(
    conn: &mut SqliteConnection,
    id: ArtworkId,
    etag: Option<&str>,
    last_modified: Option<&str>,
    now: OffsetDateTime,
) -> Result<bool> {
    let affected = sqlx::query(&format!(
        "UPDATE {TABLE} SET etag = COALESCE(?1, etag), \
         last_modified = COALESCE(?2, last_modified), retrieved_at = ?3, updated_at = ?3 \
         WHERE id = ?4"
    ))
    .bind(etag)
    .bind(last_modified)
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{Storage, podcasts};

    fn art(podcast: PodcastId, hash: &str, current: bool) -> PodcastArtwork {
        let now = OffsetDateTime::now_utc();
        PodcastArtwork {
            id: ArtworkId::new(),
            podcast_id: podcast,
            source_url: Some("https://cdn.example/cover.jpg".parse().unwrap()),
            relative_path: format!(".uguisu/artwork/{podcast}/{hash}.jpg"),
            format: ArtworkFormat::Jpeg,
            content_type: Some("image/jpeg".into()),
            size_bytes: 2048,
            hash_algo: "sha256".into(),
            hash_value: hash.to_owned(),
            etag: Some("\"v1\"".into()),
            last_modified: None,
            is_current: current,
            retrieved_at: now,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn one_current_image_old_ones_kept() {
        let s = Storage::open_temp().await.unwrap();
        let p = podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        podcasts::insert(&mut tx, &p).await.unwrap();
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();

        let first = art(p.id, "aa", true);
        upsert_current(&mut w, &first).await.unwrap();
        assert_eq!(current(&mut w, p.id).await.unwrap().unwrap().id, first.id);

        // A second image without demoting the first is refused by the
        // index, not by a check that could race two fetches.
        let second = art(p.id, "bb", true);
        let err = upsert_current(&mut w, &second).await.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("unique"),
            "{err} names the constraint"
        );

        let now = OffsetDateTime::now_utc();
        assert_eq!(clear_current(&mut w, p.id, now).await.unwrap(), 1);
        upsert_current(&mut w, &second).await.unwrap();
        assert_eq!(current(&mut w, p.id).await.unwrap().unwrap().id, second.id);

        // The superseded image is still recorded: its file is still there.
        let all = list_for_podcast(&mut w, p.id).await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|a| a.id == first.id && !a.is_current));
        assert_eq!(
            get(&mut w, first.id).await.unwrap().unwrap().hash_value,
            "aa"
        );
        drop(w);
        s.close().await;
    }

    #[tokio::test]
    async fn identical_bytes_revalidate_instead_of_duplicating() {
        let s = Storage::open_temp().await.unwrap();
        let p = podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        podcasts::insert(&mut tx, &p).await.unwrap();
        tx.commit().await.unwrap();
        let mut w = s.writer().await.unwrap();

        let a = art(p.id, "aa", true);
        upsert_current(&mut w, &a).await.unwrap();
        let mut again = art(p.id, "aa", true);
        again.etag = Some("\"v2\"".into());
        upsert_current(&mut w, &again).await.unwrap();
        let all = list_for_podcast(&mut w, p.id).await.unwrap();
        assert_eq!(all.len(), 1, "the same bytes are the same record");
        assert_eq!(all[0].id, a.id, "the record keeps its identity");
        assert_eq!(all[0].etag.as_deref(), Some("\"v2\""));

        let later = OffsetDateTime::now_utc() + time::Duration::seconds(60);
        assert!(touch(&mut w, a.id, None, Some("Mon"), later).await.unwrap());
        let stored = get(&mut w, a.id).await.unwrap().unwrap();
        assert_eq!(
            stored.etag.as_deref(),
            Some("\"v2\""),
            "a 304 carrying no validator keeps the one on file"
        );
        assert_eq!(stored.last_modified.as_deref(), Some("Mon"));
        assert!(stored.retrieved_at > a.retrieved_at);
        assert!(
            !touch(&mut w, ArtworkId::new(), None, None, later)
                .await
                .unwrap()
        );
        drop(w);
        s.close().await;
    }
}
