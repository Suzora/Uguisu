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
use crate::feed::{self, ParsedFeed, ParsedItem, without_query};
use crate::http::{build_client, check_url, parse_user_url};
use crate::model::{Attachment, Episode, EpisodeView, FeedEpisode, Podcast, PodcastSummary, Stats};
use crate::util::{now, random_token, short_hash, slugify};
use crate::{db, search};

const MAX_FEED_BYTES: usize = 64 * 1024 * 1024;
/// Upper bound for the pages of one feed (RFC 5005); far beyond what real feeds use.
const MAX_FEED_PAGES: usize = 500;
/// How many of the feeds a web page announces are tried to find the podcast feed.
const MAX_ANNOUNCED_FEEDS: usize = 8;
const MAX_COVER_BYTES: usize = 32 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOWNLOAD_ATTEMPTS: i64 = 5;

/// All podcasts with their numbers. Unlike `podcast_summary_sql`, which looks the numbers up per
/// podcast and suits a single one, this counts every podcast in one pass over the episodes; with
/// tens of thousands of episodes the per-podcast form took seconds for the library.
const PODCAST_LIST_SQL: &str = "SELECT p.*, \
     COALESCE(ec.episode_count, 0) AS episode_count, \
     COALESCE(mc.archived_count, 0) AS archived_count, \
     COALESCE(mc.archived_bytes, 0) AS archived_bytes, \
     COALESCE(ec.removed_count, 0) AS removed_count, \
     COALESCE(ec.pending_count, 0) AS pending_count \
     FROM podcasts p \
     LEFT JOIN (SELECT e.podcast_id, COUNT(*) AS episode_count, \
         SUM(e.removed_at IS NOT NULL) AS removed_count, \
         SUM(e.enclosure_url IS NOT NULL AND e.download_attempts < 5 AND (e.refetch = 1 OR NOT EXISTS \
           (SELECT 1 FROM media_files m WHERE m.episode_id = e.id AND m.status = 'ok'))) \
           AS pending_count \
       FROM episodes e GROUP BY e.podcast_id) ec ON ec.podcast_id = p.id \
     LEFT JOIN (SELECT e.podcast_id, COUNT(DISTINCT m.episode_id) AS archived_count, \
         SUM(m.size) AS archived_bytes \
       FROM media_files m JOIN episodes e ON e.id = m.episode_id \
       WHERE m.status = 'ok' GROUP BY e.podcast_id) mc ON mc.podcast_id = p.id \
     ORDER BY p.title COLLATE NOCASE";

/// Number of episodes the interface shows at a time.
pub const EPISODES_PER_PAGE: i64 = 100;

// A macro rather than a constant so that the complete statement stays a string literal.
macro_rules! episode_view_sql {
    ($tail:literal) => {
        concat!(
            "SELECT e.*, m.path AS media_path, m.size AS media_size, \
     m.mime AS media_mime, m.blake3 AS media_blake3, \
     (SELECT COUNT(*) FROM media_files WHERE episode_id = e.id) AS media_versions, \
     (SELECT COUNT(*) FROM media_files WHERE episode_id = e.id AND status != 'ok') AS media_damaged, \
     (SELECT COUNT(*) FROM attachments WHERE episode_id = e.id AND status = 'ok') AS attachment_count \
     FROM episodes e LEFT JOIN media_files m ON m.id = \
     (SELECT MAX(id) FROM media_files WHERE episode_id = e.id AND status = 'ok')",
            " ",
            $tail
        )
    };
}

