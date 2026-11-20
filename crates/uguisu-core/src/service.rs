//! The archive service: adding podcasts, refreshing feeds, downloading media, verifying files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sqlx::{FromRow, SqlitePool};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::archive::{self, write_atomic};
use crate::feed::{self, ParsedFeed, ParsedItem};
use crate::http::{build_client, check_url, parse_user_url};
use crate::model::{Episode, EpisodeView, Podcast, PodcastSummary, Stats};
use crate::util::{now, random_token, short_hash, slugify};
use crate::{db, search};

const MAX_FEED_BYTES: usize = 64 * 1024 * 1024;
const MAX_COVER_BYTES: usize = 32 * 1024 * 1024;
const MAX_DOWNLOAD_ATTEMPTS: i64 = 5;

const EPISODE_VIEW_SQL: &str = "SELECT e.*, m.path AS media_path, m.size AS media_size, \
     m.mime AS media_mime, m.blake3 AS media_blake3 \
     FROM episodes e LEFT JOIN media_files m ON m.id = \
     (SELECT MAX(id) FROM media_files WHERE episode_id = e.id AND status = 'ok')";

const PODCAST_SUMMARY_SQL: &str = "SELECT p.*, \
     (SELECT COUNT(*) FROM episodes e WHERE e.podcast_id = p.id) AS episode_count, \
     (SELECT COUNT(DISTINCT m.episode_id) FROM media_files m JOIN episodes e ON e.id = m.episode_id \
       WHERE e.podcast_id = p.id AND m.status = 'ok') AS archived_count, \
     (SELECT COALESCE(SUM(m.size), 0) FROM media_files m JOIN episodes e ON e.id = m.episode_id \
       WHERE e.podcast_id = p.id AND m.status = 'ok') AS archived_bytes \
     FROM podcasts p";

#[derive(Debug, Clone)]
pub struct Config {
    /// Holds `uguisu.db` and the `archive/` directory.
    pub data_dir: PathBuf,
    /// Allow fetching from loopback and private addresses. Off by default.
    pub allow_private_networks: bool,
    pub user_agent: String,
    /// How often each feed is checked for changes.
    pub refresh_interval: Duration,
    pub download_concurrency: usize,
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            allow_private_networks: false,
            user_agent: concat!("Uguisu/", env!("CARGO_PKG_VERSION"), " (podcast archiver)")
                .to_string(),
            refresh_interval: Duration::from_secs(3600),
            download_concurrency: 2,
        }
    }
}

#[derive(Debug, Default)]
pub struct VerifyReport {
    pub checked: u64,
    pub ok: u64,
    pub corrupt: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ImportReport {
    pub added: usize,
    pub failed: usize,
}

enum Fetched {
    NotModified,
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

#[derive(FromRow)]
struct PendingDownload {
    #[sqlx(flatten)]
    episode: Episode,
    podcast_dir: String,
}

struct Inner {
    cfg: Config,
    pool: SqlitePool,
    client: reqwest::Client,
    wake_downloader: Notify,
    downloading: Mutex<HashSet<i64>>,
}

#[derive(Clone)]
pub struct Uguisu {
    inner: Arc<Inner>,
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn iso_date(timestamp: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(timestamp, 0).map(|d| d.to_rfc3339())
}

fn episode_dir_name(item: &ParsedItem) -> String {
    let date = item
        .published_at
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "undated".to_string());
    format!(
        "{date}-{}-{}",
        slugify(&item.title, 40),
        short_hash(&item.guid)
    )
}

impl Uguisu {
    pub async fn open(cfg: Config) -> Result<Self> {
        let archive_dir = cfg.data_dir.join("archive");
        tokio::fs::create_dir_all(&archive_dir)
            .await
            .with_context(|| format!("cannot create {}", archive_dir.display()))?;
        let pool = db::open(&cfg.data_dir.join("uguisu.db")).await?;
        let client = build_client(&cfg.user_agent, cfg.allow_private_networks)?;
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                pool,
                client,
                wake_downloader: Notify::new(),
                downloading: Mutex::new(HashSet::new()),
            }),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.inner.pool
    }

    pub fn config(&self) -> &Config {
        &self.inner.cfg
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.inner.cfg.data_dir.join("archive")
    }

    /// Absolute path of a file referenced relative to the archive directory.
    pub fn archive_path(&self, relative: &str) -> PathBuf {
        self.archive_dir().join(relative)
    }

