//! The archive service: adding podcasts, refreshing feeds, downloading media, verifying files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sqlx::{FromRow, SqlitePool};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::archive::{self, write_atomic};
use crate::feed::{self, ParsedFeed, ParsedItem, without_query};
use crate::http::{build_client, check_url, lend_credentials, parse_user_url, redact};
use crate::model::{
    Attachment, AttachmentDetail, DamagedFile, DownloadProblem, Episode, EpisodeView, FeedEpisode,
    FeedSnapshot, HistoryEvent, MediaFile, NewPodcastDefaults, Podcast, PodcastSummary, Revision,
    SearchResult, Stats,
};
use crate::util::{now, random_token, short_hash};
use crate::{db, search};

/// Version of the on-disk archive format, described in `docs/ARCHIVE.md`.
pub const ARCHIVE_FORMAT: u64 = 5;
/// Value of the database's `user_version` once its paths follow the named layout of format 5.
pub const LAYOUT_NAMED: i64 = 5;
const LAYOUT_NAMED_PRAGMA: &str = "PRAGMA user_version = 5";

async fn write_archive_marker(path: &Path, format: u64) -> Result<()> {
    let content = serde_json::json!({ "format": format, "software": "Uguisu" });
    write_atomic(path, &serde_json::to_vec_pretty(&content)?).await
}
const MAX_FEED_BYTES: usize = 64 * 1024 * 1024;
/// Upper bound for the pages of one feed (RFC 5005); far beyond what real feeds use.
const MAX_FEED_PAGES: usize = 500;
/// How many of the feeds a web page announces are tried to find the podcast feed.
const MAX_ANNOUNCED_FEEDS: usize = 8;
const MAX_COVER_BYTES: usize = 32 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOWNLOAD_ATTEMPTS: i64 = 5;
/// Longest break a host can ask for with `Retry-After`, in seconds.
const MAX_HOST_BREAK: u64 = 3600;
/// How often an interrupted download is continued within one attempt.
const MAX_RESUMES: u32 = 5;
/// Under this name the options for new podcasts are stored, in the database and the archive.
const NEW_PODCASTS_KEY: &str = "new_podcasts";
/// A feed is reported as failing when fetching it has failed this many times in a row.
const FEED_FAILING_AFTER: i64 = 3;
/// A notification names at most this many files; the counts are always complete.
const MAX_NOTIFIED_FILES: usize = 50;

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
         SUM(e.enclosure_url IS NOT NULL AND e.download_attempts < 5 \
           AND (o.archive_from IS NULL OR e.published_at IS NULL \
             OR e.published_at >= o.archive_from) \
           AND (e.refetch = 1 OR NOT EXISTS \
           (SELECT 1 FROM media_files m WHERE m.episode_id = e.id AND m.status = 'ok'))) \
           AS pending_count \
       FROM episodes e JOIN podcasts o ON o.id = e.podcast_id \
       GROUP BY e.podcast_id) ec ON ec.podcast_id = p.id \
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
       AND (p.archive_from IS NULL OR e.published_at IS NULL \
         OR e.published_at >= p.archive_from) \
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
    /// Shortest time between two requests to the same host, so that archiving a long feed
    /// does not look like an attack to its server.
    pub host_pause: Duration,
    /// Free disk space that downloads leave untouched, in bytes. A disk filled to the last
    /// byte also stops the database and the feed snapshots. `0` turns the check off.
    pub min_free_space: u64,
    /// How many daily copies of the database are kept in `backups/`. `0` turns them off.
    pub backups: usize,
    /// Where notifications about damaged files and failing feeds are sent, as JSON by HTTP
    /// POST. The operator chooses this address, so it may also lie in a private network.
    pub webhook_url: Option<String>,
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
            host_pause: Duration::from_secs(1),
            min_free_space: 1 << 30,
            backups: 7,
            webhook_url: None,
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
    episode_id: i64,
    kind: String,
    source_key: String,
    source_url: String,
    attempts: i64,
    episode_dir: String,
    podcast_dir: String,
    feed_url: String,
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
    feed_url: String,
}

struct Inner {
    cfg: Config,
    pool: SqlitePool,
    client: reqwest::Client,
    /// For notifications only. See [`Config::webhook_url`].
    webhook_client: reqwest::Client,
    wake_downloader: Notify,
    downloading: Mutex<HashSet<i64>>,
    search_cache: Mutex<search::SearchCache>,
    hosts: Mutex<HashMap<String, HostState>>,
    /// Held while a transcript is put into the search index. Transcripts of one episode are
    /// downloaded side by side; without it, a run that read the list of transcripts before the
    /// others were done could write its choice last and replace the cleaner one.
    transcript_index: tokio::sync::Mutex<()>,
}

/// Midnight (UTC) of the current day: the start date of a podcast added with "only episodes
/// from today on".
pub fn start_of_today() -> i64 {
    let now = now();
    now - now.rem_euclid(86_400)
}

/// When a host may be asked again.
#[derive(Clone, Copy)]
struct HostState {
    /// Start of the next free slot, kept apart by [`Config::host_pause`].
    next_request: Instant,
    /// Set while the host has asked for a break with `Retry-After`.
    busy_until: Option<Instant>,
}

/// A host answered "too many requests" or "unavailable" and named a waiting time. This is not a
/// failure of the file that was asked for, so it does not count as a download attempt.
#[derive(Debug)]
struct HostBusy {
    host: String,
    seconds: u64,
}

impl std::fmt::Display for HostBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} asked to wait {} seconds before the next request",
            self.host, self.seconds
        )
    }
}

impl std::error::Error for HostBusy {}

/// A download was not started, or was stopped, because it would use up the reserve of free
/// disk space. Like a host's break this says nothing about the file itself.
#[derive(Debug)]
struct DiskFull {
    free: u64,
    needed: u64,
}

impl std::fmt::Display for DiskFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mib = |bytes: u64| bytes >> 20;
        write!(
            f,
            "not enough disk space: {} MiB free, {} MiB needed including the reserve",
            mib(self.free),
            mib(self.needed)
        )
    }
}

impl std::error::Error for DiskFull {}

/// How long to wait before trying again after running out of disk space.
const DISK_FULL_RETRY: i64 = 3600;

/// Seconds to wait if `error` is no failure of the file: a host's request for a break, or a
/// full disk.
fn host_break(error: &anyhow::Error) -> Option<i64> {
    if error.downcast_ref::<DiskFull>().is_some() {
        return Some(DISK_FULL_RETRY);
    }
    error
        .downcast_ref::<HostBusy>()
        .map(|busy| busy.seconds as i64)
}

