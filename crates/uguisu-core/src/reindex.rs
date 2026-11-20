//! Rebuilds the index from the archive on disk.
//!
//! The database is only an index; `docs/ARCHIVE.md` describes the files it is rebuilt from.
//! What is not in the archive by design – feed passwords, the login, the state of unfinished
//! downloads – is not restored.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::archive::{hash_file, mime_from_extension};
use crate::feed::without_query;
use crate::service::Uguisu;
use crate::util::{now, parse_rfc3339, random_token};

/// What a rebuild found.
#[derive(Debug, Default)]
pub struct ReindexReport {
    pub podcasts: usize,
    pub episodes: usize,
    pub files: usize,
    /// Things that could not be read and were left out, as human-readable lines.
    pub problems: Vec<String>,
}

async fn read_json(path: &Path) -> Result<Value> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("{} is not valid JSON", path.display()))
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn time(value: &Value) -> Option<i64> {
    value.as_str().and_then(parse_rfc3339)
}

/// `20240102T100000Z`, as used in file names.
fn stamp(input: &str) -> Option<i64> {
    let b = input.get(..16)?;
    parse_rfc3339(&format!(
        "{}-{}-{}T{}:{}:{}Z",
        b.get(0..4)?,
        b.get(4..6)?,
        b.get(6..8)?,
        b.get(9..11)?,
        b.get(11..13)?,
        b.get(13..15)?
    ))
}

async fn subdirectories(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", dir.display())),
    };
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir()
            && let Ok(name) = entry.file_name().into_string()
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

async fn file_names(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return Ok(names);
    };
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file()
            && let Ok(name) = entry.file_name().into_string()
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

impl Uguisu {
    /// Fills an empty index from the archive. Refuses to run on an index that already knows
    /// podcasts, so that nothing is mixed up or overwritten.
    pub async fn reindex(&self) -> Result<ReindexReport> {
        let known: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM podcasts")
            .fetch_one(self.pool())
            .await?;
        if known > 0 {
            bail!(
                "the database already knows {known} podcasts; a rebuild starts from an empty \
                 database – stop Uguisu and move uguisu.db (and uguisu.db-wal, uguisu.db-shm) away \
                 first"
            );
        }
        let mut report = ReindexReport::default();
        let archive = self.archive_dir();
        for dir in subdirectories(&archive).await? {
            if !tokio::fs::try_exists(archive.join(&dir).join("podcast.json"))
                .await
                .unwrap_or(false)
            {
                continue;
            }
            if let Err(error) = self.reindex_podcast(&dir, &mut report).await {
                report.problems.push(format!("{dir}: {error:#}"));
            }
        }
        Ok(report)
    }

