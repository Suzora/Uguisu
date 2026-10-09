//! Reading Podgrab's own database (ADR 0050).
//!
//! Podgrab keeps `podgrab.db`, with every downloaded episode's GUID,
//! enclosure URL, publication date and title, the path it wrote, and every
//! podcast's feed URL: what names a migrated file exactly. Its schema is
//! gorm's default naming of Podgrab's models, and every assumption about
//! it is in this file.
//!
//! The database belongs to the user and to a tool that may still be
//! running. It is opened read-only and immutable, with `query_only` on and
//! `trusted_schema` off, and refused while a journal lies beside it: an
//! immutable open would ignore a live journal, and any other open would
//! leave lock files in Podgrab's directory.

use std::path::{Path, PathBuf};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};
use url::Url;

use crate::{Result, StorageError};

/// The status Podgrab gives an episode whose file it wrote.
const DOWNLOADED: i64 = 2;

/// One episode Podgrab downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodgrabRecord {
    /// Where Podgrab wrote the file, as Podgrab saw its own disk.
    pub download_path: String,
    /// The episode's title.
    pub title: String,
    /// The episode's GUID.
    pub guid: Option<String>,
    /// The enclosure URL it downloaded.
    pub enclosure_url: Option<Url>,
    /// The publication date, in UTC.
    pub published_at: Option<OffsetDateTime>,
    /// The podcast's title.
    pub podcast_title: String,
    /// The podcast's feed URL.
    pub feed_url: Option<Url>,
}

type Row = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

/// Every episode a Podgrab database says it downloaded to a path.
///
/// Never writes, and never leaves a file beside the database.
pub async fn read(path: &Path) -> Result<Vec<PodgrabRecord>> {
    for suffix in ["-journal", "-wal"] {
        let mut beside = path.as_os_str().to_owned();
        beside.push(suffix);
        let beside = PathBuf::from(beside);
        if beside.exists() {
            return Err(StorageError::Io {
                path: beside,
                source: std::io::Error::other(
                    "Podgrab is still writing its database; stop Podgrab first",
                ),
            });
        }
    }
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(true)
        .pragma("query_only", "ON")
        .pragma("trusted_schema", "OFF")
        .connect()
        .await?;
    // gorm declares types SQLite does not enforce, so every column is cast
    // to what is read from it.
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT CAST(i.download_path AS TEXT), COALESCE(CAST(i.title AS TEXT), ''), \
                CAST(i.guid AS TEXT), CAST(i.file_url AS TEXT), CAST(i.pub_date AS TEXT), \
                COALESCE(CAST(p.title AS TEXT), ''), CAST(p.url AS TEXT) \
         FROM podcast_items i JOIN podcasts p ON p.id = i.podcast_id \
         WHERE CAST(i.download_status AS INTEGER) = ?1 \
           AND COALESCE(CAST(i.download_path AS TEXT), '') <> ''",
    )
    .bind(DOWNLOADED)
    .fetch_all(&mut conn)
    .await?;
    conn.close().await?;
    Ok(rows
        .into_iter()
        .map(
            |(download_path, title, guid, file_url, pub_date, podcast_title, feed_url)| {
                PodgrabRecord {
                    download_path,
                    title,
                    guid: guid.filter(|g| !g.trim().is_empty()),
                    enclosure_url: file_url.and_then(|u| Url::parse(u.trim()).ok()),
                    published_at: pub_date.as_deref().and_then(published),
                    podcast_title,
                    feed_url: feed_url.and_then(|u| Url::parse(u.trim()).ok()),
                }
            },
        )
        .collect())
}

/// A date as Podgrab's SQLite driver stores one,
/// `2006-01-02 15:04:05.999999999-07:00` in the feed's own offset, in UTC.
fn published(text: &str) -> Option<OffsetDateTime> {
    let text = text.trim();
    let iso = match text.as_bytes().get(10) {
        Some(b' ') => format!("{}T{}", &text[..10], &text[11..]),
        _ => text.to_owned(),
    };
    OffsetDateTime::parse(&iso, &Rfc3339)
        .ok()
        .map(|d| d.to_offset(UtcOffset::UTC))
}

/// One downloaded episode for [`write_sample`].
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone)]
pub struct SampleItem<'a> {
    /// The podcast's title.
    pub podcast_title: &'a str,
    /// The podcast's feed URL; items sharing it share a podcast.
    pub feed_url: &'a str,
    /// The episode's title.
    pub title: &'a str,
    /// The episode's GUID.
    pub guid: &'a str,
    /// The enclosure URL.
    pub file_url: &'a str,
    /// The publication date as Podgrab's driver writes it.
    pub pub_date: &'a str,
    /// Where Podgrab wrote the file.
    pub download_path: &'a str,
    /// 0 not downloaded, 1 downloading, 2 downloaded, 3 deleted.
    pub download_status: i64,
}