/// `Retry-After` is either a number of seconds or a date.
fn retry_after(response: &reqwest::Response) -> Option<u64> {
    let value = header(response, reqwest::header::RETRY_AFTER)?;
    let seconds = match value.trim().parse::<u64>() {
        Ok(seconds) => seconds,
        Err(_) => u64::try_from(feed::parse_date(&value)? - now()).ok()?,
    };
    Some(seconds.clamp(1, MAX_HOST_BREAK))
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

impl Uguisu {
    pub async fn open(cfg: Config) -> Result<Self> {
        let archive_dir = cfg.data_dir.join("archive");
        tokio::fs::create_dir_all(&archive_dir)
            .await
            .with_context(|| format!("cannot create {}", archive_dir.display()))?;
        // The marker says which version of docs/ARCHIVE.md the archive follows. An archive
        // written by a newer Uguisu is left alone rather than risked.
        let marker = archive_dir.join("archive.json");
        match tokio::fs::read(&marker).await {
            Ok(bytes) => {
                let format = serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|value| value["format"].as_u64())
                    .with_context(|| format!("cannot read {}", marker.display()))?;
                if format > ARCHIVE_FORMAT {
                    bail!(
                        "the archive in {} has format version {format}, this version of Uguisu \
                         knows up to {ARCHIVE_FORMAT}; use a newer Uguisu",
                        archive_dir.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // An archive from before the marker existed is in the first format.
                let mut entries = tokio::fs::read_dir(&archive_dir).await?;
                let format = match entries.next_entry().await? {
                    Some(_) => 1,
                    None => ARCHIVE_FORMAT,
                };
                write_archive_marker(&marker, format).await?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", marker.display()));
            }
        }
        let pool = db::open(&cfg.data_dir.join("uguisu.db")).await?;
        // A database that knows nothing yet describes a current archive as it is.
        let empty: bool = sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM podcasts)")
            .fetch_one(&pool)
            .await?;
        let format = serde_json::from_slice::<serde_json::Value>(&tokio::fs::read(&marker).await?)
            .ok()
            .and_then(|value| value["format"].as_u64())
            .unwrap_or(1);
        if empty && format >= 5 {
            sqlx::query(LAYOUT_NAMED_PRAGMA).execute(&pool).await?;
        }
        let client = build_client(&cfg.user_agent, cfg.allow_private_networks)?;
        if let Some(address) = &cfg.webhook_url {
            let valid = url::Url::parse(address)
                .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some());
            anyhow::ensure!(valid, "the webhook address must be an http or https URL");
        }
        let webhook_client = reqwest::Client::builder()
            .user_agent(&cfg.user_agent)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .context("cannot build HTTP client")?;
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                pool,
                client,
                webhook_client,
                wake_downloader: Notify::new(),
                downloading: Mutex::new(HashSet::new()),
                search_cache: Mutex::new(search::SearchCache::default()),
                hosts: Mutex::new(HashMap::new()),
                transcript_index: tokio::sync::Mutex::new(()),
            }),
        })
    }

    /// Sends a request to a source, politely: requests to one host keep a distance of
    /// [`Config::host_pause`], and a host that asks for a break with `Retry-After` gets it.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let (client, request) = request.build_split();
        let request = request?;
        let host = request.url().host_str().unwrap_or_default().to_string();
        let slot = {
            let mut hosts = self.inner.hosts.lock().unwrap();
            let now = Instant::now();
            let state = hosts.entry(host.clone()).or_insert(HostState {
                next_request: now,
                busy_until: None,
            });
            if let Some(until) = state.busy_until.filter(|until| *until > now) {
                let seconds = (until - now).as_secs().max(1);
                return Err(HostBusy { host, seconds }.into());
            }
            let slot = state.next_request.max(now);
            state.next_request = slot + self.inner.cfg.host_pause;
            slot
        };
        tokio::time::sleep_until(slot.into()).await;

        // Neither the address in the message nor the one reqwest adds may show a password.
        let url = redact(request.url().as_str());
        let response = client
            .execute(request)
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("cannot fetch {url}"))?;
        let overloaded = matches!(
            response.status(),
            reqwest::StatusCode::TOO_MANY_REQUESTS | reqwest::StatusCode::SERVICE_UNAVAILABLE
        );
        if overloaded && let Some(seconds) = retry_after(&response) {
            if let Some(state) = self.inner.hosts.lock().unwrap().get_mut(&host) {
                state.busy_until = Some(Instant::now() + Duration::from_secs(seconds));
            }
            return Err(HostBusy { host, seconds }.into());
        }
        Ok(response)
    }

    /// Free space on the disk that holds the archive, if the system tells.
    pub fn free_space(&self) -> Option<u64> {
        let stat = rustix::fs::statvfs(self.archive_dir()).ok()?;
        Some(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    /// Whether downloads are on hold because the reserve of free disk space is used up.
    pub fn space_is_low(&self) -> bool {
        let reserve = self.inner.cfg.min_free_space;
        reserve > 0 && self.free_space().is_some_and(|free| free < reserve)
    }

    /// Fails if writing `bytes` more would cut into the reserve of free disk space.
    fn ensure_space(&self, bytes: u64) -> Result<()> {
        let reserve = self.inner.cfg.min_free_space;
        if reserve == 0 {
            return Ok(());
        }
        let needed = reserve.saturating_add(bytes);
        match self.free_space() {
            Some(free) if free < needed => Err(DiskFull { free, needed }.into()),
            _ => Ok(()),
        }
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

    /// What `uguisu.json` in the archive root holds: the settings of this installation that a
    /// rebuilt index must find again.
    async fn installation_settings(&self) -> serde_json::Map<String, serde_json::Value> {
        let file = self.archive_dir().join("uguisu.json");
        match tokio::fs::read(&file).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Default::default(),
        }
    }

    async fn write_installation_setting(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let mut settings = self.installation_settings().await;
        settings.insert(key.to_string(), value);
        write_atomic(
            &self.archive_dir().join("uguisu.json"),
            &serde_json::to_vec_pretty(&settings)?,
        )
        .await
    }

    /// The secret part of the address of the feed over all podcasts; created at first use. It
    /// is kept in the archive as well, so that a rebuilt index publishes the same address.
    pub async fn combined_feed_token(&self) -> Result<String> {
        const KEY: &str = "combined_feed_token";
        if let Some(token) = db::get_setting(self.pool(), KEY).await? {
            return Ok(token);
        }
        let stored = self.installation_settings().await;
        let token = match stored.get(KEY).and_then(|token| token.as_str()) {
            Some(token) => token.to_string(),
            None => {
                let token = random_token(24);
                self.write_installation_setting(KEY, token.clone().into())
                    .await?;
                token
            }
        };
        db::set_setting(self.pool(), KEY, &token).await?;
        Ok(token)
    }

    /// The options a podcast gets when it is added.
    pub async fn new_podcast_defaults(&self) -> Result<NewPodcastDefaults> {
        if let Some(stored) = db::get_setting(self.pool(), NEW_PODCASTS_KEY).await? {
            return Ok(serde_json::from_str(&stored).unwrap_or_default());
        }
        // A rebuilt index finds them in the archive.
        let stored = self.installation_settings().await;
        Ok(stored
            .get(NEW_PODCASTS_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default())
    }

    pub async fn set_new_podcast_defaults(&self, defaults: NewPodcastDefaults) -> Result<()> {
        self.write_installation_setting(NEW_PODCASTS_KEY, serde_json::to_value(defaults)?)
            .await?;
        db::set_setting(
            self.pool(),
            NEW_PODCASTS_KEY,
            &serde_json::to_string(&defaults)?,
        )
        .await
    }

    /// The newest archived episodes across all podcasts, each with the id of its podcast.
    /// Episodes that left their source feed are included where the podcast publishes them.
    pub async fn newest_episodes(&self, limit: i64) -> Result<Vec<EpisodeView>> {
        Ok(sqlx::query_as(concat!(
            "SELECT v.* FROM (",
            episode_view_sql!(""),
            ") v JOIN podcasts p ON p.id = v.podcast_id \
             WHERE v.media_path IS NOT NULL AND (v.removed_at IS NULL OR p.include_removed) \
             ORDER BY v.published_at IS NULL, v.published_at DESC, v.id DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(self.pool())
        .await?)
    }

    /// Archived attachments of the given episodes.
    pub async fn attachments_of(&self, episode_ids: &[i64]) -> Result<Vec<Attachment>> {
        let mut found = Vec::new();
        for id in episode_ids {
            let rows: Vec<Attachment> = sqlx::query_as(
                "SELECT id, episode_id, source_key, path, mime FROM attachments \
                 WHERE episode_id = ? AND status = 'ok'",
            )
            .bind(id)
            .fetch_all(self.pool())
            .await?;
            found.extend(rows);
        }
        Ok(found)
    }

    /// Brings the full-text index of one podcast in step with its episodes. Transcripts that
    /// are already indexed stay.
    pub(crate) async fn index_podcast_text(&self, podcast_id: i64) -> Result<()> {
        let rows: Vec<(i64, String, Option<String>, bool)> = sqlx::query_as(
            "SELECT e.id, e.title, e.description, \
             EXISTS (SELECT 1 FROM episode_search s WHERE s.rowid = e.id) \
             FROM episodes e WHERE e.podcast_id = ?",
        )
        .bind(podcast_id)
        .fetch_all(self.pool())
        .await?;
        let mut tx = self.pool().begin().await?;
        for (id, title, description, indexed) in rows {
            let statement = if indexed {
                "UPDATE episode_search SET title = ?, notes = ? WHERE rowid = ?"
            } else {
                "INSERT INTO episode_search (title, notes, transcript, rowid) VALUES (?, ?, '', ?)"
            };
            sqlx::query(statement)
                .bind(title)
                .bind(crate::util::html_to_text(
                    description.as_deref().unwrap_or(""),
                ))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Puts the transcript of an episode into the full-text index. An episode often has the
    /// same transcript in several formats; one of them is enough, the cleanest one available.
    pub(crate) async fn index_transcript(&self, episode_id: i64) -> Result<()> {
        let _one_at_a_time = self.inner.transcript_index.lock().await;
        let mut paths: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM attachments WHERE episode_id = ? AND kind = 'transcript' \
             AND status = 'ok' AND path IS NOT NULL ORDER BY id",
        )
        .bind(episode_id)
        .fetch_all(self.pool())
        .await?;
        // The formats that keep speakers and time codes apart from the words come first; a
        // transcript as a web page or plain text has them in the running text.
        paths.sort_by_key(|path| match path.rsplit('.').next().unwrap_or_default() {
            "json" => 0,
            "vtt" => 1,
            "srt" => 2,
            _ => 3,
        });
        let mut text = String::new();
        for path in paths {
            let Ok(bytes) = tokio::fs::read(self.archive_path(&path)).await else {
                continue;
            };
            let extension = path.rsplit('.').next().unwrap_or_default();
            text = crate::util::transcript_text(&bytes, extension);
            if !text.trim().is_empty() {
                break;
            }
        }
        sqlx::query("UPDATE episode_search SET transcript = ? WHERE rowid = ?")
            .bind(text)
            .bind(episode_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Rebuilds the full-text index if it does not cover exactly the episodes of the archive,
    /// as after an upgrade, a rebuilt database or a removed podcast.
    pub async fn sync_search_index(&self) -> Result<()> {
        let (indexed, episodes, orphans): (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM episode_search), (SELECT COUNT(*) FROM episodes), \
             (SELECT COUNT(*) FROM episode_search WHERE rowid NOT IN (SELECT id FROM episodes))",
        )
        .fetch_one(self.pool())
        .await?;
        if orphans > 0 {
            // A removed podcast: its entries go, the rest of the index stays.
            sqlx::query("DELETE FROM episode_search WHERE rowid NOT IN (SELECT id FROM episodes)")
                .execute(self.pool())
                .await?;
        }
        if indexed - orphans == episodes {
            return Ok(());
        }
        sqlx::query("DELETE FROM episode_search")
            .execute(self.pool())
            .await?;
        let podcasts: Vec<i64> = sqlx::query_scalar("SELECT id FROM podcasts")
            .fetch_all(self.pool())
            .await?;
        for id in podcasts {
            self.index_podcast_text(id).await?;
        }
        let with_transcript: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT episode_id FROM attachments WHERE kind = 'transcript' AND status = 'ok'",
        )
        .fetch_all(self.pool())
        .await?;
        for id in with_transcript {
            self.index_transcript(id).await?;
        }
        tracing::info!(episodes, "search index rebuilt");
        Ok(())
    }

    /// Searches titles, show notes and transcripts of all archived podcasts. Every word has to occur; the
    /// last one may be the beginning of a word. Best matches first.
    pub async fn search_episodes(&self, query: &str, limit: i64) -> Result<Vec<SearchResult>> {
        // The words are passed as quoted strings, so that nothing in them is read as part of
        // the query language.
        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .take(12)
            .map(|word| format!("\"{word}\""))
            .collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let expression = format!("{}*", words.join(" "));
        Ok(sqlx::query_as(
            "SELECT e.id AS episode_id, e.podcast_id, p.title AS podcast_title, e.title, \
             e.published_at, \
             snippet(episode_search, -1, char(1), char(2), '…', 24) AS snippet \
             FROM episode_search JOIN episodes e ON e.id = episode_search.rowid \
             JOIN podcasts p ON p.id = e.podcast_id \
             WHERE episode_search MATCH ? \
             ORDER BY bm25(episode_search, 4.0, 1.0, 0.5), e.published_at DESC LIMIT ?",
        )
        .bind(expression)
        .bind(limit)
        .fetch_all(self.pool())
        .await?)
    }

    /// Episodes whose download has failed and that are not archived, those that have given up
    /// first.
    pub async fn download_problems(&self, limit: i64) -> Result<Vec<DownloadProblem>> {
        Ok(sqlx::query_as(
            "SELECT e.id AS episode_id, e.podcast_id, p.title AS podcast_title, e.title, \
             e.download_attempts, e.download_error, e.next_attempt_at \
             FROM episodes e JOIN podcasts p ON p.id = e.podcast_id \
             WHERE e.download_error IS NOT NULL AND NOT EXISTS \
               (SELECT 1 FROM media_files m WHERE m.episode_id = e.id AND m.status = 'ok') \
             ORDER BY e.download_attempts DESC, e.published_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool())
        .await?)
    }

    /// Archived files that failed their check or are gone.
    pub async fn damaged_files(&self, limit: i64) -> Result<Vec<DamagedFile>> {
        Ok(sqlx::query_as(
            "SELECT e.id AS episode_id, e.podcast_id, p.title AS podcast_title, e.title, \
             f.path, f.status FROM \
               (SELECT episode_id, path, status FROM media_files WHERE status != 'ok' \
                UNION ALL \
                SELECT episode_id, path, status FROM attachments \
                WHERE status IN ('corrupt', 'missing') AND path IS NOT NULL) f \
             JOIN episodes e ON e.id = f.episode_id JOIN podcasts p ON p.id = e.podcast_id \
             ORDER BY p.title, e.published_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool())
        .await?)
    }

    /// Gives an episode whose downloads have failed a fresh set of attempts, starting now.
    pub async fn retry_episode(&self, episode_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE episodes SET download_attempts = 0, next_attempt_at = 0, download_error = NULL \
             WHERE id = ?",
        )
        .bind(episode_id)
        .execute(self.pool())
        .await?;
        sqlx::query(
            "UPDATE attachments SET attempts = 0, next_attempt_at = 0, error = NULL \
             WHERE episode_id = ? AND status != 'ok'",
        )
        .bind(episode_id)
        .execute(self.pool())
        .await?;
        self.inner.wake_downloader.notify_one();
        Ok(())
    }

    /// The same for every episode that has failed.
    pub async fn retry_all(&self) -> Result<u64> {
        let episodes = sqlx::query(
            "UPDATE episodes SET download_attempts = 0, next_attempt_at = 0, download_error = NULL \
             WHERE download_error IS NOT NULL",
        )
        .execute(self.pool())
        .await?
        .rows_affected();
        sqlx::query(
            "UPDATE attachments SET attempts = 0, next_attempt_at = 0, error = NULL \
             WHERE status != 'ok' AND error IS NOT NULL",
        )
        .execute(self.pool())
        .await?;
        self.inner.wake_downloader.notify_one();
        Ok(episodes)
    }

    /// The reserve of free disk space that downloads leave untouched, in bytes.
    pub fn space_reserve(&self) -> u64 {
        self.inner.cfg.min_free_space
    }

    /// The stored versions of a podcast's source feed, newest first.
    pub async fn snapshots(&self, podcast_id: i64) -> Result<Vec<FeedSnapshot>> {
        Ok(sqlx::query_as(
            "SELECT id, fetched_at, blake3, size, path FROM feed_snapshots WHERE podcast_id = ? \
             ORDER BY fetched_at DESC, id DESC",
        )
        .bind(podcast_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn snapshot(
        &self,
        podcast_id: i64,
        snapshot_id: i64,
    ) -> Result<Option<FeedSnapshot>> {
        Ok(sqlx::query_as(
            "SELECT id, fetched_at, blake3, size, path FROM feed_snapshots \
             WHERE podcast_id = ? AND id = ?",
        )
        .bind(podcast_id)
        .bind(snapshot_id)
        .fetch_optional(self.pool())
        .await?)
    }

    /// What happened to the episodes of a podcast in its source feed, newest first: episodes
    /// that appeared, entries the publisher changed, episodes that left the feed.
    pub async fn history(&self, podcast_id: i64, limit: i64) -> Result<Vec<HistoryEvent>> {
        Ok(sqlx::query_as(
            "SELECT first_seen_at AS at, 'added' AS kind, id AS episode_id, title FROM episodes \
             WHERE podcast_id = ?1 \
             UNION ALL \
             SELECT r.seen_at, 'changed', e.id, e.title FROM episode_revisions r \
             JOIN episodes e ON e.id = r.episode_id \
             WHERE e.podcast_id = ?1 AND r.seen_at > e.first_seen_at \
             UNION ALL \
             SELECT removed_at, 'removed', id, title FROM episodes \
             WHERE podcast_id = ?1 AND removed_at IS NOT NULL \
             ORDER BY 1 DESC, 3 DESC LIMIT ?2",
        )
        .bind(podcast_id)
        .bind(limit)
        .fetch_all(self.pool())
        .await?)
    }

    /// Every copy of an episode's media file, oldest first.
    pub async fn media_files(&self, episode_id: i64) -> Result<Vec<MediaFile>> {
        Ok(
            sqlx::query_as("SELECT * FROM media_files WHERE episode_id = ? ORDER BY id")
                .bind(episode_id)
                .fetch_all(self.pool())
                .await?,
        )
    }

    /// The files an episode links to, archived or not.
    pub async fn episode_attachments(&self, episode_id: i64) -> Result<Vec<AttachmentDetail>> {
        Ok(sqlx::query_as(
            "SELECT id, episode_id, kind, source_url, path, size, mime, status, error \
             FROM attachments WHERE episode_id = ? ORDER BY kind, id",
        )
        .bind(episode_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Every recorded state of an episode's feed entry, oldest first.
    pub async fn revisions(&self, episode_id: i64) -> Result<Vec<Revision>> {
        Ok(sqlx::query_as(
            "SELECT seen_at, meta_hash, raw_xml FROM episode_revisions WHERE episode_id = ? \
             ORDER BY seen_at, id",
        )
        .bind(episode_id)
        .fetch_all(self.pool())
        .await?)
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
        self.add_feed(input, false, None).await
    }

    /// Like [`Self::add_podcast`], with a start date: episodes published before it are listed
    /// but their files are not downloaded.
    pub async fn add_podcast_from(&self, input: &str, archive_from: Option<i64>) -> Result<i64> {
        self.add_feed(input, false, archive_from).await
    }

    /// `discovered` is set when `input` was taken from a web page; such a feed is not searched
    /// for a further feed.
    async fn add_feed(
        &self,
        input: &str,
        discovered: bool,
        archive_from: Option<i64>,
    ) -> Result<i64> {
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
                    tracing::info!(page = %redact(url), feed = %target, "feed found on the web page");
                    return Box::pin(self.add_feed(target.as_str(), true, archive_from)).await;
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

        let dir = self
            .unused_podcast_dir(
                &archive::podcast_name(parsed.author.as_deref(), &parsed.title),
                url,
            )
            .await?;
        let defaults = self.new_podcast_defaults().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO podcasts (feed_url, original_feed_url, title, dir, feed_token, created_at, \
             archive_from, include_removed, keep_versions) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             RETURNING id",
        )
        .bind(url)
        .bind(url)
        .bind(&parsed.title)
        .bind(&dir)
        .bind(random_token(24))
        .bind(now())
        .bind(archive_from)
        .bind(defaults.include_removed)
        .bind(defaults.keep_versions)
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

    /// `name`, or `name (2)` and so on: the first name no podcast uses and nothing on the disk
    /// has, compared without case as network shares do. A directory left behind by the same
    /// feed, when it was removed with its files kept, is taken up again.
    async fn unused_podcast_dir(&self, name: &str, feed_url: &str) -> Result<String> {
        let taken: Vec<String> = sqlx::query_scalar("SELECT dir FROM podcasts")
            .fetch_all(self.pool())
            .await?;
        let mut on_disk = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(self.archive_dir()).await {
            while let Some(entry) = entries.next_entry().await? {
                let left_by_this_feed = tokio::fs::read(entry.path().join("podcast.json"))
                    .await
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .is_some_and(|json| json["feed_url"].as_str() == Some(feed_url));
                if !left_by_this_feed {
                    on_disk.push(entry.file_name().to_string_lossy().to_lowercase());
                }
            }
        }
        Ok(crate::util::unique_name(name, |candidate| {
            let candidate = candidate.to_lowercase();
            taken.iter().any(|t| t.to_lowercase() == candidate) || on_disk.contains(&candidate)
        }))
    }

    /// Writes `episode.json`: everything the index knows about an episode and its files, so
    /// that the index can be rebuilt from the archive.
    pub(crate) async fn write_episode_json(&self, episode_id: i64) -> Result<()> {
        let Some((episode, podcast_dir)): Option<(Episode, String)> =
            sqlx::query_as::<_, PendingDownload>(
                "SELECT e.*, p.dir AS podcast_dir, p.feed_url FROM episodes e \
             JOIN podcasts p ON p.id = e.podcast_id WHERE e.id = ?",
            )
            .bind(episode_id)
            .fetch_optional(self.pool())
            .await?
            .map(|row| (row.episode, row.podcast_dir))
        else {
            return Ok(());
        };
        // Media first, each kind oldest first.
        type FileRow = (String, String, String, String, i64, String, i64, i64, i64);
        let files: Vec<FileRow> = sqlx::query_as(
            "SELECT 'media', path, source_url, blake3, size, mime, downloaded_at, 0 AS place, id \
             FROM media_files WHERE episode_id = ?1 \
             UNION ALL \
             SELECT kind, path, source_url, blake3, size, mime, downloaded_at, 1, id \
             FROM attachments WHERE episode_id = ?1 AND path IS NOT NULL AND blake3 IS NOT NULL \
             ORDER BY 8, 9",
        )
        .bind(episode_id)
        .fetch_all(self.pool())
        .await?;
        let files: Vec<serde_json::Value> = files
            .into_iter()
            .map(
                |(kind, path, source, blake3, size, mime, downloaded, _, id)| {
                    serde_json::json!({
                        // The numbers are part of the addresses in the published feed.
                        "id": id,
                        "name": path.rsplit('/').next().unwrap_or(&path),
                        // Where the file is, relative to the podcast's directory.
                        "path": path
                            .strip_prefix(&format!("{podcast_dir}/"))
                            .unwrap_or(&path),
                        "kind": kind,
                        "source": redact(&source),
                        "blake3": blake3,
                        "size": size,
                        "type": mime,
                        "downloaded": iso_date(downloaded),
                    })
                },
            )
            .collect();
        // Every state of the feed entry is kept: the current one as item.xml, earlier ones
        // under the time they were seen.
        let revisions: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT seen_at, meta_hash, raw_xml FROM episode_revisions WHERE episode_id = ? \
             ORDER BY seen_at, id",
        )
        .bind(episode_id)
        .fetch_all(self.pool())
        .await?;
        let episode_dir = self.archive_path(&archive::metadata_dir(&podcast_dir, &episode.dir));
        let mut revision_list = Vec::with_capacity(revisions.len());
        for (seen_at, hash, raw_xml) in &revisions {
            // Not simply the last one: a publisher can also return to an earlier state.
            let name = if *hash == episode.meta_hash {
                "item.xml".to_string()
            } else {
                let stamp = crate::util::format_utc(*seen_at, "%Y%m%dT%H%M%SZ")
                    .unwrap_or_else(|| seen_at.to_string());
                format!("item-{stamp}-{}.xml", &hash[..hash.len().min(8)])
            };
            let path = episode_dir.join(&name);
            // The current entry is written by whoever changed it; here only what is missing.
            if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                write_atomic(&path, raw_xml.as_bytes()).await?;
            }
            revision_list.push(serde_json::json!({
                "seen": iso_date(*seen_at),
                "hash": hash,
                "file": name,
            }));
        }
        let json = serde_json::json!({
            "id": episode.id,
            "guid": episode.guid,
            "title": episode.title,
            "description": episode.description,
            "link": episode.link,
            "published": episode.published_at.and_then(iso_date),
            "duration_seconds": episode.duration,
            "enclosure": {
                "url": episode.enclosure_url,
                "type": episode.enclosure_type,
                "length": episode.enclosure_length,
            },
            "image": episode.image_url,
            "season": episode.season,
            "episode": episode.episode_no,
            "hash": episode.meta_hash,
            "first_seen": iso_date(episode.first_seen_at),
            "removed": episode.removed_at.and_then(iso_date),
            "revisions": revision_list,
            "files": files,
        });
        write_atomic(
            &episode_dir.join("episode.json"),
            &serde_json::to_vec_pretty(&json)?,
        )
        .await
    }

    /// Writes `uguisu.json`: what this installation has decided about a podcast. Unlike the
    /// other files it is not a record of the source, and it holds the secret part of the
    /// published feed's address.
    async fn write_podcast_settings(&self, podcast_id: i64) -> Result<()> {
        let Some(podcast) = self.podcast(podcast_id).await? else {
            return Ok(());
        };
        let json = serde_json::json!({
            "feed_token": podcast.feed_token,
            "include_removed": podcast.include_removed,
            "keep_versions": podcast.keep_versions,
            "archive_from": podcast.archive_from.and_then(iso_date),
            "added": iso_date(podcast.created_at),
        });
        write_atomic(
            &self.archive_path(&podcast.dir).join("uguisu.json"),
            &serde_json::to_vec_pretty(&json)?,
        )
        .await
    }

    /// Writes a consistent copy of the database to `backups/uguisu-<time>.db` in the data
    /// directory and removes the oldest copies beyond the configured number. The database can
    /// be rebuilt from the archive, but a copy also keeps what the archive leaves out: feed
    /// passwords, the login and the state of downloads.
    pub async fn backup_database(&self) -> Result<PathBuf> {
        let dir = self.inner.cfg.data_dir.join("backups");
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let stamp = crate::util::format_utc(now(), "%Y%m%dT%H%M%SZ").context("invalid time")?;
        let target = dir.join(format!("uguisu-{stamp}.db"));
        // SQLite refuses to write over an existing file, hence the detour.
        let part = dir.join(format!("uguisu-{stamp}.db.part"));
        let _ = tokio::fs::remove_file(&part).await;
        sqlx::query("VACUUM INTO ?")
            .bind(
                part.to_str()
                    .context("the data directory is not valid UTF-8")?,
            )
            .execute(self.pool())
            .await
            .context("cannot copy the database")?;
        tokio::fs::rename(&part, &target).await?;

        let mut copies = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if let Ok(name) = entry.file_name().into_string()
                && name.starts_with("uguisu-")
                && name.ends_with(".db")
            {
                copies.push(name);
            }
        }
        // The names sort by time.
        copies.sort();
        let keep = self.inner.cfg.backups.max(1);
        for name in copies.iter().take(copies.len().saturating_sub(keep)) {
            let _ = tokio::fs::remove_file(dir.join(name)).await;
        }
        Ok(target)
    }

    /// Makes a copy of the database if the newest one is older than a day.
    async fn backup_if_due(&self) -> Result<()> {
        if self.inner.cfg.backups == 0 {
            return Ok(());
        }
        let dir = self.inner.cfg.data_dir.join("backups");
        let mut newest: Option<std::time::SystemTime> = None;
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Some(entry) = entries.next_entry().await? {
                if entry.file_name().to_string_lossy().ends_with(".db")
                    && let Ok(modified) = entry.metadata().await.and_then(|m| m.modified())
                {
                    newest = newest.max(Some(modified));
                }
            }
        }
        let due = newest
            .and_then(|time| time.elapsed().ok())
            .is_none_or(|age| age >= Duration::from_secs(24 * 3600));
        if due {
            let path = self.backup_database().await?;
            tracing::info!(file = %path.display(), "database copied");
        }
        Ok(())
    }

    /// The format version in `archive.json`.
    pub(crate) async fn archive_format(&self) -> Result<u64> {
        let marker = self.archive_dir().join("archive.json");
        Ok(
            serde_json::from_slice::<serde_json::Value>(&tokio::fs::read(&marker).await?)?
                ["format"]
                .as_u64()
                .unwrap_or(1),
        )
    }

    /// Which layout of the archive the paths in the database follow: [`LAYOUT_NAMED`] from
    /// format 5 on, less before. Kept as SQLite's `user_version`.
    pub(crate) async fn database_layout(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(self.pool())
            .await?)
    }

    /// Records whether the paths in the database follow the named layout.
    pub(crate) async fn set_database_layout(&self, named: bool) -> Result<()> {
        // PRAGMA takes no bound parameters.
        let pragma = if named {
            LAYOUT_NAMED_PRAGMA
        } else {
            "PRAGMA user_version = 0"
        };
        sqlx::query(pragma).execute(self.pool()).await?;
        Ok(())
    }

    /// Brings an archive written by an earlier version up to the current format. Returns the
    /// number of episodes whose files were rewritten.
    pub async fn upgrade_archive(&self) -> Result<usize> {
        let marker = self.archive_dir().join("archive.json");
        let format = self.archive_format().await?;
        // The database records the layout it describes, too: a copy from before the move, put
        // back onto a moved archive, still names the old paths.
        let layout = self.database_layout().await?;
        if format >= ARCHIVE_FORMAT && layout >= LAYOUT_NAMED {
            return Ok(0);
        }
        // Format 2: episode.json also lists the episode's files and its state in the feed.
        // Format 3: earlier states of the feed entry are files, and each podcast has uguisu.json.
        // Format 4: episodes and files carry the numbers used in published addresses.
        // Format 5: names as people write them; media files in `episodes/`, the rest in
        // `metadata/`. Moves files, so it runs before anything else writes to the archive.
        if format < 5 || layout < LAYOUT_NAMED {
            tracing::info!("moving the archive to named directories (format 5)");
            self.move_to_named_layout().await?;
            self.set_database_layout(true).await?;
            // The copies made before describe the old paths; one that matches comes first.
            if self.inner.cfg.backups > 0 {
                self.backup_database().await?;
            }
        }
        let podcasts: Vec<i64> = sqlx::query_scalar("SELECT id FROM podcasts ORDER BY id")
            .fetch_all(self.pool())
            .await?;
        for id in podcasts {
            self.write_podcast_settings(id).await?;
        }
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM episodes ORDER BY id")
            .fetch_all(self.pool())
            .await?;
        tracing::info!(
            episodes = ids.len(),
            "upgrading the archive to format {ARCHIVE_FORMAT}"
        );
        for id in &ids {
            self.write_episode_json(*id).await?;
        }
        write_archive_marker(&marker, ARCHIVE_FORMAT).await?;
        tracing::info!("archive upgraded");
        Ok(ids.len())
    }

    /// Adds every URL of an OPML import. Failures are logged and counted, not fatal.
    pub async fn import_feeds(&self, urls: &[String]) -> ImportReport {
        let mut report = ImportReport::default();
        let only_new = self
            .new_podcast_defaults()
            .await
            .is_ok_and(|defaults| defaults.only_new);
        let archive_from = only_new.then(start_of_today);
        for url in urls {
            match self.add_podcast_from(url, archive_from).await {
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
        let failures: Option<i64> = match &result {
            Err(e) => {
                sqlx::query_scalar(
                    "UPDATE podcasts SET last_fetched_at = ?, last_fetch_error = ?, \
                     fetch_failures = fetch_failures + 1 WHERE id = ? RETURNING fetch_failures",
                )
                .bind(now())
                .bind(format!("{e:#}"))
                .bind(podcast_id)
                .fetch_optional(self.pool())
                .await?
            }
            Ok(()) => None,
        };
        match (&result, failures) {
            (Err(e), Some(FEED_FAILING_AFTER)) => {
                self.notify_about_feed(podcast_id, "feed.failing", Some(&format!("{e:#}")))
                    .await;
            }
            (Ok(()), _) => {
                let before: i64 =
                    sqlx::query_scalar("SELECT fetch_failures FROM podcasts WHERE id = ?")
                        .bind(podcast_id)
                        .fetch_optional(self.pool())
                        .await?
                        .unwrap_or(0);
                if before > 0 {
                    sqlx::query("UPDATE podcasts SET fetch_failures = 0 WHERE id = ?")
                        .bind(podcast_id)
                        .execute(self.pool())
                        .await?;
                }
                if before >= FEED_FAILING_AFTER {
                    self.notify_about_feed(podcast_id, "feed.recovered", None)
                        .await;
                }
            }
            _ => {}
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
            let Ok(mut target) = current.join(&link) else {
                break;
            };
            lend_credentials(&mut target, url);
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
        let mut response = self.send(request).await?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        if !response.status().is_success() {
            bail!(
                "the feed server answered {} for {}",
                response.status(),
                redact(url)
            );
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
                tracing::info!(
                    podcast = %podcast.title,
                    from = %redact(&podcast.feed_url),
                    to = %redact(target.as_str()),
                    "feed moved"
                );
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
        let mut changed: Vec<(i64, String, &ParsedItem)> = Vec::new();
        // Episodes that left the feed or came back; their files say so too.
        let mut status_changed: Vec<i64> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::with_capacity(parsed.items.len());
        let mut tx = self.pool().begin().await?;
        for item in &parsed.items {
            let meta_hash = item.meta_hash();
            let existing: Option<(i64, String, String, Option<String>, bool)> = sqlx::query_as(
                "SELECT id, meta_hash, dir, enclosure_url, removed_at IS NOT NULL FROM episodes \
                 WHERE podcast_id = ? AND guid = ?",
            )
            .bind(podcast_id)
            .bind(&item.guid)
            .fetch_optional(&mut *tx)
            .await?;

            // A better file than the enclosure, if the feed offers one. Only recorded: whether
            // the source replaced its file is still judged by the enclosure.
            let best = item.best_media();
            let (episode_id, dir, is_new_revision) = match existing {
                None => {
                    let taken: Vec<String> =
                        sqlx::query_scalar("SELECT dir FROM episodes WHERE podcast_id = ?")
                            .bind(podcast_id)
                            .fetch_all(&mut *tx)
                            .await?;
                    let dir = crate::util::unique_name(
                        &archive::episode_name(item.published_at, &item.title),
                        |name| {
                            taken
                                .iter()
                                .any(|t| t.to_lowercase() == name.to_lowercase())
                        },
                    );
                    let id: i64 = sqlx::query_scalar(
                        "INSERT INTO episodes (podcast_id, guid, title, description, link, \
                         published_at, duration, enclosure_url, enclosure_type, enclosure_length, \
                         image_url, season, episode_no, dir, meta_hash, first_seen_at, last_seen_at, \
                         media_url, media_type) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
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
                    .bind(best.as_ref().map(|b| &b.url))
                    .bind(best.as_ref().and_then(|b| b.mime.as_ref()))
                    .fetch_one(&mut *tx)
                    .await?;
                    (id, dir, true)
                }
                Some((id, old_hash, dir, old_enclosure, was_removed)) => {
                    let is_changed = old_hash != meta_hash;
                    if was_removed && !is_changed {
                        status_changed.push(id);
                    }
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
                             refetch = MAX(refetch, ?), \
                             media_url = CASE WHEN ? OR NOT EXISTS (SELECT 1 FROM media_files \
                               WHERE episode_id = episodes.id) THEN ? ELSE media_url END, \
                             media_type = CASE WHEN ? OR NOT EXISTS (SELECT 1 FROM media_files \
                               WHERE episode_id = episodes.id) THEN ? ELSE media_type END \
                             WHERE id = ?",
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
                        // The source is chosen while nothing is archived, or anew when the
                        // source replaced its file. A repair fetches the file it had.
                        .bind(replaced)
                        .bind(best.as_ref().map(|b| &b.url))
                        .bind(replaced)
                        .bind(best.as_ref().and_then(|b| b.mime.as_ref()))
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    } else {
                        sqlx::query(
                            "UPDATE episodes SET last_seen_at = ?, removed_at = NULL, \
                             media_url = CASE WHEN EXISTS (SELECT 1 FROM media_files \
                               WHERE episode_id = episodes.id) THEN media_url ELSE ? END, \
                             media_type = CASE WHEN EXISTS (SELECT 1 FROM media_files \
                               WHERE episode_id = episodes.id) THEN media_type ELSE ? END \
                             WHERE id = ?",
                        )
                        .bind(now)
                        .bind(best.as_ref().map(|b| &b.url))
                        .bind(best.as_ref().and_then(|b| b.mime.as_ref()))
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
                changed.push((episode_id, dir, item));
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
                status_changed.push(id);
            }
        }
        tx.commit().await?;

        // 5. Sidecar files, so the archive explains itself without the database.
        let texts_changed = !changed.is_empty();
        for (id, dir, item) in changed {
            let episode_dir = podcast_dir.join("metadata").join(dir);
            write_atomic(&episode_dir.join("item.xml"), item.raw_xml.as_bytes()).await?;
            self.write_episode_json(id).await?;
        }
        if texts_changed {
            self.index_podcast_text(podcast_id).await?;
        }
        for id in status_changed {
            self.write_episode_json(id).await?;
        }
        let json = serde_json::json!({
            "title": parsed.title,
            "author": parsed.author,
            "description": parsed.description,
            "link": parsed.link,
            "language": parsed.language,
            "image": parsed.image_url,
            "podcast_guid": parsed.podcast_guid.as_ref().or(podcast.podcast_guid.as_ref()),
            // Without user name and password: the archive may be passed on, a password not.
            "feed_url": redact(&feed_url),
            "original_feed_url": redact(&podcast.original_feed_url),
            "namespaces": parsed.namespaces,
            "last_fetched": iso_date(now),
        });
        write_atomic(
            &podcast_dir.join("podcast.json"),
            &serde_json::to_vec_pretty(&json)?,
        )
        .await?;
        if !tokio::fs::try_exists(podcast_dir.join("uguisu.json"))
            .await
            .unwrap_or(false)
        {
            self.write_podcast_settings(podcast_id).await?;
        }

        // 6. Cover art, best effort.
        if let Some(image_url) = &parsed.image_url
            && (podcast.cover_file.is_none() || podcast.image_url.as_ref() != Some(image_url))
            && let Err(e) = self
                .fetch_cover(podcast_id, &podcast_dir, image_url, &feed_url)
                .await
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
        feed_url: &str,
    ) -> Result<()> {
        let mut url = url::Url::parse(image_url).context("invalid cover URL")?;
        check_url(&url, self.inner.cfg.allow_private_networks)?;
        lend_credentials(&mut url, feed_url);
        let request = self.inner.client.get(url).timeout(Duration::from_secs(120));
        let mut response = self.send(request).await?.error_for_status()?;
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
        self.write_podcast_settings(podcast_id).await
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
        self.sync_search_index().await?;
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
    /// Sets or clears the start date of a podcast. Clearing it, or moving it back, lets the
    /// episodes it held back be downloaded; files already archived are never removed.
    pub async fn set_archive_from(&self, podcast_id: i64, archive_from: Option<i64>) -> Result<()> {
        sqlx::query("UPDATE podcasts SET archive_from = ? WHERE id = ?")
            .bind(archive_from)
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        self.write_podcast_settings(podcast_id).await?;
        self.inner.wake_downloader.notify_one();
        Ok(())
    }

    pub async fn regenerate_token(&self, podcast_id: i64) -> Result<()> {
        sqlx::query("UPDATE podcasts SET feed_token = ? WHERE id = ?")
            .bind(random_token(24))
            .bind(podcast_id)
            .execute(self.pool())
            .await?;
        self.write_podcast_settings(podcast_id).await
    }

    // ── Downloads ──────────────────────────────────────────────────────────────────────────

    async fn pending_downloads(&self) -> Result<Vec<PendingDownload>> {
        Ok(sqlx::query_as(
            "SELECT e.*, p.dir AS podcast_dir, p.feed_url FROM episodes e \
             JOIN podcasts p ON p.id = e.podcast_id \
             WHERE e.enclosure_url IS NOT NULL AND e.download_attempts < ? AND e.next_attempt_at <= ? \
             AND (p.archive_from IS NULL OR e.published_at IS NULL \
               OR e.published_at >= p.archive_from) \
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
            "SELECT a.id, a.episode_id, a.kind, a.source_key, a.source_url, a.attempts, \
             e.dir AS episode_dir, p.dir AS podcast_dir, p.feed_url \
             FROM attachments a JOIN episodes e ON e.id = a.episode_id \
             JOIN podcasts p ON p.id = e.podcast_id \
             WHERE a.status != 'ok' AND a.attempts < ? AND a.next_attempt_at <= ? \
             AND (p.archive_from IS NULL OR e.published_at IS NULL \
               OR e.published_at >= p.archive_from) LIMIT 50",
        )
        .bind(MAX_DOWNLOAD_ATTEMPTS)
        .bind(now())
        .fetch_all(self.pool())
        .await?)
    }

    async fn download_attachment(&self, pending: PendingAttachment) {
        let outcome = match self.fetch_attachment(&pending).await {
            Ok(()) => {
                if pending.kind == "transcript"
                    && let Err(e) = self.index_transcript(pending.episode_id).await
                {
                    tracing::warn!("cannot index a transcript: {e:#}");
                }
                self.write_episode_json(pending.episode_id).await
            }
            Err(e) => {
                tracing::warn!(url = %pending.source_key, "attachment download failed: {e:#}");
                let host_break = host_break(&e);
                let delay =
                    host_break.unwrap_or(300 * 4i64.pow(pending.attempts.clamp(0, 6) as u32));
                sqlx::query(
                    "UPDATE attachments SET attempts = attempts + ?, error = ?, \
                     next_attempt_at = ? WHERE id = ?",
                )
                .bind(host_break.is_none())
                .bind(format!("{e:#}"))
                .bind(now() + delay)
                .bind(pending.id)
                .execute(self.pool())
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
            }
        };
        if let Err(e) = outcome {
            tracing::error!("cannot record attachment result: {e:#}");
        }
    }

    async fn fetch_attachment(&self, pending: &PendingAttachment) -> Result<()> {
        self.ensure_space(0)?;
        let mut url = url::Url::parse(&pending.source_url).context("invalid address")?;
        check_url(&url, self.inner.cfg.allow_private_networks)?;
        lend_credentials(&mut url, &pending.feed_url);
        let request = self.inner.client.get(url).timeout(Duration::from_secs(120));
        let mut response = self.send(request).await?;
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

        // Show notes also embed things that are not pictures, such as counting pixels that
        // answer with a web page.
        if pending.kind == "notes-image" && !mime.starts_with("image/") {
            bail!("not an image ({mime})");
        }

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
            "{}/{file_name}",
            archive::metadata_dir(&pending.podcast_dir, &pending.episode_dir)
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

    /// Writes the checksum file of a media file, in the format of `b3sum`, so that the archive
    /// can be checked with `b3sum -c` without Uguisu.
    pub(crate) async fn write_checksum(&self, media_path: &str, hash: &str) -> Result<()> {
        let (checksum, named) = archive::checksum_location(media_path);
        write_atomic(
            &self.archive_path(&checksum),
            format!("{hash}  {named}\n").as_bytes(),
        )
        .await
    }

    /// Moves a media file from `episodes/` to `<metadata>/versions/v<n>.<ext>`, with its
    /// checksum, and records the new place.
    async fn set_aside_as_version(
        &self,
        id: i64,
        path: &str,
        hash: &str,
        metadata: &str,
    ) -> Result<()> {
        let ext = path.rsplit_once('.').map_or("bin", |(_, ext)| ext);
        let versions = format!("{metadata}/versions");
        tokio::fs::create_dir_all(self.archive_path(&versions)).await?;
        let mut number = 1;
        let target = loop {
            let candidate = format!("{versions}/v{number}.{ext}");
            let in_use: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM media_files WHERE path = ?)")
                    .bind(&candidate)
                    .fetch_one(self.pool())
                    .await?;
            if !in_use && !tokio::fs::try_exists(self.archive_path(&candidate)).await? {
                break candidate;
            }
            number += 1;
        };
        let (old_checksum, _) = archive::checksum_location(path);
        if tokio::fs::try_exists(self.archive_path(path)).await? {
            tokio::fs::rename(self.archive_path(path), self.archive_path(&target)).await?;
        }
        sqlx::query("UPDATE media_files SET path = ? WHERE id = ?")
            .bind(&target)
            .bind(id)
            .execute(self.pool())
            .await?;
        self.write_checksum(&target, hash).await?;
        let _ = tokio::fs::remove_file(self.archive_path(&old_checksum)).await;
        Ok(())
    }

    async fn download_one(&self, pending: PendingDownload) {
        let episode = &pending.episode;
        self.inner.downloading.lock().unwrap().insert(episode.id);
        let result = self
            .download(episode, &pending.podcast_dir, &pending.feed_url)
            .await;
        self.inner.downloading.lock().unwrap().remove(&episode.id);

        let outcome = match result {
            Ok(()) => {
                tracing::info!(episode = %episode.title, "archived");
                if let Err(e) = self.write_episode_json(episode.id).await {
                    tracing::error!(episode = %episode.title, "cannot write episode.json: {e:#}");
                }
                sqlx::query("UPDATE episodes SET download_error = NULL, refetch = 0 WHERE id = ?")
                    .bind(episode.id)
                    .execute(self.pool())
                    .await
            }
            Err(e) => {
                tracing::warn!(episode = %episode.title, "download failed: {e:#}");
                // Back off: 5 min, 20 min, 80 min, ...
                let host_break = host_break(&e);
                let delay = host_break
                    .unwrap_or(300 * 4i64.pow(episode.download_attempts.clamp(0, 6) as u32));
                sqlx::query(
                    "UPDATE episodes SET download_attempts = download_attempts + ?, \
                     download_error = ?, next_attempt_at = ? WHERE id = ?",
                )
                .bind(host_break.is_none())
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

    /// Downloads `url` into `part` and returns hash, size and content type of the complete file.
    ///
    /// An interrupted download is continued instead of started again, within this call and at
    /// the next attempt: the partial file stays on disk together with the validator (ETag or
    /// Last-Modified) the server gave for it, and the rest is requested with `Range` and
    /// `If-Range`. A server that does not support this, or whose file has changed, answers with
    /// the whole file, and the download starts over.
    async fn fetch_media(&self, url: &url::Url, part: &Path) -> Result<(String, u64, String)> {
        let note = part.with_extension("part.json");
        let source = redact(without_query(url.as_str()));
        let mut resumes = 0;
        loop {
            // What is already there, if it belongs to this source and can be continued.
            let kept: Option<(u64, String)> = async {
                let stored: serde_json::Value =
                    serde_json::from_slice(&tokio::fs::read(&note).await.ok()?).ok()?;
                let size = tokio::fs::metadata(part).await.ok()?.len();
                (stored["source"].as_str() == Some(source.as_str()) && size > 0)
                    .then(|| Some((size, stored["validator"].as_str()?.to_string())))?
            }
            .await;

            // Not even asked for while the disk is full.
            self.ensure_space(0)?;
            let mut request = self.inner.client.get(url.clone());
            if let Some((size, validator)) = &kept {
                request = request
                    .header(reqwest::header::RANGE, format!("bytes={size}-"))
                    .header(reqwest::header::IF_RANGE, validator);
            }
            let mut response = self.send(request).await?;
            let continued = match (&kept, response.status()) {
                (Some((size, _)), reqwest::StatusCode::PARTIAL_CONTENT)
                    if header(&response, reqwest::header::CONTENT_RANGE)
                        .is_some_and(|range| range.starts_with(&format!("bytes {size}-"))) =>
                {
                    true
                }
                // The partial file is longer than the file on the server: start over.
                (Some(_), reqwest::StatusCode::RANGE_NOT_SATISFIABLE) => {
                    let _ = tokio::fs::remove_file(&note).await;
                    continue;
                }
                (_, status) if status == reqwest::StatusCode::OK => false,
                (_, status) => bail!("the media server answered {status}"),
            };
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
            // With the size known, a file that will not fit is not begun.
            self.ensure_space(response.content_length().unwrap_or(0))?;

            let mut hasher = blake3::Hasher::new();
            let mut size = 0u64;
            let mut file = if continued {
                // The hash covers the whole file, so the part on disk is read once more.
                let mut existing = tokio::fs::File::open(part).await?;
                let mut buffer = vec![0u8; 1 << 16];
                loop {
                    let read = tokio::io::AsyncReadExt::read(&mut existing, &mut buffer).await?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..read]);
                    size += read as u64;
                }
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(part)
                    .await?
            } else {
                // Only a strong validator guarantees that the rest fits the part on disk.
                let validator = header(&response, reqwest::header::ETAG)
                    .filter(|etag| !etag.starts_with("W/"))
                    .or_else(|| header(&response, reqwest::header::LAST_MODIFIED));
                match validator {
                    Some(validator) => {
                        let stored =
                            serde_json::json!({ "source": source, "validator": validator });
                        write_atomic(&note, &serde_json::to_vec(&stored)?).await?;
                    }
                    None => {
                        let _ = tokio::fs::remove_file(&note).await;
                    }
                }
                tokio::fs::File::create(part).await?
            };
            let before = size;

            let streamed: Result<()> = async {
                while let Some(chunk) = response.chunk().await.context("download interrupted")? {
                    hasher.update(&chunk);
                    file.write_all(&chunk).await?;
                    size += chunk.len() as u64;
                }
                file.sync_all().await?;
                Ok(())
            }
            .await;
            drop(file);
            match streamed {
                Ok(()) if size == 0 => {
                    let _ = tokio::fs::remove_file(part).await;
                    let _ = tokio::fs::remove_file(&note).await;
                    bail!("the media server sent an empty file");
                }
                Ok(()) => {
                    let _ = tokio::fs::remove_file(&note).await;
                    return Ok((hasher.finalize().to_hex().to_string(), size, content_type));
                }
                Err(error) => {
                    let resumable = tokio::fs::try_exists(&note).await.unwrap_or(false);
                    if !resumable {
                        let _ = tokio::fs::remove_file(part).await;
                        return Err(error);
                    }
                    // Continue right away while the transfer makes progress; otherwise the part
                    // stays for the next attempt.
                    if size == before || resumes >= MAX_RESUMES {
                        return Err(error);
                    }
                    resumes += 1;
                    tracing::debug!(url = %source, at = size, "download interrupted, continuing");
                }
            }
        }
    }

    async fn download(&self, episode: &Episode, podcast_dir: &str, feed_url: &str) -> Result<()> {
        let source = episode
            .media_url
            .as_deref()
            .or(episode.enclosure_url.as_deref())
            .context("episode has no enclosure")?;
        let mut url = url::Url::parse(source).context("invalid enclosure URL")?;
        check_url(&url, self.inner.cfg.allow_private_networks)?;
        lend_credentials(&mut url, feed_url);

        let metadata = archive::metadata_dir(podcast_dir, &episode.dir);
        let dir = self.archive_path(&metadata);
        tokio::fs::create_dir_all(&dir).await?;
        let part = dir.join("download.part");
        let (hash, size, content_type) = self.fetch_media(&url, &part).await?;

        let declared = if episode.media_url.is_some() {
            episode.media_type.as_deref()
        } else {
            episode.enclosure_type.as_deref()
        };
        let declared = declared
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

        let existing: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, path, blake3, status FROM media_files WHERE episode_id = ? ORDER BY id",
        )
        .bind(episode.id)
        .fetch_all(self.pool())
        .await?;
        if existing
            .iter()
            .any(|(_, _, b, status)| *b == hash && status == "ok")
        {
            // The source moved the file without changing it.
            let _ = tokio::fs::remove_file(&part).await;
            return Ok(());
        }
        let relative = archive::media_path(podcast_dir, &episode.dir, ext);
        let target = self.archive_path(&relative);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let current_prefix = format!("{podcast_dir}/episodes/");
        if existing.iter().any(|(_, _, _, s)| s == "ok") {
            // The source replaced the file. `episodes/` holds the newest one; the files there
            // so far become earlier versions under `metadata/<episode>/versions/`.
            for (id, path, recorded, _) in &existing {
                if !path.starts_with(&current_prefix) {
                    continue;
                }
                self.set_aside_as_version(*id, path, recorded, &metadata)
                    .await?;
            }
        } else if let Some((_, _, recorded, _)) = existing
            .iter()
            .find(|(_, path, _, status)| status == "corrupt" && *path == relative)
            && *recorded != hash
            && tokio::fs::try_exists(&target).await.unwrap_or(false)
        {
            // A damaged copy is only replaced by the very file it once was. If the source now
            // offers something else, the damaged copy may still be the only trace of the
            // original, so it is set aside instead of overwritten.
            let aside = format!("damaged-{}.{ext}", &recorded[..recorded.len().min(8)]);
            tracing::warn!(
                episode = %episode.title,
                kept = %aside,
                "the source no longer offers the file that was archived; the damaged copy is kept"
            );
            tokio::fs::rename(&target, dir.join(aside)).await?;
        }
        tokio::fs::rename(&part, &target).await?;
        self.write_checksum(&relative, &hash).await?;

        let now = now();
        sqlx::query(
            "INSERT INTO media_files (episode_id, path, size, blake3, mime, source_url, \
             downloaded_at, verified_at, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'ok') \
             ON CONFLICT (path) DO UPDATE SET size = excluded.size, blake3 = excluded.blake3, \
             mime = excluded.mime, source_url = excluded.source_url, \
             downloaded_at = excluded.downloaded_at, verified_at = excluded.verified_at, status = 'ok'",
        )
        .bind(episode.id)
        .bind(&relative)
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
            if let Err(e) = this.sync_search_index().await {
                tracing::error!("rebuilding the search index failed: {e:#}");
            }
        });
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = this.backup_if_due().await {
                    tracing::error!("copying the database failed: {e:#}");
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
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
            let listed = |files: &[String]| -> Vec<String> {
                files.iter().take(MAX_NOTIFIED_FILES).cloned().collect()
            };
            self.notify(
                "archive.damaged",
                format!(
                    "{} damaged and {} missing files in the archive",
                    report.corrupt.len(),
                    report.missing.len()
                ),
                serde_json::json!({
                    "damaged": report.corrupt.len(),
                    "missing": report.missing.len(),
                    "damaged_files": listed(&report.corrupt),
                    "missing_files": listed(&report.missing),
                }),
            )
            .await;
        }
        Ok(report)
    }

    // ── Notifications ──────────────────────────────────────────────────────────────────────

    /// Whether an address for notifications is configured, and which host it names.
    pub fn webhook_host(&self) -> Option<String> {
        let url = self.inner.cfg.webhook_url.as_deref()?;
        Some(match url::Url::parse(url) {
            Ok(parsed) => parsed.host_str().unwrap_or("?").to_string(),
            Err(_) => "?".to_string(),
        })
    }

    /// Sends a notification that says nothing happened, to try the configured address.
    pub async fn send_test_notification(&self) -> Result<()> {
        self.post_notification(
            "test",
            "This is a test notification from Uguisu.".to_string(),
            serde_json::json!({}),
        )
        .await
    }

    async fn notify_about_feed(&self, podcast_id: i64, event: &str, error: Option<&str>) {
        let Ok(Some(podcast)) = self.podcast(podcast_id).await else {
            return;
        };
        let text = match error {
            Some(error) => format!(
                "The feed of \"{}\" has failed {FEED_FAILING_AFTER} times in a row: {error}",
                podcast.title
            ),
            None => format!("The feed of \"{}\" works again.", podcast.title),
        };
        let details = serde_json::json!({
            "podcast": {
                "id": podcast.id,
                "title": podcast.title,
                "feed": redact(&podcast.feed_url),
            },
            "error": error,
        });
        self.notify(event, text, details).await;
    }

    /// Sends a notification if an address is configured. A notification that cannot be
    /// delivered is logged and otherwise ignored: it must never stop the work it reports on.
    async fn notify(&self, event: &str, text: String, details: serde_json::Value) {
        if self.inner.cfg.webhook_url.is_none() {
            return;
        }
        if let Err(e) = self.post_notification(event, text, details).await {
            tracing::warn!(event, "notification could not be delivered: {e:#}");
        }
    }

    async fn post_notification(
        &self,
        event: &str,
        text: String,
        details: serde_json::Value,
    ) -> Result<()> {
        let url = self
            .inner
            .cfg
            .webhook_url
            .as_deref()
            .context("no address for notifications is configured")?;
        let mut body = serde_json::json!({
            "event": event,
            "time": crate::util::format_utc(now(), "%Y-%m-%dT%H:%M:%SZ"),
            "software": concat!("uguisu ", env!("CARGO_PKG_VERSION")),
            "text": text,
        });
        if let (Some(body), serde_json::Value::Object(details)) = (body.as_object_mut(), details) {
            body.extend(details);
        }
        // The address may carry a secret; errors name the host only.
        let response = self
            .inner
            .webhook_client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body)?)
            .send()
            .await
            .map_err(|e| {
                // The innermost cause says what went wrong ("connection refused").
                let e = e.without_url();
                let mut cause: &dyn std::error::Error = &e;
                while let Some(next) = cause.source() {
                    cause = next;
                }
                anyhow::anyhow!("the receiver could not be reached ({cause})")
            })?;
        if !response.status().is_success() {
            bail!("the receiver answered {}", response.status());
        }
        Ok(())
    }
}