    // ── Queries ────────────────────────────────────────────────────────────────────────────

    pub async fn podcasts(&self) -> Result<Vec<PodcastSummary>> {
        let sql = format!("{PODCAST_SUMMARY_SQL} ORDER BY p.title COLLATE NOCASE");
        Ok(sqlx::query_as(&sql).fetch_all(self.pool()).await?)
    }

    pub async fn podcast_summary(&self, id: i64) -> Result<Option<PodcastSummary>> {
        let sql = format!("{PODCAST_SUMMARY_SQL} WHERE p.id = ?");
        Ok(sqlx::query_as(&sql)
            .bind(id)
            .fetch_optional(self.pool())
            .await?)
    }

    pub async fn podcast(&self, id: i64) -> Result<Option<Podcast>> {
        Ok(sqlx::query_as("SELECT * FROM podcasts WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?)
    }

    pub async fn podcast_by_token(&self, token: &str) -> Result<Option<Podcast>> {
        Ok(
            sqlx::query_as("SELECT * FROM podcasts WHERE feed_token = ?")
                .bind(token)
                .fetch_optional(self.pool())
                .await?,
        )
    }

    /// Episodes of a podcast, newest first.
    pub async fn episodes(&self, podcast_id: i64) -> Result<Vec<EpisodeView>> {
        let sql = format!(
            "{EPISODE_VIEW_SQL} WHERE e.podcast_id = ? \
             ORDER BY e.published_at IS NULL, e.published_at DESC, e.id DESC"
        );
        Ok(sqlx::query_as(&sql)
            .bind(podcast_id)
            .fetch_all(self.pool())
            .await?)
    }

    pub async fn episode(&self, podcast_id: i64, episode_id: i64) -> Result<Option<EpisodeView>> {
        let sql = format!("{EPISODE_VIEW_SQL} WHERE e.podcast_id = ? AND e.id = ?");
        Ok(sqlx::query_as(&sql)
            .bind(podcast_id)
            .bind(episode_id)
            .fetch_optional(self.pool())
            .await?)
    }

    pub async fn stats(&self) -> Result<Stats> {
        Ok(sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM podcasts) AS podcasts, \
             (SELECT COUNT(*) FROM episodes) AS episodes, \
             (SELECT COUNT(DISTINCT episode_id) FROM media_files WHERE status = 'ok') AS archived, \
             (SELECT COALESCE(SUM(size), 0) FROM media_files WHERE status = 'ok') AS bytes",
        )
        .fetch_one(self.pool())
        .await?)
    }

    /// Ids of the episodes that are being downloaded right now.
    pub fn downloading(&self) -> HashSet<i64> {
        self.inner.downloading.lock().unwrap().clone()
    }

    pub async fn search(&self, query: &str) -> Result<Vec<search::SearchHit>> {
        search::search(&self.inner.client, query).await
    }

    // ── Adding and refreshing ──────────────────────────────────────────────────────────────

    /// Adds a podcast by feed URL and returns its id. Adding a podcast that is already archived
    /// (same URL or same `<podcast:guid>`) returns the existing id.
    pub async fn add_podcast(&self, input: &str) -> Result<i64> {
        let url = parse_user_url(input, self.inner.cfg.allow_private_networks)?;
        let url = url.as_str();
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM podcasts WHERE feed_url = ? OR original_feed_url = ?",
        )
        .bind(url)
        .bind(url)
        .fetch_optional(self.pool())
        .await?;
        if let Some(id) = existing {
            return Ok(id);
        }

        let Fetched::Body {
            bytes,
            etag,
            last_modified,
        } = self.fetch_feed(url, None, None).await?
        else {
            bail!("the server answered 'not modified' to an unconditional request");
        };
        let parsed = feed::parse(&bytes)?;

        if let Some(guid) = &parsed.podcast_guid {
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT id FROM podcasts WHERE podcast_guid = ?")
                    .bind(guid)
                    .fetch_optional(self.pool())
                    .await?;
            if let Some(id) = existing {
                return Ok(id);
            }
        }

        let dir = format!("{}-{}", slugify(&parsed.title, 60), short_hash(url));
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO podcasts (feed_url, original_feed_url, title, dir, feed_token, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(url)
        .bind(url)
        .bind(&parsed.title)
        .bind(&dir)
        .bind(random_token(24))
        .bind(now())
        .fetch_one(self.pool())
        .await?;

        self.ingest(id, &parsed, &bytes, etag, last_modified)
            .await?;
        tracing::info!(podcast = %parsed.title, episodes = parsed.items.len(), "podcast added");
        Ok(id)
    }

