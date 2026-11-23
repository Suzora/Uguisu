//! Takes media files that already exist on disk into the archive of a podcast.
//!
//! People who come to Uguisu often have years of episodes downloaded by a podcast app. Those
//! files are matched to the episodes of the feed; what the feed no longer lists can be kept as
//! episodes of its own, which is frequently the very reason to import.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::archive::{hash_file, mime_from_extension};
use crate::feed::without_query;
use crate::service::Uguisu;
use crate::util::{now, slugify};

#[derive(Debug, Default)]
pub struct ImportReport {
    /// File and the title of the episode it became the media file of.
    pub matched: Vec<(String, String)>,
    /// Files kept as episodes of their own because the feed does not list them.
    pub added: Vec<String>,
    /// Files whose content is already in the archive.
    pub known: Vec<String>,
    /// Files that were left alone, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Audio and video files below `dir`, in a stable order.
fn media_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current)
            .with_context(|| format!("cannot read {}", current.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if extension(&path)
                .and_then(|ext| mime_from_extension(&ext))
                .is_some_and(|mime| mime.starts_with("audio/") || mime.starts_with("video/"))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

/// The last part of an address's path, as a file name.
fn url_file_name(address: &str) -> Option<String> {
    let url = url::Url::parse(without_query(address)).ok()?;
    let name = url.path_segments()?.next_back()?;
    let decoded: String = url::form_urlencoded::parse(name.replace('+', "%2B").as_bytes())
        .map(|(key, _)| key.into_owned())
        .collect();
    (!decoded.is_empty()).then(|| decoded.to_lowercase())
}

struct Candidate {
    id: i64,
    title: String,
    dir: String,
    enclosure_url: Option<String>,
    has_media: bool,
}

impl Uguisu {
    /// Imports the media files below `dir` into a podcast. A file is matched to an episode by
    /// the file name in the episode's media address, or else by its title. With `keep_unmatched`
    /// a file that fits no episode becomes an episode of its own, marked as not in the feed.
    /// Files are copied; the directory is left as it is.
    pub async fn import_directory(
        &self,
        podcast_id: i64,
        dir: &Path,
        keep_unmatched: bool,
    ) -> Result<ImportReport> {
        let podcast = self
            .podcast(podcast_id)
            .await?
            .context("podcast not found")?;
        let files = {
            let dir = dir.to_path_buf();
            tokio::task::spawn_blocking(move || media_files(&dir)).await??
        };
        let mut candidates: Vec<Candidate> =
            sqlx::query_as::<_, (i64, String, String, Option<String>, bool)>(
                "SELECT e.id, e.title, e.dir, e.enclosure_url, EXISTS \
               (SELECT 1 FROM media_files m WHERE m.episode_id = e.id AND m.status = 'ok') \
             FROM episodes e WHERE e.podcast_id = ?",
            )
            .bind(podcast_id)
            .fetch_all(self.pool())
            .await?
            .into_iter()
            .map(|(id, title, dir, enclosure_url, has_media)| Candidate {
                id,
                title,
                dir,
                enclosure_url,
                has_media,
            })
            .collect();

        let mut report = ImportReport::default();
        for file in files {
            let shown = file
                .strip_prefix(dir)
                .unwrap_or(&file)
                .display()
                .to_string();
            let (Some(name), Some(stem), Some(ext)) = (
                file.file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_lowercase),
                file.file_stem()
                    .and_then(|n| n.to_str())
                    .map(str::to_string),
                extension(&file),
            ) else {
                report.skipped.push((shown, "unreadable file name".into()));
                continue;
            };
            let (hash, size) = {
                let file = file.clone();
                tokio::task::spawn_blocking(move || hash_file(&file)).await??
            };
            let known: Option<i64> = sqlx::query_scalar(
                "SELECT m.id FROM media_files m JOIN episodes e ON e.id = m.episode_id \
                 WHERE e.podcast_id = ? AND m.blake3 = ? AND m.status = 'ok'",
            )
            .bind(podcast_id)
            .bind(&hash)
            .fetch_optional(self.pool())
            .await?;
            if known.is_some() {
                report.known.push(shown);
                continue;
            }

            // By the name in the media address first: it is exact. Titles only if exactly
            // one episode fits.
            let by_name: Vec<usize> = candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.enclosure_url
                        .as_deref()
                        .and_then(url_file_name)
                        .as_deref()
                        == Some(&name)
                })
                .map(|(index, _)| index)
                .collect();
            let stem_slug = slugify(&stem, 200);
            let by_title: Vec<usize> = candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    let title = slugify(&c.title, 200);
                    title.len() >= 6 && (stem_slug == title || stem_slug.contains(&title))
                })
                .map(|(index, _)| index)
                .collect();
            let matched = match (by_name.as_slice(), by_title.as_slice()) {
                ([index], _) => Some(*index),
                ([], [index]) => Some(*index),
                _ => None,
            };

            let mime = mime_from_extension(&ext).unwrap_or("application/octet-stream");
            let episode_id = match matched {
                Some(index) if candidates[index].has_media => {
                    let reason = format!(
                        "\"{}\" already has a different file",
                        candidates[index].title
                    );
                    report.skipped.push((shown, reason));
                    continue;
                }
                Some(index) => {
                    candidates[index].has_media = true;
                    report
                        .matched
                        .push((shown, candidates[index].title.clone()));
                    candidates[index].id
                }
                None if !keep_unmatched => {
                    let reason = if by_name.len() > 1 || by_title.len() > 1 {
                        "fits more than one episode"
                    } else {
                        "fits no episode of the feed"
                    };
                    report.skipped.push((shown, reason.into()));
                    continue;
                }
                None => {
                    // An episode the feed no longer lists. The file's date is the best guess
                    // for when it was published.
                    let published = tokio::fs::metadata(&file)
                        .await
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64);
                    let guid = format!("imported:{hash}");
                    let taken: Vec<String> =
                        sqlx::query_scalar("SELECT dir FROM episodes WHERE podcast_id = ?")
                            .bind(podcast_id)
                            .fetch_all(self.pool())
                            .await?;
                    let episode_dir = crate::util::unique_name(
                        &crate::archive::episode_name(published, &stem),
                        |name| {
                            taken
                                .iter()
                                .any(|t| t.to_lowercase() == name.to_lowercase())
                        },
                    );
                    let now = now();
                    let id: i64 = sqlx::query_scalar(
                        "INSERT INTO episodes (podcast_id, guid, title, published_at, dir, \
                         meta_hash, first_seen_at, last_seen_at, removed_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                    )
                    .bind(podcast_id)
                    .bind(&guid)
                    .bind(&stem)
                    .bind(published)
                    .bind(&episode_dir)
                    .bind(&hash)
                    .bind(now)
                    .bind(now)
                    .bind(now)
                    .fetch_one(self.pool())
                    .await?;
                    candidates.push(Candidate {
                        id,
                        title: stem.clone(),
                        dir: episode_dir,
                        enclosure_url: None,
                        has_media: true,
                    });
                    report.added.push(shown);
                    id
                }
            };
            let episode = candidates
                .iter()
                .find(|c| c.id == episode_id)
                .context("episode vanished")?;

            // Copy, then check the copy, then let it count.
            let metadata_dir =
                self.archive_path(&crate::archive::metadata_dir(&podcast.dir, &episode.dir));
            tokio::fs::create_dir_all(&metadata_dir).await?;
            let relative = crate::archive::media_path(&podcast.dir, &episode.dir, &ext);
            let target = self.archive_path(&relative);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let part = metadata_dir.join("import.part");
            tokio::fs::copy(&file, &part)
                .await
                .with_context(|| format!("cannot copy {}", file.display()))?;
            let (copied, _) = {
                let part = part.clone();
                tokio::task::spawn_blocking(move || hash_file(&part)).await??
            };
            if copied != hash {
                let _ = tokio::fs::remove_file(&part).await;
                anyhow::bail!("{} changed while it was copied", file.display());
            }
            tokio::fs::rename(&part, &target).await?;
            self.write_checksum(&relative, &hash).await?;
            let now = now();
            sqlx::query(
                "INSERT INTO media_files (episode_id, path, size, blake3, mime, source_url, \
                 downloaded_at, verified_at, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'ok') \
                 ON CONFLICT (path) DO UPDATE SET size = excluded.size, blake3 = excluded.blake3, \
                 mime = excluded.mime, source_url = excluded.source_url, \
                 downloaded_at = excluded.downloaded_at, verified_at = excluded.verified_at, \
                 status = 'ok'",
            )
            .bind(episode_id)
            .bind(&relative)
            .bind(size as i64)
            .bind(&hash)
            .bind(mime)
            .bind(format!("imported:{name}"))
            .bind(now)
            .bind(now)
            .execute(self.pool())
            .await?;
            self.write_episode_json(episode_id).await?;
        }
        if !report.added.is_empty() {
            self.index_podcast_text(podcast_id).await?;
        }
        Ok(report)
    }
}