/// Writes a database shaped as Podgrab's gorm models make it, for tests of
/// the reader here and of the import that uses it.
#[cfg(any(test, feature = "testing"))]
pub async fn write_sample(path: &Path, items: &[SampleItem<'_>]) -> Result<()> {
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .connect()
        .await?;
    sqlx::raw_sql(
        "CREATE TABLE `podcasts` (`id` text,`created_at` datetime,`updated_at` datetime,\
         `deleted_at` datetime,`title` text,`summary` text,`author` text,`image` text,\
         `url` text,`last_episode` datetime,`is_paused` numeric DEFAULT false,\
         PRIMARY KEY (`id`));\
         CREATE TABLE `podcast_items` (`id` text,`created_at` datetime,`updated_at` datetime,\
         `deleted_at` datetime,`podcast_id` text,`title` text,`summary` text,\
         `episode_type` text,`duration` integer,`pub_date` datetime,`file_url` text,\
         `guid` text,`image` text,`download_date` datetime,`download_path` text,\
         `download_status` integer DEFAULT 0,`is_played` numeric DEFAULT false,\
         `bookmark_date` datetime,`local_image` text,`file_size` integer,PRIMARY KEY (`id`),\
         CONSTRAINT `fk_podcasts_podcast_items` FOREIGN KEY (`podcast_id`) \
         REFERENCES `podcasts`(`id`));",
    )
    .execute(&mut conn)
    .await?;
    for (n, item) in items.iter().enumerate() {
        sqlx::query("INSERT OR IGNORE INTO podcasts (id, title, url) VALUES (?1, ?2, ?1)")
            .bind(item.feed_url)
            .bind(item.podcast_title)
            .execute(&mut conn)
            .await?;
        sqlx::query(
            "INSERT INTO podcast_items (id, podcast_id, title, guid, file_url, pub_date, \
             download_path, download_status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(format!("item-{n}"))
        .bind(item.feed_url)
        .bind(item.title)
        .bind(item.guid)
        .bind(item.file_url)
        .bind(item.pub_date)
        .bind(item.download_path)
        .bind(item.download_status)
        .execute(&mut conn)
        .await?;
    }
    conn.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn item<'a>(title: &'a str, download_path: &'a str, status: i64) -> SampleItem<'a> {
        SampleItem {
            podcast_title: "Darknet Diaries",
            feed_url: "https://feeds.example/darknet.xml",
            title,
            guid: "guid-1",
            file_url: "https://cdn.example/1.mp3",
            pub_date: "2024-01-05 10:00:00+00:00",
            download_path,
            download_status: status,
        }
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn reads_downloaded_rows_with_podcast() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("podgrab.db");
        write_sample(
            &db,
            &[
                item(
                    "Der Einbruch",
                    "/assets/darknet-diaries/der-einbruch.mp3",
                    2,
                ),
                item("Kein Pfad", "", 2),
                item("Nicht geladen", "", 0),
                item("Gelöscht", "/assets/darknet-diaries/geloescht.mp3", 3),
            ],
        )
        .await
        .unwrap();
        let records = read(&db).await.unwrap();
        assert_eq!(
            records,
            [PodgrabRecord {
                download_path: "/assets/darknet-diaries/der-einbruch.mp3".to_owned(),
                title: "Der Einbruch".to_owned(),
                guid: Some("guid-1".to_owned()),
                enclosure_url: Some(Url::parse("https://cdn.example/1.mp3").unwrap()),
                published_at: Some(time::macros::datetime!(2024-01-05 10:00:00 UTC)),
                podcast_title: "Darknet Diaries".to_owned(),
                feed_url: Some(Url::parse("https://feeds.example/darknet.xml").unwrap()),
            }]
        );
    }

    #[test]
    fn pub_date_offset_becomes_utc() {
        assert_eq!(
            published("2024-01-05 23:30:00+02:00"),
            Some(time::macros::datetime!(2024-01-05 21:30:00 UTC))
        );
        assert_eq!(
            published("2024-01-05 22:00:00.123456789-05:00"),
            Some(time::macros::datetime!(2024-01-06 03:00:00.123456789 UTC))
        );
        assert_eq!(published("not a date"), None);
    }

    #[tokio::test]
    async fn hot_journal_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("podgrab.db");
        write_sample(&db, &[item("Der Einbruch", "/assets/a/b.mp3", 2)])
            .await
            .unwrap();
        std::fs::write(dir.path().join("podgrab.db-journal"), b"").unwrap();
        let err = read(&db).await.unwrap_err();
        assert!(err.to_string().contains("stop Podgrab first"), "{err}");
    }

    #[tokio::test]
    async fn podgrab_database_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("podgrab.db");
        write_sample(&db, &[item("Der Einbruch", "/assets/a/b.mp3", 2)])
            .await
            .unwrap();
        let (bytes, files) = (std::fs::read(&db).unwrap(), listing(dir.path()));
        assert_eq!(read(&db).await.unwrap().len(), 1);
        assert_eq!(std::fs::read(&db).unwrap(), bytes);
        assert_eq!(listing(dir.path()), files, "no -wal, -shm or -journal");
    }

    #[tokio::test]
    async fn foreign_schema_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.db");
        let mut conn = SqliteConnectOptions::new()
            .filename(&other)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE notes (body text)")
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
        let err = read(&other).await.unwrap_err();
        assert!(err.to_string().contains("no such table"), "{err}");
        std::fs::write(dir.path().join("text.db"), b"just text").unwrap();
        assert!(read(&dir.path().join("text.db")).await.is_err());
    }
}