    /// Adds every URL of an OPML import. Failures are logged and counted, not fatal.
    pub async fn import_feeds(&self, urls: &[String]) -> ImportReport {
        let mut report = ImportReport::default();
        for url in urls {
            match self.add_podcast(url).await {
                Ok(_) => report.added += 1,
                Err(e) => {
                    report.failed += 1;
                    tracing::warn!("import of {url} failed: {e:#}");
                }
            }
        }
        report
    }

    /// Fetches the source feed again and records what changed. Errors are also stored on the
    /// podcast so the UI can show them.
    pub async fn refresh(&self, podcast_id: i64) -> Result<()> {
        let result = self.refresh_inner(podcast_id).await;
        if let Err(e) = &result {
            sqlx::query(
                "UPDATE podcasts SET last_fetched_at = ?, last_fetch_error = ? WHERE id = ?",
            )
            .bind(now())
            .bind(format!("{e:#}"))
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        }
        result
    }

    async fn refresh_inner(&self, podcast_id: i64) -> Result<()> {
        let podcast = self
            .podcast(podcast_id)
            .await?
            .context("podcast not found")?;
        let fetched = self
            .fetch_feed(
                &podcast.feed_url,
                podcast.etag.as_deref(),
                podcast.last_modified.as_deref(),
            )
            .await?;
        match fetched {
            Fetched::NotModified => {
                sqlx::query(
                    "UPDATE podcasts SET last_fetched_at = ?, last_fetch_error = NULL WHERE id = ?",
                )
                .bind(now())
                .bind(podcast_id)
                .execute(self.pool())
                .await?;
                self.inner.wake_downloader.notify_one();
                Ok(())
            }
            Fetched::Body {
                bytes,
                etag,
                last_modified,
            } => {
                let parsed = feed::parse(&bytes)?;
                self.ingest(podcast_id, &parsed, &bytes, etag, last_modified)
                    .await
            }
        }
    }