    async fn reindex_podcast(&self, dir: &str, report: &mut ReindexReport) -> Result<()> {
        let path = self.archive_path(dir);
        let podcast = read_json(&path.join("podcast.json")).await?;
        let settings = read_json(&path.join("uguisu.json"))
            .await
            .unwrap_or(Value::Null);
        let feed_url = text(&podcast["feed_url"]).context("podcast.json has no feed_url")?;
        let last_fetched = time(&podcast["last_fetched"]);
        let cover = file_names(&path)
            .await?
            .into_iter()
            .find(|name| name.starts_with("cover.") && !name.ends_with(".part"));
        let namespaces = podcast
            .get("namespaces")
            .filter(|value| value.is_array())
            .map(Value::to_string);

        let mut tx = self.pool().begin().await?;
        let podcast_id: i64 = sqlx::query_scalar(
            "INSERT INTO podcasts (feed_url, original_feed_url, podcast_guid, title, author, \
             description, link, image_url, language, dir, cover_file, feed_token, include_removed, \
             keep_versions, archive_from, namespaces, created_at, last_fetched_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&feed_url)
        .bind(text(&podcast["original_feed_url"]).unwrap_or_else(|| feed_url.clone()))
        .bind(text(&podcast["podcast_guid"]))
        .bind(text(&podcast["title"]).unwrap_or_else(|| dir.to_string()))
        .bind(text(&podcast["author"]))
        .bind(text(&podcast["description"]))
        .bind(text(&podcast["link"]))
        .bind(text(&podcast["image"]))
        .bind(text(&podcast["language"]))
        .bind(dir)
        .bind(cover)
        .bind(text(&settings["feed_token"]).unwrap_or_else(|| random_token(24)))
        .bind(settings["include_removed"].as_bool().unwrap_or(true))
        .bind(settings["keep_versions"].as_bool().unwrap_or(false))
        .bind(time(&settings["archive_from"]))
        .bind(namespaces)
        .bind(
            time(&settings["added"])
                .or(last_fetched)
                .unwrap_or_else(now),
        )
        .bind(last_fetched)
        .fetch_one(&mut *tx)
        .await?;

        // Feed snapshots: the name gives the time, the content the checksum.
        for name in file_names(&path.join("feeds")).await? {
            if !name.ends_with(".xml") {
                continue;
            }
            let file = path.join("feeds").join(&name);
            let (hash, size) = tokio::task::spawn_blocking(move || hash_file(&file)).await??;
            sqlx::query(
                "INSERT OR IGNORE INTO feed_snapshots (podcast_id, fetched_at, blake3, size, path) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(podcast_id)
            .bind(stamp(&name).or(last_fetched).unwrap_or_else(now))
            .bind(hash)
            .bind(size as i64)
            .bind(format!("{dir}/feeds/{name}"))
            .execute(&mut *tx)
            .await?;
        }

        for episode_dir in subdirectories(&path.join("episodes")).await? {
            let relative = format!("{dir}/episodes/{episode_dir}");
            match self
                .reindex_episode(&mut tx, podcast_id, &relative, &episode_dir, last_fetched)
                .await
            {
                Ok(files) => {
                    report.episodes += 1;
                    report.files += files;
                }
                Err(error) => report.problems.push(format!("{relative}: {error:#}")),
            }
        }
        tx.commit().await?;
        report.podcasts += 1;
        Ok(())
    }

    /// Returns the number of files indexed for the episode.
    async fn reindex_episode(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        podcast_id: i64,
        relative: &str,
        dir_name: &str,
        last_fetched: Option<i64>,
    ) -> Result<usize> {
        let path = self.archive_path(relative);
        let episode = read_json(&path.join("episode.json")).await?;
        let guid = text(&episode["guid"]).context("episode.json has no guid")?;
        let item_xml = tokio::fs::read_to_string(path.join("item.xml"))
            .await
            .unwrap_or_default();
        // Files from before these fields existed get stand-ins. A stand-in hash merely makes
        // the next check of the feed record the entry once more.
        let hash = text(&episode["hash"])
            .unwrap_or_else(|| blake3::hash(item_xml.as_bytes()).to_hex().to_string());
        let first_seen = time(&episode["first_seen"])
            .or(last_fetched)
            .unwrap_or_else(now);

        let episode_id: i64 = sqlx::query_scalar(
            "INSERT INTO episodes (id, podcast_id, guid, title, description, link, published_at, \
             duration, enclosure_url, enclosure_type, enclosure_length, image_url, season, \
             episode_no, dir, meta_hash, first_seen_at, last_seen_at, removed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(episode["id"].as_i64())
        .bind(podcast_id)
        .bind(&guid)
        .bind(text(&episode["title"]).unwrap_or_else(|| "Untitled episode".into()))
        .bind(text(&episode["description"]))
        .bind(text(&episode["link"]))
        .bind(time(&episode["published"]))
        .bind(episode["duration_seconds"].as_i64())
        .bind(text(&episode["enclosure"]["url"]))
        .bind(text(&episode["enclosure"]["type"]))
        .bind(episode["enclosure"]["length"].as_i64())
        .bind(text(&episode["image"]))
        .bind(episode["season"].as_i64())
        .bind(episode["episode"].as_i64())
        .bind(dir_name)
        .bind(&hash)
        .bind(first_seen)
        .bind(last_fetched.unwrap_or(first_seen))
        .bind(time(&episode["removed"]))
        .fetch_one(&mut **tx)
        .await?;

        // Every recorded state of the feed entry; without a list, the current one.
        let listed = episode["revisions"].as_array().cloned().unwrap_or_default();
        let mut have_current = false;
        for revision in &listed {
            let (Some(file), Some(revision_hash)) =
                (revision["file"].as_str(), revision["hash"].as_str())
            else {
                continue;
            };
            let Ok(raw_xml) = tokio::fs::read_to_string(path.join(file)).await else {
                continue;
            };
            have_current |= revision_hash == hash;
            sqlx::query(
                "INSERT OR IGNORE INTO episode_revisions (episode_id, seen_at, meta_hash, raw_xml) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(time(&revision["seen"]).unwrap_or(first_seen))
            .bind(revision_hash)
            .bind(raw_xml)
            .execute(&mut **tx)
            .await?;
        }
        if !have_current && !item_xml.is_empty() {
            sqlx::query(
                "INSERT OR IGNORE INTO episode_revisions (episode_id, seen_at, meta_hash, raw_xml) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(first_seen)
            .bind(&hash)
            .bind(&item_xml)
            .execute(&mut **tx)
            .await?;
        }

        // The files. Without a list, the media files are found by their checksum files.
        let mut files = episode["files"].as_array().cloned().unwrap_or_default();
        if episode.get("files").is_none() {
            for name in file_names(&path).await? {
                let Some(media) = name.strip_suffix(".blake3") else {
                    continue;
                };
                let Ok(line) = tokio::fs::read_to_string(path.join(&name)).await else {
                    continue;
                };
                let Ok(metadata) = tokio::fs::metadata(path.join(media)).await else {
                    continue;
                };
                let extension = media.rsplit('.').next().unwrap_or_default();
                files.push(serde_json::json!({
                    "name": media,
                    "kind": "media",
                    "source": episode["enclosure"]["url"],
                    "blake3": line.split_whitespace().next(),
                    "size": metadata.len(),
                    "type": mime_from_extension(extension).unwrap_or("application/octet-stream"),
                }));
            }
        }
        let mut indexed = 0;
        for file in &files {
            let (Some(name), Some(kind), Some(checksum)) = (
                file["name"].as_str(),
                file["kind"].as_str(),
                file["blake3"].as_str(),
            ) else {
                continue;
            };
            // A name is a file in this directory, nothing else.
            if name.contains(['/', '\\']) || name.starts_with('.') {
                continue;
            }
            let present = tokio::fs::try_exists(path.join(name))
                .await
                .unwrap_or(false);
            let status = if present { "ok" } else { "missing" };
            let source = file["source"].as_str().unwrap_or_default();
            let downloaded = time(&file["downloaded"]).unwrap_or(first_seen);
            let file_path = format!("{relative}/{name}");
            let size = file["size"].as_i64().unwrap_or(0);
            let mime = file["type"].as_str().unwrap_or("application/octet-stream");
            if kind == "media" {
                sqlx::query(
                    "INSERT INTO media_files (id, episode_id, path, size, blake3, mime, \
                     source_url, downloaded_at, verified_at, status) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(file["id"].as_i64())
                .bind(episode_id)
                .bind(&file_path)
                .bind(size)
                .bind(checksum)
                .bind(mime)
                .bind(source)
                .bind(downloaded)
                .bind(downloaded)
                .bind(status)
                .execute(&mut **tx)
                .await?;
            } else {
                sqlx::query(
                    "INSERT OR IGNORE INTO attachments (id, episode_id, kind, source_key, \
                     source_url, path, size, blake3, mime, downloaded_at, verified_at, status) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(file["id"].as_i64())
                .bind(episode_id)
                .bind(kind)
                .bind(without_query(source))
                .bind(source)
                .bind(&file_path)
                .bind(size)
                .bind(checksum)
                .bind(mime)
                .bind(downloaded)
                .bind(downloaded)
                .bind(status)
                .execute(&mut **tx)
                .await?;
            }
            indexed += 1;
        }
        Ok(indexed)
    }
}