// A macro rather than a constant so that the complete statement stays a string literal.
macro_rules! podcast_summary_sql {
    ($tail:literal) => {
        concat!("SELECT p.*, \
     (SELECT COUNT(*) FROM episodes e WHERE e.podcast_id = p.id) AS episode_count, \
     (SELECT COUNT(DISTINCT m.episode_id) FROM media_files m JOIN episodes e ON e.id = m.episode_id \
       WHERE e.podcast_id = p.id AND m.status = 'ok') AS archived_count, \
     (SELECT COALESCE(SUM(m.size), 0) FROM media_files m JOIN episodes e ON e.id = m.episode_id \
       WHERE e.podcast_id = p.id AND m.status = 'ok') AS archived_bytes, \
     (SELECT COUNT(*) FROM episodes e WHERE e.podcast_id = p.id AND e.removed_at IS NOT NULL) \
       AS removed_count, \
     (SELECT COUNT(*) FROM episodes e WHERE e.podcast_id = p.id AND e.enclosure_url IS NOT NULL \
       AND e.download_attempts < 5 AND (e.refetch = 1 OR NOT EXISTS \
         (SELECT 1 FROM media_files m WHERE m.episode_id = e.id AND m.status = 'ok'))) \
       AS pending_count \
     FROM podcasts p", " ", $tail)
    };
}

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
    /// Every archived file is re-hashed at least this often. `None` disables the automatic check.
    pub verify_interval: Option<Duration>,
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
            verify_interval: Some(Duration::from_secs(30 * 24 * 3600)),
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

impl VerifyReport {
    fn merge(&mut self, other: VerifyReport) {
        self.checked += other.checked;
        self.ok += other.ok;
        self.corrupt.extend(other.corrupt);
        self.missing.extend(other.missing);
    }
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

/// The address did not lead to a feed but to a web page that announces these feeds.
#[derive(Debug)]
struct FeedElsewhere(Vec<reqwest::Url>);

impl std::fmt::Display for FeedElsewhere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "this address is a web page, not a podcast feed")
    }
}

impl std::error::Error for FeedElsewhere {}

/// A source feed with all of its pages.
struct FetchedFeed {
    /// The first page, with the episodes and namespaces of the further pages merged in.
    parsed: ParsedFeed,
    /// Every page exactly as served, the first one first.
    pages: Vec<Vec<u8>>,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(FromRow)]
struct PendingAttachment {
    id: i64,
    kind: String,
    source_key: String,
    source_url: String,
    attempts: i64,
    episode_dir: String,
    podcast_dir: String,
}

/// The two tables that hold archived files.
#[derive(Clone, Copy)]
enum Files {
    Media,
    Attachments,
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
    search_cache: Mutex<search::SearchCache>,
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
    crate::util::rfc3339(timestamp)
}