    async fn fetch_feed(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<Fetched> {
        let mut request = self
            .inner
            .client
            .get(url)
            .header(
                reqwest::header::ACCEPT,
                "application/rss+xml, application/xml;q=0.9, */*;q=0.5",
            )
            .timeout(Duration::from_secs(120));
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = last_modified {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
        }
        let mut response = request
            .send()
            .await
            .with_context(|| format!("cannot fetch {url}"))?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        if !response.status().is_success() {
            bail!("the feed server answered {} for {url}", response.status());
        }
        let etag = header(&response, reqwest::header::ETAG);
        let last_modified = header(&response, reqwest::header::LAST_MODIFIED);
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("feed download interrupted")?
        {
            if bytes.len() + chunk.len() > MAX_FEED_BYTES {
                bail!("the feed is larger than {} MiB", MAX_FEED_BYTES >> 20);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Fetched::Body {
            bytes,
            etag,
            last_modified,
        })
    }

    /// Stores a fetched feed: snapshot on disk, podcast metadata, episodes and their revisions.
    async fn ingest(
        &self,
        podcast_id: i64,
        parsed: &ParsedFeed,
        bytes: &[u8],
        etag: Option<String>,
        last_modified: Option<String>,
    ) -> Result<()> {
        let podcast = self
            .podcast(podcast_id)
            .await?
            .context("podcast not found")?;
        let now = now();
        let podcast_dir = self.archive_path(&podcast.dir);

        // 1. Keep the feed exactly as served, once per distinct content.
        let hash = blake3::hash(bytes).to_hex().to_string();
        let known: Option<i64> =
            sqlx::query_scalar("SELECT id FROM feed_snapshots WHERE podcast_id = ? AND blake3 = ?")
                .bind(podcast_id)
                .bind(&hash)
                .fetch_optional(self.pool())
                .await?;
        if known.is_none() {
            let stamp = chrono::DateTime::from_timestamp(now, 0)
                .map(|d| d.format("%Y%m%dT%H%M%SZ").to_string())
                .unwrap_or_else(|| now.to_string());
            let relative = format!("{}/feeds/{stamp}-{}.xml", podcast.dir, &hash[..8]);
            write_atomic(&self.archive_path(&relative), bytes).await?;
            sqlx::query(
                "INSERT INTO feed_snapshots (podcast_id, fetched_at, blake3, size, path) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(podcast_id)
            .bind(now)
            .bind(&hash)
            .bind(bytes.len() as i64)
            .bind(&relative)
            .execute(self.pool())
            .await?;
        }

        // 2. Follow an announced feed move, unless the target is unusable or already archived.
        let mut feed_url = podcast.feed_url.clone();
        let mut moved = false;
        if let Some(target) = &parsed.new_feed_url
            && let Ok(target) = parse_user_url(target, self.inner.cfg.allow_private_networks)
            && target.as_str() != podcast.feed_url
        {
            let taken: Option<i64> =
                sqlx::query_scalar("SELECT id FROM podcasts WHERE feed_url = ?")
                    .bind(target.as_str())
                    .fetch_optional(self.pool())
                    .await?;
            if taken.is_none() {
                tracing::info!(podcast = %podcast.title, from = %podcast.feed_url, to = %target, "feed moved");
                feed_url = target.to_string();
                moved = true;
            }
        }

        // 3. Podcast metadata. After a move the cache validators belong to the old URL.
        sqlx::query(
            "UPDATE podcasts SET feed_url = ?, podcast_guid = COALESCE(?, podcast_guid), title = ?, \
             author = ?, description = ?, link = ?, image_url = ?, language = ?, etag = ?, \
             last_modified = ?, last_fetched_at = ?, last_fetch_error = NULL WHERE id = ?",
        )
        .bind(&feed_url)
        .bind(&parsed.podcast_guid)
        .bind(&parsed.title)
        .bind(&parsed.author)
        .bind(&parsed.description)
        .bind(&parsed.link)
        .bind(&parsed.image_url)
        .bind(&parsed.language)
        .bind(if moved { None } else { etag })
        .bind(if moved { None } else { last_modified })
        .bind(now)
        .bind(podcast_id)
        .execute(self.pool())
        .await?;

        // 4. Episodes.
        let mut changed: Vec<(String, &ParsedItem)> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::with_capacity(parsed.items.len());
        let mut tx = self.pool().begin().await?;
        for item in &parsed.items {
            let meta_hash = item.meta_hash();
            let existing: Option<(i64, String, String)> = sqlx::query_as(
                "SELECT id, meta_hash, dir FROM episodes WHERE podcast_id = ? AND guid = ?",
            )
            .bind(podcast_id)
            .bind(&item.guid)
            .fetch_optional(&mut *tx)
            .await?;

            let (episode_id, dir, is_new_revision) = match existing {
                None => {
                    let dir = episode_dir_name(item);
                    let id: i64 = sqlx::query_scalar(
                        "INSERT INTO episodes (podcast_id, guid, title, description, link, \
                         published_at, duration, enclosure_url, enclosure_type, enclosure_length, \
                         image_url, season, episode_no, dir, meta_hash, first_seen_at, last_seen_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                    )
                    .bind(podcast_id)
                    .bind(&item.guid)
                    .bind(&item.title)
                    .bind(&item.description)
                    .bind(&item.link)
                    .bind(item.published_at)
                    .bind(item.duration)
                    .bind(&item.enclosure_url)
                    .bind(&item.enclosure_type)
                    .bind(item.enclosure_length)
                    .bind(&item.image_url)
                    .bind(item.season)
                    .bind(item.episode_no)
                    .bind(&dir)
                    .bind(&meta_hash)
                    .bind(now)
                    .bind(now)
                    .fetch_one(&mut *tx)
                    .await?;
                    (id, dir, true)
                }
                Some((id, old_hash, dir)) => {
                    let is_changed = old_hash != meta_hash;
                    if is_changed {
                        // A changed episode gets a fresh chance if its download had given up.
                        sqlx::query(
                            "UPDATE episodes SET title = ?, description = ?, link = ?, \
                             published_at = ?, duration = ?, enclosure_url = ?, enclosure_type = ?, \
                             enclosure_length = ?, image_url = ?, season = ?, episode_no = ?, \
                             meta_hash = ?, last_seen_at = ?, removed_at = NULL, \
                             download_attempts = 0, next_attempt_at = 0 WHERE id = ?",
                        )
                        .bind(&item.title)
                        .bind(&item.description)
                        .bind(&item.link)
                        .bind(item.published_at)
                        .bind(item.duration)
                        .bind(&item.enclosure_url)
                        .bind(&item.enclosure_type)
                        .bind(item.enclosure_length)
                        .bind(&item.image_url)
                        .bind(item.season)
                        .bind(item.episode_no)
                        .bind(&meta_hash)
                        .bind(now)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    } else {
                        sqlx::query(
                            "UPDATE episodes SET last_seen_at = ?, removed_at = NULL WHERE id = ?",
                        )
                        .bind(now)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    }
                    (id, dir, is_changed)
                }
            };

            seen.insert(episode_id);
            if is_new_revision {
                sqlx::query(
                    "INSERT OR IGNORE INTO episode_revisions (episode_id, seen_at, meta_hash, raw_xml) \
                     VALUES (?, ?, ?, ?)",
                )
                .bind(episode_id)
                .bind(now)
                .bind(&meta_hash)
                .bind(&item.raw_xml)
                .execute(&mut *tx)
                .await?;
                changed.push((dir, item));
            }
        }
        // Episodes that were not in this feed have left it. An empty feed is more likely broken
        // than emptied, so it marks nothing.
        if !parsed.items.is_empty() {
            let present: Vec<i64> = sqlx::query_scalar(
                "SELECT id FROM episodes WHERE podcast_id = ? AND removed_at IS NULL",
            )
            .bind(podcast_id)
            .fetch_all(&mut *tx)
            .await?;
            for id in present.into_iter().filter(|id| !seen.contains(id)) {
                sqlx::query("UPDATE episodes SET removed_at = ? WHERE id = ?")
                    .bind(now)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;

        // 5. Sidecar files, so the archive explains itself without the database.
        for (dir, item) in changed {
            let episode_dir = podcast_dir.join("episodes").join(dir);
            let json = serde_json::json!({
                "guid": item.guid,
                "title": item.title,
                "description": item.description,
                "link": item.link,
                "published": item.published_at.and_then(iso_date),
                "duration_seconds": item.duration,
                "enclosure": {
                    "url": item.enclosure_url,
                    "type": item.enclosure_type,
                    "length": item.enclosure_length,
                },
                "image": item.image_url,
                "season": item.season,
                "episode": item.episode_no,
            });
            write_atomic(
                &episode_dir.join("episode.json"),
                &serde_json::to_vec_pretty(&json)?,
            )
            .await?;
            write_atomic(&episode_dir.join("item.xml"), item.raw_xml.as_bytes()).await?;
        }
        let json = serde_json::json!({
            "title": parsed.title,
            "author": parsed.author,
            "description": parsed.description,
            "link": parsed.link,
            "language": parsed.language,
            "image": parsed.image_url,
            "podcast_guid": parsed.podcast_guid.as_ref().or(podcast.podcast_guid.as_ref()),
            "feed_url": feed_url,
            "original_feed_url": podcast.original_feed_url,
            "last_fetched": iso_date(now),
        });
        write_atomic(
            &podcast_dir.join("podcast.json"),
            &serde_json::to_vec_pretty(&json)?,
        )
        .await?;

        // 6. Cover art, best effort.
        if let Some(image_url) = &parsed.image_url
            && (podcast.cover_file.is_none() || podcast.image_url.as_ref() != Some(image_url))
            && let Err(e) = self.fetch_cover(podcast_id, &podcast_dir, image_url).await
        {
            tracing::warn!(podcast = %parsed.title, "cover download failed: {e:#}");
        }

        self.inner.wake_downloader.notify_one();
        Ok(())
    }

    async fn fetch_cover(
        &self,
        podcast_id: i64,
        podcast_dir: &Path,
        image_url: &str,
    ) -> Result<()> {
        let url = url::Url::parse(image_url).context("invalid cover URL")?;
        check_url(&url, self.inner.cfg.allow_private_networks)?;
        let mut response = self
            .inner
            .client
            .get(url)
            .timeout(Duration::from_secs(120))
            .send()
            .await?
            .error_for_status()?;
        let content_type = header(&response, reqwest::header::CONTENT_TYPE).unwrap_or_default();
        let ext = archive::extension_from_mime(&content_type)
            .or_else(|| archive::extension_from_url(image_url))
            .filter(|e| matches!(*e, "jpg" | "jpeg" | "png" | "webp" | "gif"))
            .unwrap_or("jpg");
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > MAX_COVER_BYTES {
                bail!("cover image is too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            bail!("cover image is empty");
        }
        let file_name = format!("cover.{ext}");
        write_atomic(&podcast_dir.join(&file_name), &bytes).await?;
        sqlx::query("UPDATE podcasts SET cover_file = ? WHERE id = ?")
            .bind(&file_name)
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_include_removed(&self, podcast_id: i64, include: bool) -> Result<()> {
        sqlx::query("UPDATE podcasts SET include_removed = ? WHERE id = ?")
            .bind(include)
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Replaces the secret in the published feed URL. The old URL stops working immediately.
    pub async fn regenerate_token(&self, podcast_id: i64) -> Result<()> {
        sqlx::query("UPDATE podcasts SET feed_token = ? WHERE id = ?")
            .bind(random_token(24))
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // ── Downloads ──────────────────────────────────────────────────────────────────────────

    async fn pending_downloads(&self) -> Result<Vec<PendingDownload>> {
        Ok(sqlx::query_as(
            "SELECT e.*, p.dir AS podcast_dir FROM episodes e JOIN podcasts p ON p.id = e.podcast_id \
             WHERE e.enclosure_url IS NOT NULL AND e.download_attempts < ? AND e.next_attempt_at <= ? \
             AND NOT EXISTS (SELECT 1 FROM media_files m WHERE m.episode_id = e.id AND m.status = 'ok') \
             ORDER BY e.published_at DESC LIMIT 50",
        )
        .bind(MAX_DOWNLOAD_ATTEMPTS)
        .bind(now())
        .fetch_all(self.pool())
        .await?)
    }

    /// Downloads everything that is currently due. Returns the number of attempted episodes.
    pub async fn download_pending(&self) -> Result<usize> {
        let mut attempted = 0;
        loop {
            let pending = self.pending_downloads().await?;
            if pending.is_empty() {
                return Ok(attempted);
            }
            attempted += pending.len();
            futures_util::stream::iter(pending)
                .for_each_concurrent(self.inner.cfg.download_concurrency.max(1), |p| async move {
                    self.download_one(p).await;
                })
                .await;
        }
    }

    async fn download_one(&self, pending: PendingDownload) {
        let episode = &pending.episode;
        self.inner.downloading.lock().unwrap().insert(episode.id);
        let result = self.download(episode, &pending.podcast_dir).await;
        self.inner.downloading.lock().unwrap().remove(&episode.id);

        let outcome = match result {
            Ok(()) => {
                tracing::info!(episode = %episode.title, "archived");
                sqlx::query("UPDATE episodes SET download_error = NULL WHERE id = ?")
                    .bind(episode.id)
                    .execute(self.pool())
                    .await
            }
            Err(e) => {
                tracing::warn!(episode = %episode.title, "download failed: {e:#}");
                // Back off: 5 min, 20 min, 80 min, ...
                let delay = 300 * 4i64.pow(episode.download_attempts.clamp(0, 6) as u32);
                sqlx::query(
                    "UPDATE episodes SET download_attempts = download_attempts + 1, \
                     download_error = ?, next_attempt_at = ? WHERE id = ?",
                )
                .bind(format!("{e:#}"))
                .bind(now() + delay)
                .bind(episode.id)
                .execute(self.pool())
                .await
            }
        };
        if let Err(e) = outcome {
            tracing::error!("cannot record download result: {e:#}");
        }
    }

    async fn download(&self, episode: &Episode, podcast_dir: &str) -> Result<()> {
        let source = episode
            .enclosure_url
            .as_deref()
            .context("episode has no enclosure")?;
        let url = url::Url::parse(source).context("invalid enclosure URL")?;
        check_url(&url, self.inner.cfg.allow_private_networks)?;

        let mut response = self.inner.client.get(url).send().await?;
        if !response.status().is_success() {
            bail!("the media server answered {}", response.status());
        }
        let content_type = header(&response, reqwest::header::CONTENT_TYPE)
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        if content_type == "text/html" {
            bail!("the media server returned a web page instead of a media file");
        }
        let declared = episode
            .enclosure_type
            .as_deref()
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| t.contains('/'));
        let ext = archive::extension_from_url(source)
            .or_else(|| declared.as_deref().and_then(archive::extension_from_mime))
            .or_else(|| archive::extension_from_mime(&content_type))
            .unwrap_or("bin");
        let mime = declared
            .or_else(|| archive::mime_from_extension(ext).map(str::to_string))
            .or_else(|| (!content_type.is_empty()).then(|| content_type.clone()))
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let file_name = format!("media.{ext}");
        let relative_dir = format!("{podcast_dir}/episodes/{}", episode.dir);
        let dir = self.archive_path(&relative_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let part = dir.join(format!("{file_name}.part"));
        let target = dir.join(&file_name);

        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = blake3::Hasher::new();
        let mut size = 0u64;
        let streamed: Result<()> = async {
            while let Some(chunk) = response.chunk().await.context("download interrupted")? {
                hasher.update(&chunk);
                file.write_all(&chunk).await?;
                size += chunk.len() as u64;
            }
            if size == 0 {
                bail!("the media server sent an empty file");
            }
            file.sync_all().await?;
            Ok(())
        }
        .await;
        drop(file);
        if let Err(e) = streamed {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        tokio::fs::rename(&part, &target).await?;

        let hash = hasher.finalize().to_hex().to_string();
        // Same format as `b3sum`, so the file can be checked with `b3sum -c` without Uguisu.
        write_atomic(
            &dir.join(format!("{file_name}.blake3")),
            format!("{hash}  {file_name}\n").as_bytes(),
        )
        .await?;

        let now = now();
        sqlx::query(
            "INSERT INTO media_files (episode_id, path, size, blake3, mime, source_url, \
             downloaded_at, verified_at, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'ok') \
             ON CONFLICT (path) DO UPDATE SET size = excluded.size, blake3 = excluded.blake3, \
             mime = excluded.mime, source_url = excluded.source_url, \
             downloaded_at = excluded.downloaded_at, verified_at = excluded.verified_at, status = 'ok'",
        )
        .bind(episode.id)
        .bind(format!("{relative_dir}/{file_name}"))
        .bind(size as i64)
        .bind(&hash)
        .bind(&mime)
        .bind(source)
        .bind(now)
        .bind(now)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    // ── Background work ────────────────────────────────────────────────────────────────────

    /// Refreshes every podcast whose last check is older than the configured interval.
    pub async fn refresh_due(&self) -> Result<()> {
        let cutoff = now() - self.inner.cfg.refresh_interval.as_secs() as i64;
        let due: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM podcasts WHERE last_fetched_at IS NULL OR last_fetched_at <= ? \
             ORDER BY last_fetched_at",
        )
        .bind(cutoff)
        .fetch_all(self.pool())
        .await?;
        for id in due {
            if let Err(e) = self.refresh(id).await {
                tracing::warn!(podcast_id = id, "refresh failed: {e:#}");
            }
        }
        Ok(())
    }

    /// Starts the feed refresher and the downloader. Both run until the process exits.
    pub fn spawn_workers(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = this.refresh_due().await {
                    tracing::error!("refresh cycle failed: {e:#}");
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = this.download_pending().await {
                    tracing::error!("download cycle failed: {e:#}");
                }
                tokio::select! {
                    _ = this.inner.wake_downloader.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                }
            }
        });
    }

    // ── Fixity ─────────────────────────────────────────────────────────────────────────────

    /// Re-hashes every archived media file and compares it with the hash recorded at download.
    /// Damaged or missing files are flagged, which also queues them for a new download.
    pub async fn verify(&self) -> Result<VerifyReport> {
        let files: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, path, blake3 FROM media_files ORDER BY path")
                .fetch_all(self.pool())
                .await?;
        let mut report = VerifyReport::default();
        for (id, relative, expected) in files {
            report.checked += 1;
            let path = self.archive_path(&relative);
            let hashed = tokio::task::spawn_blocking(move || archive::hash_file(&path)).await?;
            let status = match hashed {
                Ok((actual, _)) if actual == expected => {
                    report.ok += 1;
                    "ok"
                }
                Ok(_) => {
                    report.corrupt.push(relative);
                    "corrupt"
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    report.missing.push(relative);
                    "missing"
                }
                Err(e) => return Err(e).with_context(|| format!("cannot read {relative}")),
            };
            if status == "ok" {
                sqlx::query("UPDATE media_files SET status = 'ok', verified_at = ? WHERE id = ?")
                    .bind(now())
                    .bind(id)
                    .execute(self.pool())
                    .await?;
            } else {
                sqlx::query("UPDATE media_files SET status = ? WHERE id = ?")
                    .bind(status)
                    .bind(id)
                    .execute(self.pool())
                    .await?;
            }
        }
        if !report.corrupt.is_empty() || !report.missing.is_empty() {
            self.inner.wake_downloader.notify_one();
        }
        Ok(report)
    }
}