fn episode_dir_name(item: &ParsedItem) -> String {
    let date = item
        .published_at
        .and_then(|t| crate::util::format_utc(t, "%Y-%m-%d"))
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
                search_cache: Mutex::new(search::SearchCache::default()),
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
        Ok(sqlx::query_as(PODCAST_LIST_SQL)
            .fetch_all(self.pool())
            .await?)
    }

    pub async fn podcast_summary(&self, id: i64) -> Result<Option<PodcastSummary>> {
        Ok(sqlx::query_as(podcast_summary_sql!("WHERE p.id = ?"))
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
        Ok(sqlx::query_as(episode_view_sql!(
            "WHERE e.podcast_id = ? \
             ORDER BY e.published_at IS NULL, e.published_at DESC, e.id DESC"
        ))
        .bind(podcast_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Episodes of a podcast with the raw `<item>` of their current revision, newest first.
    pub async fn feed_episodes(&self, podcast_id: i64) -> Result<Vec<FeedEpisode>> {
        Ok(sqlx::query_as(concat!(
            "SELECT v.*, (SELECT r.raw_xml FROM episode_revisions r \
               WHERE r.episode_id = v.id AND r.meta_hash = v.meta_hash) AS raw_xml FROM (",
            episode_view_sql!(""),
            ") v WHERE v.podcast_id = ? \
             ORDER BY v.published_at IS NULL, v.published_at DESC, v.id DESC"
        ))
        .bind(podcast_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// One page of a podcast's episodes, newest first. Pages are numbered from 1.
    pub async fn episodes_page(&self, podcast_id: i64, page: i64) -> Result<Vec<EpisodeView>> {
        Ok(sqlx::query_as(episode_view_sql!(
            "WHERE e.podcast_id = ? \
             ORDER BY e.published_at IS NULL, e.published_at DESC, e.id DESC LIMIT ? OFFSET ?"
        ))
        .bind(podcast_id)
        .bind(EPISODES_PER_PAGE)
        .bind((page.max(1) - 1) * EPISODES_PER_PAGE)
        .fetch_all(self.pool())
        .await?)
    }

    /// Archived attachments of all episodes of a podcast.
    pub async fn attachments(&self, podcast_id: i64) -> Result<Vec<Attachment>> {
        Ok(sqlx::query_as(
            "SELECT a.id, a.episode_id, a.source_key, a.path, a.mime FROM attachments a \
             JOIN episodes e ON e.id = a.episode_id WHERE e.podcast_id = ? AND a.status = 'ok'",
        )
        .bind(podcast_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn attachment(
        &self,
        podcast_id: i64,
        episode_id: i64,
        attachment_id: i64,
    ) -> Result<Option<Attachment>> {
        Ok(sqlx::query_as(
            "SELECT a.id, a.episode_id, a.source_key, a.path, a.mime FROM attachments a \
             JOIN episodes e ON e.id = a.episode_id \
             WHERE e.podcast_id = ? AND a.episode_id = ? AND a.id = ? AND a.status = 'ok'",
        )
        .bind(podcast_id)
        .bind(episode_id)
        .bind(attachment_id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn episode(&self, podcast_id: i64, episode_id: i64) -> Result<Option<EpisodeView>> {
        Ok(
            sqlx::query_as(episode_view_sql!("WHERE e.podcast_id = ? AND e.id = ?"))
                .bind(podcast_id)
                .bind(episode_id)
                .fetch_optional(self.pool())
                .await?,
        )
    }

    pub async fn stats(&self) -> Result<Stats> {
        Ok(sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM podcasts) AS podcasts, \
             (SELECT COUNT(*) FROM episodes) AS episodes, \
             (SELECT COUNT(DISTINCT episode_id) FROM media_files WHERE status = 'ok') AS archived, \
             (SELECT COALESCE(SUM(size), 0) FROM media_files WHERE status = 'ok') AS bytes, \
             (SELECT COUNT(*) FROM media_files WHERE status != 'ok') \
               + (SELECT COUNT(*) FROM attachments WHERE status IN ('corrupt', 'missing')) AS damaged, \
             (SELECT MIN(verified_at) FROM media_files WHERE status = 'ok') AS oldest_check",
        )
        .fetch_one(self.pool())
        .await?)
    }

    /// Ids of the episodes that are being downloaded right now.
    pub fn downloading(&self) -> HashSet<i64> {
        self.inner.downloading.lock().unwrap().clone()
    }

    /// Searches one directory. Results are cached for a few minutes, and feeds that another
    /// provider already returned for the same query are left out.
    pub async fn search(
        &self,
        provider: search::Provider,
        query: &str,
    ) -> Result<Vec<search::SearchHit>> {
        if let Some(hits) = self.inner.search_cache.lock().unwrap().get(provider, query) {
            return Ok(hits);
        }
        let hits = search::search_provider(&self.inner.client, provider, query).await?;
        Ok(self
            .inner
            .search_cache
            .lock()
            .unwrap()
            .store(provider, query, hits))
    }

    // ── Adding and refreshing ──────────────────────────────────────────────────────────────

    /// Adds a podcast by feed URL and returns its id. Adding a podcast that is already archived
    /// (same URL or same `<podcast:guid>`) returns the existing id. The address of a web page
    /// that announces a feed works as well.
    pub async fn add_podcast(&self, input: &str) -> Result<i64> {
        self.add_feed(input, false).await
    }

    /// `discovered` is set when `input` was taken from a web page; such a feed is not searched
    /// for a further feed.
    async fn add_feed(&self, input: &str, discovered: bool) -> Result<i64> {
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

        let fetched = match self.fetch_all(url, None, None).await {
            // A web page that announces feeds: add the one that carries episodes.
            Err(error) => match error.downcast_ref::<FeedElsewhere>() {
                Some(FeedElsewhere(announced)) if !discovered => {
                    let target = self.podcast_feed_among(announced).await.context(
                        "this address is a web page, and none of the feeds it announces \
                         contains episodes",
                    )?;
                    tracing::info!(page = url, feed = %target, "feed found on the web page");
                    return Box::pin(self.add_feed(target.as_str(), true)).await;
                }
                _ => return Err(error),
            },
            Ok(fetched) => fetched,
        };
        let Some(fetched) = fetched else {
            bail!("the server answered 'not modified' to an unconditional request");
        };
        let parsed = &fetched.parsed;

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

        tracing::info!(podcast = %parsed.title, episodes = parsed.items.len(), "podcast added");
        self.ingest(id, fetched).await?;
        Ok(id)
    }

    /// The first of the announced feeds that has an episode with a media file. Pages commonly
    /// list feeds of blog posts and comments before the podcast feed.
    async fn podcast_feed_among(&self, announced: &[reqwest::Url]) -> Option<reqwest::Url> {
        for target in announced.iter().take(MAX_ANNOUNCED_FEEDS) {
            if check_url(target, self.inner.cfg.allow_private_networks).is_err() {
                continue;
            }
            let Ok(Fetched::Body { bytes, .. }) =
                self.fetch_feed(target.as_str(), None, None).await
            else {
                continue;
            };
            if feed::parse(&bytes).is_ok_and(|f| f.items.iter().any(|i| i.enclosure_url.is_some()))
            {
                return Some(target.clone());
            }
        }
        None
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
            .fetch_all(
                &podcast.feed_url,
                podcast.etag.as_deref(),
                podcast.last_modified.as_deref(),
            )
            .await?;
        match fetched {
            None => {
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
            Some(fetched) => self.ingest(podcast_id, fetched).await,
        }
    }

    /// Fetches a feed and, if it is split into pages, every further page. `None` means the
    /// first page has not changed since the given validators were issued.
    ///
    /// A page that cannot be fetched fails the whole request: an incomplete feed would make
    /// the episodes of the missing pages look removed.
    async fn fetch_all(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<Option<FetchedFeed>> {
        let Fetched::Body {
            bytes,
            etag,
            last_modified,
        } = self.fetch_feed(url, etag, last_modified).await?
        else {
            return Ok(None);
        };
        let mut current = reqwest::Url::parse(url)?;
        let mut parsed = match feed::parse(&bytes) {
            Ok(parsed) => parsed,
            Err(error) => {
                let announced = feed::discover(&bytes, &current);
                return Err(if announced.is_empty() {
                    if feed::looks_like_html(&bytes) {
                        anyhow::anyhow!("this address is a web page that announces no podcast feed")
                    } else {
                        error
                    }
                } else {
                    FeedElsewhere(announced).into()
                });
            }
        };
        let mut pages = vec![bytes];

        let mut visited: HashSet<String> = HashSet::from([current.to_string()]);
        let mut guids: HashSet<String> = parsed.items.iter().map(|i| i.guid.clone()).collect();
        let mut next = parsed.next_page.take();
        while let Some(link) = next {
            let Ok(target) = current.join(&link) else {
                break;
            };
            // A link back to a page that was already read ends the chain.
            if !visited.insert(target.to_string()) {
                break;
            }
            if pages.len() >= MAX_FEED_PAGES {
                bail!("the feed has more than {MAX_FEED_PAGES} pages");
            }
            check_url(&target, self.inner.cfg.allow_private_networks)
                .with_context(|| format!("page {} of the feed", pages.len() + 1))?;
            let Fetched::Body { bytes, .. } = self.fetch_feed(target.as_str(), None, None).await?
            else {
                bail!("the server answered 'not modified' to an unconditional request");
            };
            let page = feed::parse(&bytes)
                .with_context(|| format!("page {} of the feed ({target})", pages.len() + 1))?;
            pages.push(bytes);
            for namespace in page.namespaces {
                if !parsed.namespaces.iter().any(|(p, _)| *p == namespace.0) {
                    parsed.namespaces.push(namespace);
                }
            }
            for item in page.items {
                if guids.insert(item.guid.clone()) {
                    parsed.items.push(item);
                }
            }
            next = page.next_page;
            current = target;
        }
        Ok(Some(FetchedFeed {
            parsed,
            pages,
            etag,
            last_modified,
        }))
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
                "application/rss+xml, application/atom+xml, application/xml;q=0.9, */*;q=0.5",
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
    async fn ingest(&self, podcast_id: i64, fetched: FetchedFeed) -> Result<()> {
        let FetchedFeed {
            parsed,
            pages,
            etag,
            last_modified,
        } = fetched;
        let parsed = &parsed;
        let podcast = self
            .podcast(podcast_id)
            .await?
            .context("podcast not found")?;
        let now = now();
        let podcast_dir = self.archive_path(&podcast.dir);

        // 1. Keep the feed exactly as served, once per distinct content. A paged feed is kept
        //    page by page.
        let stamp =
            crate::util::format_utc(now, "%Y%m%dT%H%M%SZ").unwrap_or_else(|| now.to_string());
        for bytes in &pages {
            let hash = blake3::hash(bytes).to_hex().to_string();
            let known: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM feed_snapshots WHERE podcast_id = ? AND blake3 = ?",
            )
            .bind(podcast_id)
            .bind(&hash)
            .fetch_optional(self.pool())
            .await?;
            if known.is_some() {
                continue;
            }
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
             author = ?, description = ?, link = ?, image_url = ?, language = ?, namespaces = ?, \
             etag = ?, last_modified = ?, last_fetched_at = ?, last_fetch_error = NULL WHERE id = ?",
        )
        .bind(&feed_url)
        .bind(&parsed.podcast_guid)
        .bind(&parsed.title)
        .bind(&parsed.author)
        .bind(&parsed.description)
        .bind(&parsed.link)
        .bind(&parsed.image_url)
        .bind(&parsed.language)
        .bind(serde_json::to_string(&parsed.namespaces)?)
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
            let existing: Option<(i64, String, String, Option<String>)> = sqlx::query_as(
                "SELECT id, meta_hash, dir, enclosure_url FROM episodes \
                 WHERE podcast_id = ? AND guid = ?",
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
                Some((id, old_hash, dir, old_enclosure)) => {
                    let is_changed = old_hash != meta_hash;
                    // A new address (ignoring its query string) means the source replaced the file.
                    let replaced = podcast.keep_versions
                        && item.enclosure_url.is_some()
                        && old_enclosure.as_deref().map(without_query)
                            != item.enclosure_url.as_deref().map(without_query);
                    if is_changed {
                        // A changed episode gets a fresh chance if its download had given up.
                        sqlx::query(
                            "UPDATE episodes SET title = ?, description = ?, link = ?, \
                             published_at = ?, duration = ?, enclosure_url = ?, enclosure_type = ?, \
                             enclosure_length = ?, image_url = ?, season = ?, episode_no = ?, \
                             meta_hash = ?, last_seen_at = ?, removed_at = NULL, \
                             download_attempts = 0, next_attempt_at = 0, \
                             refetch = MAX(refetch, ?) WHERE id = ?",
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
                        .bind(replaced)
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
            // An episode image that merely repeats the podcast's cover is not worth a copy per
            // episode; real feeds do this for thousands of episodes.
            let cover = parsed.image_url.as_deref().map(without_query);
            for link in item
                .links
                .iter()
                .filter(|l| !(l.kind == "image" && Some(without_query(&l.url)) == cover))
            {
                // The address may carry a fresh token at every fetch; keep the latest.
                sqlx::query(
                    "INSERT INTO attachments (episode_id, kind, source_key, source_url) \
                     VALUES (?, ?, ?, ?) ON CONFLICT (episode_id, source_key) \
                     DO UPDATE SET source_url = excluded.source_url",
                )
                .bind(episode_id)
                .bind(link.kind)
                .bind(without_query(&link.url))
                .bind(&link.url)
                .execute(&mut *tx)
                .await?;
            }
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

    pub async fn set_options(
        &self,
        podcast_id: i64,
        include_removed: bool,
        keep_versions: bool,
    ) -> Result<()> {
        sqlx::query("UPDATE podcasts SET include_removed = ?, keep_versions = ? WHERE id = ?")
            .bind(include_removed)
            .bind(keep_versions)
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Removes a podcast from the index. The archived files stay on disk unless `delete_files`
    /// is set; without the index they remain a readable, self-describing directory.
    pub async fn remove_podcast(&self, podcast_id: i64, delete_files: bool) -> Result<()> {
        let podcast = self
            .podcast(podcast_id)
            .await?
            .context("podcast not found")?;
        // Episodes, revisions, snapshots and media rows go with it (ON DELETE CASCADE).
        sqlx::query("DELETE FROM podcasts WHERE id = ?")
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        tracing::info!(podcast = %podcast.title, delete_files, "podcast removed");
        if delete_files {
            // `dir` is generated by Uguisu as a single path segment; refuse anything else
            // rather than risk deleting outside the podcast's own directory.
            let is_single_segment = !podcast.dir.is_empty()
                && !podcast.dir.contains(['/', '\\'])
                && podcast.dir != "."
                && podcast.dir != "..";
            if !is_single_segment {
                bail!("refusing to delete unexpected directory {:?}", podcast.dir);
            }
            let dir = self.archive_path(&podcast.dir);
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot delete {}", dir.display()));
                }
            }
        }
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
             AND (e.refetch = 1 OR NOT EXISTS \
               (SELECT 1 FROM media_files m WHERE m.episode_id = e.id AND m.status = 'ok')) \
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
            let attachments = self.pending_attachments().await?;
            if pending.is_empty() && attachments.is_empty() {
                return Ok(attempted);
            }
            attempted += pending.len();
            let concurrency = self.inner.cfg.download_concurrency.max(1);
            futures_util::stream::iter(pending)
                .for_each_concurrent(concurrency, |p| async move {
                    self.download_one(p).await;
                })
                .await;
            futures_util::stream::iter(attachments)
                .for_each_concurrent(concurrency, |a| async move {
                    self.download_attachment(a).await;
                })
                .await;
        }
    }

    async fn pending_attachments(&self) -> Result<Vec<PendingAttachment>> {
        Ok(sqlx::query_as(
            "SELECT a.id, a.kind, a.source_key, a.source_url, a.attempts, \
             e.dir AS episode_dir, p.dir AS podcast_dir \
             FROM attachments a JOIN episodes e ON e.id = a.episode_id \
             JOIN podcasts p ON p.id = e.podcast_id \
             WHERE a.status != 'ok' AND a.attempts < ? AND a.next_attempt_at <= ? LIMIT 50",
        )
        .bind(MAX_DOWNLOAD_ATTEMPTS)
        .bind(now())
        .fetch_all(self.pool())
        .await?)
    }

    async fn download_attachment(&self, pending: PendingAttachment) {
        let outcome = match self.fetch_attachment(&pending).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(url = %pending.source_key, "attachment download failed: {e:#}");
                let delay = 300 * 4i64.pow(pending.attempts.clamp(0, 6) as u32);
                sqlx::query(
                    "UPDATE attachments SET attempts = attempts + 1, error = ?, \
                     next_attempt_at = ? WHERE id = ?",
                )
                .bind(format!("{e:#}"))
                .bind(now() + delay)
                .bind(pending.id)
                .execute(self.pool())
                .await
                .map(|_| ())
            }
        };
        if let Err(e) = outcome {
            tracing::error!("cannot record attachment result: {e:#}");
        }
    }

    async fn fetch_attachment(&self, pending: &PendingAttachment) -> Result<()> {
        let url = url::Url::parse(&pending.source_url).context("invalid address")?;
        check_url(&url, self.inner.cfg.allow_private_networks)?;
        let mut response = self
            .inner
            .client
            .get(url)
            .timeout(Duration::from_secs(120))
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("the server answered {}", response.status());
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
        let ext = archive::extension_from_url(&pending.source_key)
            .or_else(|| archive::extension_from_mime(&content_type))
            .unwrap_or("bin");
        let mime = archive::mime_from_extension(ext)
            .map(str::to_string)
            .or_else(|| (!content_type.is_empty()).then(|| content_type.clone()))
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.context("download interrupted")? {
            if bytes.len() + chunk.len() > MAX_ATTACHMENT_BYTES {
                bail!("the file is larger than {} MiB", MAX_ATTACHMENT_BYTES >> 20);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            bail!("the server sent an empty file");
        }

        // Several transcripts per episode are common, so the name carries a hash of the address.
        let file_name = format!("{}-{}.{ext}", pending.kind, short_hash(&pending.source_key));
        let relative = format!(
            "{}/episodes/{}/{file_name}",
            pending.podcast_dir, pending.episode_dir
        );
        write_atomic(&self.archive_path(&relative), &bytes).await?;
        let now = now();
        sqlx::query(
            "UPDATE attachments SET path = ?, size = ?, blake3 = ?, mime = ?, downloaded_at = ?, \
             verified_at = ?, status = 'ok', error = NULL WHERE id = ?",
        )
        .bind(&relative)
        .bind(bytes.len() as i64)
        .bind(blake3::hash(&bytes).to_hex().to_string())
        .bind(&mime)
        .bind(now)
        .bind(now)
        .bind(pending.id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn download_one(&self, pending: PendingDownload) {
        let episode = &pending.episode;
        self.inner.downloading.lock().unwrap().insert(episode.id);
        let result = self.download(episode, &pending.podcast_dir).await;
        self.inner.downloading.lock().unwrap().remove(&episode.id);

        let outcome = match result {
            Ok(()) => {
                tracing::info!(episode = %episode.title, "archived");
                sqlx::query("UPDATE episodes SET download_error = NULL, refetch = 0 WHERE id = ?")
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

        let relative_dir = format!("{podcast_dir}/episodes/{}", episode.dir);
        let dir = self.archive_path(&relative_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let part = dir.join("download.part");

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
        let hash = hasher.finalize().to_hex().to_string();

        // Decide where the copy goes. The first copy, and the repair of a damaged one, is
        // `media.<ext>`. A copy with different content from a replaced source file is kept next
        // to the earlier ones as `media.v2.<ext>`, `media.v3.<ext>`, ...
        let existing: Vec<(String, String, String)> =
            sqlx::query_as("SELECT path, blake3, status FROM media_files WHERE episode_id = ?")
                .bind(episode.id)
                .fetch_all(self.pool())
                .await?;
        if existing
            .iter()
            .any(|(_, b, status)| *b == hash && status == "ok")
        {
            // The source moved the file without changing it.
            let _ = tokio::fs::remove_file(&part).await;
            return Ok(());
        }
        let intact = existing.iter().filter(|(_, _, s)| s == "ok").count();
        let file_name = if intact == 0 {
            format!("media.{ext}")
        } else {
            let mut version = existing.len() + 1;
            loop {
                let candidate = format!("media.v{version}.{ext}");
                let taken = existing
                    .iter()
                    .any(|(path, _, _)| path.rsplit('/').next() == Some(candidate.as_str()));
                if !taken {
                    break candidate;
                }
                version += 1;
            }
        };
        tokio::fs::rename(&part, dir.join(&file_name)).await?;

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
        if let Some(interval) = self.inner.cfg.verify_interval {
            let this = self.clone();
            tokio::spawn(async move {
                loop {
                    // Small batches keep the disk free for downloads and playback.
                    match this.verify_due(interval, 25).await {
                        Ok(report) if report.checked > 0 => {
                            tracing::info!(
                                checked = report.checked,
                                damaged = report.corrupt.len() + report.missing.len(),
                                "integrity check"
                            );
                            if report.checked == 25 {
                                continue;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => tracing::error!("integrity check failed: {e:#}"),
                    }
                    tokio::time::sleep(Duration::from_secs(600)).await;
                }
            });
        }
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

    /// Re-hashes every archived file and compares it with the hash recorded at download.
    /// Damaged or missing files are flagged, which also queues them for a new download.
    pub async fn verify(&self) -> Result<VerifyReport> {
        let media: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, path, blake3 FROM media_files ORDER BY path")
                .fetch_all(self.pool())
                .await?;
        let attachments: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT id, path, blake3 FROM attachments WHERE path IS NOT NULL ORDER BY path",
        )
        .fetch_all(self.pool())
        .await?;
        let mut report = self.verify_files(Files::Media, media).await?;
        report.merge(self.verify_files(Files::Attachments, attachments).await?);
        Ok(report)
    }

    /// Re-hashes up to `limit` intact files that have not been checked for `max_age`, oldest
    /// check first. Called periodically, this spreads a full pass over the archive.
    pub async fn verify_due(&self, max_age: Duration, limit: i64) -> Result<VerifyReport> {
        let cutoff = now() - max_age.as_secs() as i64;
        let media: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT id, path, blake3 FROM media_files WHERE status = 'ok' AND verified_at <= ? \
             ORDER BY verified_at LIMIT ?",
        )
        .bind(cutoff)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        let remaining = limit - media.len() as i64;
        let attachments: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT id, path, blake3 FROM attachments WHERE status = 'ok' AND verified_at <= ? \
             ORDER BY verified_at LIMIT ?",
        )
        .bind(cutoff)
        .bind(remaining.max(0))
        .fetch_all(self.pool())
        .await?;
        let mut report = self.verify_files(Files::Media, media).await?;
        report.merge(self.verify_files(Files::Attachments, attachments).await?);
        Ok(report)
    }

    async fn verify_files(
        &self,
        table: Files,
        files: Vec<(i64, String, String)>,
    ) -> Result<VerifyReport> {
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
                    tracing::error!(file = %relative, "archived file is damaged");
                    report.corrupt.push(relative);
                    "corrupt"
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tracing::error!(file = %relative, "archived file is missing");
                    report.missing.push(relative);
                    "missing"
                }
                Err(e) => return Err(e).with_context(|| format!("cannot read {relative}")),
            };
            // An intact file gets a new check date; a damaged one keeps the date it was last good
            // and, for attachments, a fresh set of download attempts.
            let statement = match (table, status == "ok") {
                (Files::Media, true) => {
                    "UPDATE media_files SET status = ?, verified_at = ? WHERE id = ?"
                }
                (Files::Media, false) => {
                    "UPDATE media_files SET status = ?, verified_at = MIN(verified_at, ?) WHERE id = ?"
                }
                (Files::Attachments, true) => {
                    "UPDATE attachments SET status = ?, verified_at = ? WHERE id = ?"
                }
                (Files::Attachments, false) => {
                    "UPDATE attachments SET status = ?, verified_at = MIN(verified_at, ?), \
                     attempts = 0, next_attempt_at = 0 WHERE id = ?"
                }
            };
            sqlx::query(statement)
                .bind(status)
                .bind(now())
                .bind(id)
                .execute(self.pool())
                .await?;
        }
        if !report.corrupt.is_empty() || !report.missing.is_empty() {
            self.inner.wake_downloader.notify_one();
        }
        Ok(report)
    }
}
