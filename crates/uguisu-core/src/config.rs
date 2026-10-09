//! Configuration types for discovery, networking, storage and the feed
//! engine.
//!
//! Configuration is loaded from built-in defaults and environment variables
//! (`docs/CONFIGURATION.md`); the config file and the settings table are
//! later layers. Every value records its origin so `uguisu config validate`
//! can explain where it came from.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::archive::{PathProfile, TagMode, VerifyDepth};
use crate::download::Priority;
use crate::secret::Secret;

/// Where a configuration value came from.
///
/// Declaration order *is* precedence, lowest first, and [`Origin::rank`]
/// makes that usable. An explicitly set environment variable outranks a
/// stored setting on purpose (ADR 0028): the operator who wrote it into a
/// unit file or a compose file has to be able to rely on it, and a value
/// the API would silently ignore is worse than one it refuses to take.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Built-in default.
    Default,
    /// A row in the `settings` table.
    Settings,
    /// An environment variable that was actually set.
    Env,
    /// A command-line flag.
    Cli,
}

impl Origin {
    /// Stable name, as the API and `config validate` spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Settings => "settings",
            Self::Env => "env",
            Self::Cli => "cli",
        }
    }

    /// Precedence, lowest first.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::Settings => 1,
            Self::Env => 2,
            Self::Cli => 3,
        }
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The layers a value may come from, consulted in precedence order.
///
/// One type, so that the merge rule exists once: `Config::from_layers` and
/// `DiscoveryConfig::from_layers` both ask this and neither decides
/// anything. It also enforces the one rule a parser cannot — that a key
/// marked `persistable: false` is never taken from the database, whatever
/// is sitting in the table.
pub struct Layers<'a> {
    stored: BTreeMap<String, String>,
    env: Box<dyn Fn(&str) -> Option<String> + 'a>,
}

impl<'a> Layers<'a> {
    /// Environment only, which is what `from_lookup`
    /// still means.
    pub fn env_only(env: impl Fn(&str) -> Option<String> + 'a) -> Self {
        Self {
            stored: BTreeMap::new(),
            env: Box::new(env),
        }
    }

    /// Stored settings under the environment.
    ///
    /// The stored layer is a map rather than a closure because it *is* a
    /// table and can be enumerated — which is what lets a key nothing
    /// reads any more still be reported. The environment cannot be
    /// enumerated safely (Uguisu has no business listing a user's whole
    /// environment), so it stays a lookup over the keys we know.
    pub fn new(
        stored: BTreeMap<String, String>,
        env: impl Fn(&str) -> Option<String> + 'a,
    ) -> Self {
        Self {
            stored,
            env: Box::new(env),
        }
    }

    /// Both layers, flattened to the keys Uguisu knows.
    ///
    /// Taken once, so that a `Config` can remember what it was built from
    /// and re-assemble itself when a stored value changes — without
    /// reading the environment a second time and getting a different
    /// answer.
    fn snapshot(&self) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        let trimmed = |v: Option<String>| -> Option<String> {
            v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
        };
        let stored: BTreeMap<String, String> = self
            .stored
            .iter()
            .filter_map(|(k, v)| trimmed(Some(v.clone())).map(|v| (k.clone(), v)))
            .collect();
        let mut env = BTreeMap::new();
        for key in crate::settings::all() {
            if let Some(v) = trimmed((self.env)(key)) {
                env.insert(key.to_owned(), v);
            }
        }
        (stored, env)
    }

    /// The effective value of `key` and where it came from.
    ///
    /// An empty or whitespace-only value counts as unset at every layer,
    /// which is what makes `UGUISU_APPLE_LANG=` in a compose file mean
    /// "leave it alone" rather than "set it to nothing".
    fn resolve(&self, key: &str) -> Option<(String, Origin)> {
        let trimmed = |v: Option<String>| -> Option<String> {
            v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
        };
        if let Some(v) = trimmed((self.env)(key)) {
            return Some((v, Origin::Env));
        }
        if crate::settings::is_persistable(key)
            && let Some(v) = trimmed(self.stored.get(key).cloned())
        {
            return Some((v, Origin::Settings));
        }
        None
    }
}

/// One key, as `config validate` and the settings API report it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct KeyDescription {
    /// The `UGUISU_*` name.
    pub key: String,
    /// The value in force, rendered; secrets are redacted.
    pub value: String,
    /// Which layer that value came from.
    pub origin: Origin,
    /// The stored value, if there is one — **even when it is not the one
    /// in force**, because "I set that and nothing happened" is the
    /// question this field exists to answer.
    pub stored: Option<String>,
    /// Whether a stored value exists but something above it wins.
    pub pinned: bool,
    /// Whether this key may be stored at all.
    pub persistable: bool,
    /// Whether a change takes effect without a restart.
    pub live: bool,
}

/// Error produced when an environment variable holds an invalid value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid value for {key}: {message}")]
pub struct ConfigError {
    /// The environment variable name.
    pub key: String,
    /// What was wrong with it.
    pub message: String,
}

/// Apple iTunes Search provider settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleConfig {
    /// Whether the provider is queried. Default: enabled (no key needed).
    pub enabled: bool,
    /// Storefront country (ISO 3166-1 alpha-2). Default `US`.
    pub country: String,
    /// Optional `lang` parameter; Apple accepts only `en_us` and `ja_jp`.
    pub lang: Option<String>,
    /// API base URL (overridable for tests and mirrors).
    pub base_url: String,
}

/// Podcast Index provider settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodcastIndexConfig {
    /// Whether the provider is queried. Default: enabled only when a key is set.
    pub enabled: bool,
    /// The user's API key (never shipped with Uguisu).
    pub key: Option<Secret<String>>,
    /// The user's API secret.
    pub secret: Option<Secret<String>>,
    /// API base URL.
    pub base_url: String,
}

impl PodcastIndexConfig {
    /// True when both key and secret are configured.
    pub fn has_credentials(&self) -> bool {
        self.key.is_some() && self.secret.is_some()
    }
}

/// gpodder.net directory settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpodderNetConfig {
    /// Whether the provider is queried. Default: disabled (opt-in).
    pub enabled: bool,
    /// API base URL.
    pub base_url: String,
}

/// Search orchestration settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchConfig {
    /// First result snapshot is emitted no later than this after the search starts.
    pub soft_deadline: Duration,
    /// The search is closed after this; slow providers are reported as timed out.
    pub hard_deadline: Duration,
    /// Per-provider request timeout.
    pub provider_timeout: Duration,
    /// Default number of results returned.
    pub default_limit: usize,
    /// Upper bound for a requested limit.
    pub max_limit: usize,
}

/// Discovery cache settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    /// TTL for search results per provider and query.
    pub search_ttl: Duration,
    /// TTL for lookups by provider reference.
    pub lookup_ttl: Duration,
    /// Maximum number of cached entries.
    pub max_entries: u64,
}

/// Outbound network settings shared by every HTTP client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    /// Host names that may resolve to private addresses (e.g. a LAN feed mirror).
    pub allow_private_hosts: Vec<String>,
    /// Override for the `User-Agent` header.
    pub user_agent: Option<String>,
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// Total request timeout.
    pub request_timeout: Duration,
}

/// Everything the discovery engine needs to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryConfig {
    /// Apple provider.
    pub apple: AppleConfig,
    /// Podcast Index provider.
    pub podcastindex: PodcastIndexConfig,
    /// gpodder.net provider.
    pub gpoddernet: GpodderNetConfig,
    /// Search orchestration.
    pub search: SearchConfig,
    /// Result cache.
    pub cache: CacheConfig,
    /// Networking.
    pub network: NetworkConfig,
    /// Origin of every value that can be set through the environment.
    origins: BTreeMap<&'static str, Origin>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            apple: AppleConfig {
                enabled: true,
                country: "US".to_owned(),
                lang: None,
                base_url: "https://itunes.apple.com".to_owned(),
            },
            podcastindex: PodcastIndexConfig {
                enabled: false,
                key: None,
                secret: None,
                base_url: "https://api.podcastindex.org/api/1.0".to_owned(),
            },
            gpoddernet: GpodderNetConfig {
                enabled: false,
                base_url: "https://gpodder.net".to_owned(),
            },
            search: SearchConfig {
                soft_deadline: Duration::from_secs(2),
                hard_deadline: Duration::from_secs(8),
                provider_timeout: Duration::from_secs(6),
                default_limit: 25,
                max_limit: 100,
            },
            cache: CacheConfig {
                search_ttl: Duration::from_secs(900),
                lookup_ttl: Duration::from_secs(86_400),
                max_entries: 10_000,
            },
            network: NetworkConfig {
                allow_private_hosts: Vec::new(),
                user_agent: None,
                connect_timeout: Duration::from_secs(10),
                request_timeout: Duration::from_secs(30),
            },
            origins: BTreeMap::new(),
        }
    }
}

/// Environment variable names understood by [`DiscoveryConfig::from_env`].
pub mod env_keys {
    /// `true`/`false`.
    pub const APPLE_ENABLED: &str = "UGUISU_DISCOVERY_APPLE_ENABLED";
    /// ISO 3166-1 alpha-2 storefront.
    pub const APPLE_COUNTRY: &str = "UGUISU_APPLE_COUNTRY";
    /// `en_us` or `ja_jp`.
    pub const APPLE_LANG: &str = "UGUISU_APPLE_LANG";
    /// Base URL override.
    pub const APPLE_BASE_URL: &str = "UGUISU_DISCOVERY_APPLE_BASE_URL";
    /// `true`/`false`; defaults to "key present".
    pub const PODCASTINDEX_ENABLED: &str = "UGUISU_DISCOVERY_PODCASTINDEX_ENABLED";
    /// API key.
    pub const PODCASTINDEX_KEY: &str = "UGUISU_PODCASTINDEX_KEY";
    /// API secret.
    pub const PODCASTINDEX_SECRET: &str = "UGUISU_PODCASTINDEX_SECRET";
    /// Base URL override.
    pub const PODCASTINDEX_BASE_URL: &str = "UGUISU_DISCOVERY_PODCASTINDEX_BASE_URL";
    /// `true`/`false`.
    pub const GPODDERNET_ENABLED: &str = "UGUISU_DISCOVERY_GPODDERNET_ENABLED";
    /// Base URL override.
    pub const GPODDERNET_BASE_URL: &str = "UGUISU_DISCOVERY_GPODDERNET_BASE_URL";
    /// Milliseconds.
    pub const SOFT_DEADLINE_MS: &str = "UGUISU_DISCOVERY_SOFT_DEADLINE_MS";
    /// Milliseconds.
    pub const HARD_DEADLINE_MS: &str = "UGUISU_DISCOVERY_HARD_DEADLINE_MS";
    /// Milliseconds.
    pub const PROVIDER_TIMEOUT_MS: &str = "UGUISU_DISCOVERY_PROVIDER_TIMEOUT_MS";
    /// Default result limit.
    pub const LIMIT: &str = "UGUISU_DISCOVERY_LIMIT";
    /// Seconds.
    pub const CACHE_SEARCH_TTL_SECS: &str = "UGUISU_DISCOVERY_CACHE_SEARCH_TTL_SECS";
    /// Seconds.
    pub const CACHE_LOOKUP_TTL_SECS: &str = "UGUISU_DISCOVERY_CACHE_LOOKUP_TTL_SECS";
    /// Entry count.
    pub const CACHE_MAX_ENTRIES: &str = "UGUISU_DISCOVERY_CACHE_MAX_ENTRIES";
    /// Comma-separated host names.
    pub const HTTP_ALLOW_PRIVATE_HOSTS: &str = "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS";
    /// User agent override.
    pub const HTTP_USER_AGENT: &str = "UGUISU_HTTP_USER_AGENT";
    /// Milliseconds.
    pub const HTTP_CONNECT_TIMEOUT_MS: &str = "UGUISU_HTTP_CONNECT_TIMEOUT_MS";
    /// Milliseconds.
    pub const HTTP_REQUEST_TIMEOUT_MS: &str = "UGUISU_HTTP_REQUEST_TIMEOUT_MS";
    /// Data directory (database, lock file).
    pub const DATA_DIR: &str = "UGUISU_DATA_DIR";
    /// Bytes.
    pub const FEED_MAX_BYTES: &str = "UGUISU_FEED_MAX_BYTES";
    /// Item count.
    pub const FEED_MAX_ITEMS: &str = "UGUISU_FEED_MAX_ITEMS";
    /// Parallel refreshes for `refresh --all`.
    pub const FEED_REFRESH_CONCURRENCY: &str = "UGUISU_FEED_REFRESH_CONCURRENCY";
    /// Milliseconds for one whole refresh.
    pub const FEED_REFRESH_TIMEOUT_MS: &str = "UGUISU_FEED_REFRESH_TIMEOUT_MS";
    /// Fetch log rows kept per podcast.
    pub const FEED_RETAIN_FETCHES: &str = "UGUISU_FEED_RETAIN_FETCHES";
    /// Complete fetches without an item before it counts as removed.
    pub const FEED_REMOVAL_STREAK: &str = "UGUISU_FEED_REMOVAL_STREAK";
    /// Percent of present episodes that may vanish in one fetch before removal detection pauses.
    pub const FEED_MASS_REMOVAL_GUARD_PERCENT: &str = "UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT";
    /// Whether `uguisu serve` refreshes feeds on its own.
    pub const FEED_SCHEDULER: &str = "UGUISU_FEED_SCHEDULER";
    /// Seconds between automatic refreshes of one podcast.
    pub const FEED_REFRESH_INTERVAL_SECS: &str = "UGUISU_FEED_REFRESH_INTERVAL_SECS";
    /// Media directory (downloaded files); default `<data dir>/media`.
    pub const MEDIA_DIR: &str = "UGUISU_MEDIA_DIR";
    /// Parallel downloads overall.
    pub const DOWNLOAD_GLOBAL_CONCURRENCY: &str = "UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY";
    /// Parallel downloads per host (`scheme://host:port`).
    pub const DOWNLOAD_PER_HOST_CONCURRENCY: &str = "UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY";
    /// Attempt budget per job.
    pub const DOWNLOAD_MAX_ATTEMPTS: &str = "UGUISU_DOWNLOAD_MAX_ATTEMPTS";
    /// Milliseconds before the second attempt; doubles each time.
    pub const DOWNLOAD_BACKOFF_BASE_MS: &str = "UGUISU_DOWNLOAD_BACKOFF_BASE_MS";
    /// Milliseconds; longest wait between attempts (also caps `Retry-After`).
    pub const DOWNLOAD_BACKOFF_MAX_MS: &str = "UGUISU_DOWNLOAD_BACKOFF_MAX_MS";
    /// Milliseconds without a body chunk before an attempt fails.
    pub const DOWNLOAD_IDLE_TIMEOUT_MS: &str = "UGUISU_DOWNLOAD_IDLE_TIMEOUT_MS";
    /// Milliseconds between progress records and events per job.
    pub const DOWNLOAD_PROGRESS_INTERVAL_MS: &str = "UGUISU_DOWNLOAD_PROGRESS_INTERVAL_MS";
    /// Bytes; largest media file accepted.
    pub const DOWNLOAD_MAX_BYTES: &str = "UGUISU_DOWNLOAD_MAX_BYTES";
    /// Bytes that must stay free on the media file system.
    pub const DOWNLOAD_MIN_FREE_BYTES: &str = "UGUISU_DOWNLOAD_MIN_FREE_BYTES";
    /// Milliseconds workers get to finish or park their jobs on shutdown.
    pub const DOWNLOAD_SHUTDOWN_GRACE_MS: &str = "UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS";
    /// Path template for archived files (ADR 0009 grammar).
    pub const ARCHIVE_TEMPLATE: &str = "UGUISU_ARCHIVE_TEMPLATE";
    /// Sanitization profile: `portable`, `posix` or `windows`.
    pub const ARCHIVE_PATH_PROFILE: &str = "UGUISU_ARCHIVE_PATH_PROFILE";
    /// Whether a finished download is verified right away.
    pub const ARCHIVE_VERIFY_ON_COMPLETION: &str = "UGUISU_ARCHIVE_VERIFY_ON_COMPLETION";
    /// Depth of that check: `existence`, `light` or `full`.
    pub const ARCHIVE_VERIFY_DEPTH: &str = "UGUISU_ARCHIVE_VERIFY_DEPTH";
    /// Whether discovered episodes are queued automatically.
    pub const ARCHIVE_AUTO_DOWNLOAD: &str = "UGUISU_ARCHIVE_AUTO_DOWNLOAD";
    /// Newest episodes an automatic evaluation may queue at once.
    pub const ARCHIVE_MAX_BACKLOG: &str = "UGUISU_ARCHIVE_MAX_BACKLOG";
    /// Days; older episodes are not queued automatically.
    pub const ARCHIVE_MAX_AGE_DAYS: &str = "UGUISU_ARCHIVE_MAX_AGE_DAYS";
    /// Priority for automatically queued jobs: `low`, `normal`, `high`.
    pub const ARCHIVE_PRIORITY: &str = "UGUISU_ARCHIVE_PRIORITY";
    /// Whether a portable `<media file>.json` sidecar is written.
    pub const ARCHIVE_SIDECARS: &str = "UGUISU_ARCHIVE_SIDECARS";
    /// Whether per-podcast `manifest.sha256` files are maintained.
    pub const ARCHIVE_MANIFESTS: &str = "UGUISU_ARCHIVE_MANIFESTS";
    /// Whether podcast artwork is fetched automatically.
    pub const ARCHIVE_ARTWORK_FETCH: &str = "UGUISU_ARCHIVE_ARTWORK_FETCH";
    /// Bytes; largest artwork image accepted.
    pub const ARCHIVE_ARTWORK_MAX_BYTES: &str = "UGUISU_ARCHIVE_ARTWORK_MAX_BYTES";
    /// Default tag-writing mode: `fill_missing` or `sync`.
    pub const ARCHIVE_TAG_MODE: &str = "UGUISU_ARCHIVE_TAG_MODE";
    /// Percent (0-100); how confident an import match must be.
    pub const ARCHIVE_IMPORT_MATCH_THRESHOLD: &str = "UGUISU_ARCHIVE_IMPORT_MATCH_THRESHOLD";
    /// Seconds between maintenance passes.
    pub const MAINTENANCE_INTERVAL_SECS: &str = "UGUISU_MAINTENANCE_INTERVAL_SECS";
    /// Days of event history kept (0 = no age limit).
    pub const EVENTS_RETAIN_DAYS: &str = "UGUISU_EVENTS_RETAIN_DAYS";
    /// Event rows kept whatever their age (0 = no row limit).
    pub const EVENTS_RETAIN_MAX_ROWS: &str = "UGUISU_EVENTS_RETAIN_MAX_ROWS";

    // The key lists are derived from `crate::settings::SETTINGS`; see
    // `settings::all` and `settings::in_scope`.
}

/// Where persistent state lives (`docs/ARCHITECTURE.md` §3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataConfig {
    /// Directory holding `uguisu.db`, `uguisu.lock` and `uguisu.pid`. `None` means "the
    /// platform data directory", resolved lazily by [`DataConfig::data_dir`].
    pub data_dir: Option<PathBuf>,
    /// Directory holding downloaded media. `None` means `<data dir>/media`;
    /// Docker images set
    /// `/media/podcasts`. Application data and media may live on different
    /// file systems.
    pub media_dir: Option<PathBuf>,
}

impl DataConfig {
    /// File name of the database inside the data directory.
    pub const DATABASE_FILE: &'static str = "uguisu.db";
    /// File name of the single-process lock inside the data directory.
    pub const LOCK_FILE: &'static str = "uguisu.lock";
    /// File name of the lock holder's process id, beside [`Self::LOCK_FILE`].
    pub const PID_FILE: &'static str = "uguisu.pid";

    /// The effective data directory: the configured one or the platform
    /// default (`~/.local/share/uguisu`, `%APPDATA%\uguisu\data`, …).
    pub fn data_dir(&self) -> Result<PathBuf, ConfigError> {
        if let Some(dir) = &self.data_dir {
            return Ok(dir.clone());
        }
        directories::ProjectDirs::from("", "", "uguisu")
            .map(|d| d.data_dir().to_path_buf())
            .ok_or_else(|| ConfigError {
                key: env_keys::DATA_DIR.to_owned(),
                message: "no platform data directory is known for this user; set the data directory explicitly".to_owned(),
            })
    }

    /// Path of the database file.
    pub fn database_path(&self) -> Result<PathBuf, ConfigError> {
        Ok(self.data_dir()?.join(Self::DATABASE_FILE))
    }

    /// Path of the lock file.
    pub fn lock_path(&self) -> Result<PathBuf, ConfigError> {
        Ok(self.data_dir()?.join(Self::LOCK_FILE))
    }

    /// The effective media directory: the configured one or `<data dir>/media`.
    pub fn media_dir(&self) -> Result<PathBuf, ConfigError> {
        if let Some(dir) = &self.media_dir {
            return Ok(dir.clone());
        }
        Ok(self.data_dir()?.join(Self::MEDIA_SUBDIR))
    }
}

impl DataConfig {
    /// Sub-directory of the data directory used for media by default.
    pub const MEDIA_SUBDIR: &'static str = "media";
}

/// Download engine settings (`docs/DOWNLOAD_ENGINE.md` "Configuration").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadConfig {
    /// Parallel downloads overall.
    pub global_concurrency: usize,
    /// Parallel downloads per host.
    pub per_host_concurrency: usize,
    /// Attempt budget per job.
    pub max_attempts: u32,
    /// Delay before the second attempt; doubles each time.
    pub backoff_base: Duration,
    /// Longest wait between attempts; also caps `Retry-After`.
    pub backoff_max: Duration,
    /// Longest wait for the next body chunk.
    pub idle_timeout: Duration,
    /// Minimum interval between progress records and events per job.
    pub progress_interval: Duration,
    /// Largest media file accepted.
    pub max_bytes: u64,
    /// Bytes that must remain free on the media file system after a download.
    pub min_free_bytes: u64,
    /// Time workers get to finish finalization or park their jobs on shutdown.
    pub shutdown_grace: Duration,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            global_concurrency: 3,
            per_host_concurrency: 1,
            max_attempts: 8,
            backoff_base: Duration::from_secs(30),
            backoff_max: Duration::from_secs(6 * 60 * 60),
            idle_timeout: Duration::from_secs(60),
            progress_interval: Duration::from_secs(1),
            max_bytes: 8 * 1024 * 1024 * 1024,
            min_free_bytes: 256 * 1024 * 1024,
            shutdown_grace: Duration::from_secs(15),
        }
    }
}

/// Archive engine settings (`docs/ARCHIVE_ENGINE.md` "Configuration").
///
/// The template and the profile decide where a finished download lands;
/// the policy fields are the **global defaults** that a per-podcast
/// `archive_policies` row may override (ADR 0023).
// Every field here mirrors one environment variable one-to-one, which is
// what makes `config show` and `Config::describe` exhaustive. Folding the
// flags into two-variant enums would break that mapping for no reader's
// benefit.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveConfig {
    /// Path template for archived files (ADR 0009 grammar).
    pub template: String,
    /// Sanitization profile applied to every rendered segment.
    pub path_profile: PathProfile,
    /// Whether a finished download is verified immediately.
    pub verify_on_completion: bool,
    /// How hard that check looks.
    pub verify_depth: VerifyDepth,
    /// Whether discovered episodes are queued without being asked.
    pub auto_download: bool,
    /// Most episodes of one podcast that may be waiting to be archived at
    /// once (0 = no limit).
    pub max_backlog: u32,
    /// Episodes published longer ago than this are left alone (0 = no limit).
    pub max_age_days: u32,
    /// Priority for automatically queued jobs.
    pub priority: Priority,
    /// Whether a portable `<media file>.json` sidecar is written beside
    /// every artifact.
    pub sidecars: bool,
    /// Whether per-podcast `manifest.sha256` files are maintained.
    pub manifests: bool,
    /// Whether podcast artwork is fetched without being asked. Off by
    /// default: an upgrade must not start making requests.
    pub artwork_fetch: bool,
    /// Largest artwork image accepted, in bytes.
    pub artwork_max_bytes: u64,
    /// The tag-writing mode commands use when none is given.
    pub tag_mode: TagMode,
    /// How confident an import match must be, in percent. The score is a
    /// fraction internally; this is an integer so the configuration stays
    /// exactly comparable and round-trips through `config show`.
    pub import_match_threshold: u32,
}

impl ArchiveConfig {
    /// The default template: podcast, year, date-prefixed episode title.
    pub const DEFAULT_TEMPLATE: &'static str =
        "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}";
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            template: Self::DEFAULT_TEMPLATE.to_owned(),
            path_profile: PathProfile::Portable,
            verify_on_completion: true,
            verify_depth: VerifyDepth::Light,
            // Off by default: an update must never start downloading on its
            // own (ADR 0023).
            auto_download: false,
            max_backlog: 3,
            max_age_days: 0,
            priority: Priority::Normal,
            sidecars: true,
            manifests: true,
            // Off by default: an upgrade must not start fetching on its own.
            artwork_fetch: false,
            artwork_max_bytes: 16 * 1024 * 1024,
            tag_mode: TagMode::FillMissing,
            import_match_threshold: 85,
        }
    }
}

/// Parser and ingestion limits (`docs/FEED_ENGINE.md` "Limits").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedLimits {
    /// Maximum feed body in bytes (after decompression).
    pub max_bytes: u64,
    /// Maximum XML nesting depth.
    pub max_depth: usize,
    /// Maximum items parsed per document; the rest is reported as truncated.
    pub max_items: usize,
    /// Maximum bytes kept for long text fields (descriptions).
    pub max_text_bytes: usize,
    /// Maximum bytes kept for short fields (titles, URLs, names).
    pub max_field_bytes: usize,
    /// Maximum enclosures kept per item.
    pub max_enclosures_per_item: usize,
    /// Maximum unmodelled elements kept per item.
    pub max_raw_extensions_per_item: usize,
    /// Maximum malformed items tolerated before parsing stops.
    pub max_malformed_items: usize,
}

impl Default for FeedLimits {
    fn default() -> Self {
        Self {
            max_bytes: 50 * 1024 * 1024,
            max_depth: 64,
            max_items: 50_000,
            max_text_bytes: 64 * 1024,
            max_field_bytes: 4 * 1024,
            max_enclosures_per_item: 16,
            max_raw_extensions_per_item: 64,
            max_malformed_items: 100,
        }
    }
}

/// Feed engine settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedConfig {
    /// Parser limits.
    pub limits: FeedLimits,
    /// Wall-clock budget for one refresh (fetch + parse + persist).
    pub refresh_timeout: Duration,
    /// Parallel refreshes for `refresh --all` and the future scheduler.
    pub refresh_concurrency: usize,
    /// Fetch log rows kept per podcast.
    pub retain_fetches: u32,
    /// Complete fetches without an item before it is reported as removed.
    pub removal_streak: u32,
    /// Fraction of stored episodes that may go missing in one fetch before
    /// removal detection is suppressed for that fetch.
    pub mass_removal_guard_percent: u8,
    /// Consecutive failures after which the podcast status becomes `error`.
    pub error_after_failures: u32,
    /// Whether `uguisu serve` refreshes feeds without being asked. On by
    /// default: a daemon that never refreshes is not an archiver. It
    /// changes nothing about *downloading* — that stays behind the
    /// archive policy, which is off by default (ADR 0023).
    pub scheduler: bool,
    /// How long after a successful refresh the next one is due, for a
    /// podcast that has no interval of its own. A shorter interval the
    /// origin asks for is never honoured; a longer one is (ADR 0027).
    pub refresh_interval: Duration,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            limits: FeedLimits::default(),
            refresh_timeout: Duration::from_secs(60),
            refresh_concurrency: 8,
            retain_fetches: 50,
            removal_streak: 2,
            mass_removal_guard_percent: 50,
            error_after_failures: 5,
            scheduler: true,
            refresh_interval: Duration::from_secs(3600),
        }
    }
}

/// Periodic housekeeping (ADR 0027).
///
/// Its own section rather than a corner of [`FeedConfig`] or
/// [`DataConfig`]: pruning the event log is neither a feed-engine setting
/// nor a question of where files live, and the next piece of housekeeping
/// would have had to pick a wrong home too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceConfig {
    /// How often the scheduler runs the housekeeping pass.
    pub interval: Duration,
    /// Days of event history kept. `0` keeps every event whatever its age
    /// — the row limit is then the only bound.
    pub retain_events_days: u32,
    /// Event rows kept regardless of age, newest first. `0` means no row
    /// limit; with both limits at `0` nothing is ever pruned, which is a
    /// choice an operator may make and Uguisu will not second-guess.
    pub retain_events_max_rows: u64,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(24 * 3600),
            retain_events_days: 30,
            retain_events_max_rows: 100_000,
        }
    }
}

/// The whole application configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Discovery providers, search and networking.
    pub discovery: DiscoveryConfig,
    /// Storage location.
    pub data: DataConfig,
    /// Feed engine.
    pub feed: FeedConfig,
    /// Download engine.
    pub download: DownloadConfig,
    /// Archive engine.
    pub archive: ArchiveConfig,
    /// Periodic housekeeping.
    pub maintenance: MaintenanceConfig,
    /// Origins of the engine keys.
    origins: BTreeMap<&'static str, Origin>,
    /// The stored layer this was built from, whether or not each value is
    /// the effective one. Keeping it is what lets `describe` report a key
    /// as pinned — a stored value the environment shadows is exactly the
    /// case a user needs told about.
    stored: BTreeMap<String, String>,
    /// The environment layer this was built from.
    env: BTreeMap<String, String>,
}

impl Config {
    /// Loads defaults overridden by the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Loads defaults overridden by values returned from `lookup`.
    ///
    /// Environment-only, which is what every caller outside the engine
    /// means. The engine uses [`Config::from_layers`].
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Self::from_layers(&Layers::env_only(lookup))
    }

    /// Loads defaults, then stored settings, then the environment.
    ///
    /// One pass over every key, so the cross-field rules below
    /// (`per_host <= global`, `backoff_max >= backoff_base`, a template
    /// that is not empty) are checked against the *merged* view — which is
    /// the only view in which they mean anything. A stored value is
    /// therefore validated exactly the way an environment variable is, and
    /// a rejection quotes the same message.
    pub fn from_layers(layers: &Layers<'_>) -> Result<Self, ConfigError> {
        let (stored, env) = layers.snapshot();
        Self::from_snapshot(stored, env)
    }

    /// The config these two layers produce, remembering both.
    fn from_snapshot(
        stored: BTreeMap<String, String>,
        env: BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let mut cfg = {
            let layers = Layers::new(stored.clone(), |key: &str| env.get(key).cloned());
            Self::parse(&layers)?
        };
        cfg.stored = stored;
        cfg.env = env;
        Ok(cfg)
    }

    /// The config this one would become with `key` set to `value` in the
    /// stored layer, or removed from it when `value` is `None`.
    ///
    /// The whole configuration is re-assembled, which is the point: a
    /// per-key validator could not catch `per_host > global`, and this
    /// returns the same `ConfigError` the environment variable would
    /// produce. Nothing is written — the caller decides what to do with
    /// the answer. The data section is carried over unchanged.
    pub fn with_stored(&self, key: &str, value: Option<&str>) -> Result<Self, ConfigError> {
        let mut stored = self.stored.clone();
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => {
                stored.insert(key.to_owned(), v.to_owned());
            }
            None => {
                stored.remove(key);
            }
        }
        let mut next = Self::from_snapshot(stored, self.env.clone())?;
        // Its keys are environment only, so no stored value can change it,
        // but `--data-dir` and the desktop's archive folder are assigned after
        // the layers were read. Rebuilt from the layers alone, a running
        // engine's archive root would move to the platform default while its
        // downloads stayed where they were.
        next.data = self.data.clone();
        Ok(next)
    }

    /// Replaces the data section. For tests, benchmarks and anything else
    /// that builds a config by hand rather than from layers.
    ///
    /// The provenance fields stay private and stay empty, which is the
    /// truth about a hand-built config: every value in it is a default or
    /// was assigned directly, and none of it came from a layer. Letting a
    /// caller set them would let `with_stored` rebuild from a lie.
    #[must_use]
    pub fn with_data(mut self, data: DataConfig) -> Self {
        self.data = data;
        self
    }

    /// Replaces the feed section.
    #[must_use]
    pub fn with_feed(mut self, feed: FeedConfig) -> Self {
        self.feed = feed;
        self
    }

    /// Replaces the discovery section.
    #[must_use]
    pub fn with_discovery(mut self, discovery: DiscoveryConfig) -> Self {
        self.discovery = discovery;
        self
    }

    /// Replaces the download section.
    #[must_use]
    pub fn with_download(mut self, download: DownloadConfig) -> Self {
        self.download = download;
        self
    }

    /// Replaces the archive section.
    #[must_use]
    pub fn with_archive(mut self, archive: ArchiveConfig) -> Self {
        self.archive = archive;
        self
    }

    /// Replaces the maintenance section.
    #[must_use]
    pub fn with_maintenance(mut self, maintenance: MaintenanceConfig) -> Self {
        self.maintenance = maintenance;
        self
    }

    /// The stored value of `key`, shadowed or not.
    #[must_use]
    pub fn stored(&self, key: &str) -> Option<&str> {
        self.stored.get(key).map(String::as_str)
    }

    /// Whether the environment holds `key`, so a stored value for it would
    /// have no effect.
    #[must_use]
    pub fn is_pinned(&self, key: &str) -> bool {
        self.env.contains_key(key)
    }

    #[allow(clippy::too_many_lines)] // one linear pass over every key is clearer than splitting
    fn parse(layers: &Layers<'_>) -> Result<Self, ConfigError> {
        use env_keys as k;
        let mut cfg = Self {
            discovery: DiscoveryConfig::from_layers(layers)?,
            ..Self::default()
        };
        let mut origins = BTreeMap::new();
        let mut get = |key: &'static str| -> Option<String> {
            let resolved = layers.resolve(key);
            if let Some((_, origin)) = &resolved {
                origins.insert(key, *origin);
            }
            resolved.map(|(v, _)| v)
        };
        cfg.data.data_dir = get(k::DATA_DIR).map(PathBuf::from);
        if let Some(v) = get(k::FEED_MAX_BYTES) {
            let n = parse_number(k::FEED_MAX_BYTES, &v)?;
            if n < 1024 {
                return Err(ConfigError {
                    key: k::FEED_MAX_BYTES.to_owned(),
                    message: "must be at least 1024".to_owned(),
                });
            }
            cfg.feed.limits.max_bytes = n;
        }
        if let Some(v) = get(k::FEED_MAX_ITEMS) {
            cfg.feed.limits.max_items = parse_positive(k::FEED_MAX_ITEMS, &v)?;
        }
        if let Some(v) = get(k::FEED_REFRESH_CONCURRENCY) {
            cfg.feed.refresh_concurrency = parse_positive(k::FEED_REFRESH_CONCURRENCY, &v)?;
        }
        if let Some(v) = get(k::FEED_REFRESH_TIMEOUT_MS) {
            cfg.feed.refresh_timeout = parse_millis(k::FEED_REFRESH_TIMEOUT_MS, &v)?;
        }
        if let Some(v) = get(k::FEED_RETAIN_FETCHES) {
            cfg.feed.retain_fetches =
                u32::try_from(parse_positive(k::FEED_RETAIN_FETCHES, &v)?).unwrap_or(u32::MAX);
        }
        if let Some(v) = get(k::FEED_REMOVAL_STREAK) {
            cfg.feed.removal_streak =
                u32::try_from(parse_positive(k::FEED_REMOVAL_STREAK, &v)?).unwrap_or(u32::MAX);
        }
        if let Some(v) = get(k::FEED_MASS_REMOVAL_GUARD_PERCENT) {
            let n = parse_u32(k::FEED_MASS_REMOVAL_GUARD_PERCENT, &v)?;
            cfg.feed.mass_removal_guard_percent = u8::try_from(n)
                .ok()
                .filter(|n| (1..=100).contains(n))
                .ok_or_else(|| ConfigError {
                    key: k::FEED_MASS_REMOVAL_GUARD_PERCENT.to_owned(),
                    message: "must be a percentage between 1 and 100".to_owned(),
                })?;
        }
        if let Some(v) = get(k::FEED_SCHEDULER) {
            cfg.feed.scheduler = parse_bool(k::FEED_SCHEDULER, &v)?;
        }
        if let Some(v) = get(k::FEED_REFRESH_INTERVAL_SECS) {
            cfg.feed.refresh_interval =
                parse_interval(k::FEED_REFRESH_INTERVAL_SECS, &v, MIN_REFRESH_INTERVAL_SECS)?;
        }
        if let Some(v) = get(k::MAINTENANCE_INTERVAL_SECS) {
            cfg.maintenance.interval = parse_interval(
                k::MAINTENANCE_INTERVAL_SECS,
                &v,
                MIN_MAINTENANCE_INTERVAL_SECS,
            )?;
        }
        if let Some(v) = get(k::EVENTS_RETAIN_DAYS) {
            cfg.maintenance.retain_events_days = parse_u32(k::EVENTS_RETAIN_DAYS, &v)?;
        }
        if let Some(v) = get(k::EVENTS_RETAIN_MAX_ROWS) {
            cfg.maintenance.retain_events_max_rows = parse_number(k::EVENTS_RETAIN_MAX_ROWS, &v)?;
        }
        cfg.data.media_dir = get(k::MEDIA_DIR).map(PathBuf::from);
        let d = &mut cfg.download;
        if let Some(v) = get(k::DOWNLOAD_GLOBAL_CONCURRENCY) {
            d.global_concurrency = parse_positive(k::DOWNLOAD_GLOBAL_CONCURRENCY, &v)?;
        }
        if let Some(v) = get(k::DOWNLOAD_PER_HOST_CONCURRENCY) {
            d.per_host_concurrency = parse_positive(k::DOWNLOAD_PER_HOST_CONCURRENCY, &v)?;
        }
        if d.per_host_concurrency > d.global_concurrency {
            return Err(ConfigError {
                key: k::DOWNLOAD_PER_HOST_CONCURRENCY.to_owned(),
                message: format!(
                    "must not exceed {} ({})",
                    k::DOWNLOAD_GLOBAL_CONCURRENCY,
                    d.global_concurrency
                ),
            });
        }
        if let Some(v) = get(k::DOWNLOAD_MAX_ATTEMPTS) {
            d.max_attempts =
                u32::try_from(parse_positive(k::DOWNLOAD_MAX_ATTEMPTS, &v)?).unwrap_or(u32::MAX);
        }
        if let Some(v) = get(k::DOWNLOAD_BACKOFF_BASE_MS) {
            d.backoff_base = parse_millis(k::DOWNLOAD_BACKOFF_BASE_MS, &v)?;
        }
        if let Some(v) = get(k::DOWNLOAD_BACKOFF_MAX_MS) {
            d.backoff_max = parse_millis(k::DOWNLOAD_BACKOFF_MAX_MS, &v)?;
        }
        if d.backoff_max < d.backoff_base {
            return Err(ConfigError {
                key: k::DOWNLOAD_BACKOFF_MAX_MS.to_owned(),
                message: format!("must not be below {}", k::DOWNLOAD_BACKOFF_BASE_MS),
            });
        }
        if let Some(v) = get(k::DOWNLOAD_IDLE_TIMEOUT_MS) {
            d.idle_timeout = parse_millis(k::DOWNLOAD_IDLE_TIMEOUT_MS, &v)?;
        }
        if let Some(v) = get(k::DOWNLOAD_PROGRESS_INTERVAL_MS) {
            d.progress_interval = parse_millis(k::DOWNLOAD_PROGRESS_INTERVAL_MS, &v)?;
        }
        if let Some(v) = get(k::DOWNLOAD_MAX_BYTES) {
            let n = parse_number(k::DOWNLOAD_MAX_BYTES, &v)?;
            if n < 1024 * 1024 {
                return Err(ConfigError {
                    key: k::DOWNLOAD_MAX_BYTES.to_owned(),
                    message: "must be at least 1048576 (1 MiB)".to_owned(),
                });
            }
            d.max_bytes = n;
        }
        if let Some(v) = get(k::DOWNLOAD_MIN_FREE_BYTES) {
            d.min_free_bytes = parse_number(k::DOWNLOAD_MIN_FREE_BYTES, &v)?;
        }
        if let Some(v) = get(k::DOWNLOAD_SHUTDOWN_GRACE_MS) {
            d.shutdown_grace = parse_millis(k::DOWNLOAD_SHUTDOWN_GRACE_MS, &v)?;
        }
        let a = &mut cfg.archive;
        if let Some(v) = get(k::ARCHIVE_TEMPLATE) {
            a.template = v;
        }
        if let Some(v) = get(k::ARCHIVE_PATH_PROFILE) {
            a.path_profile = PathProfile::parse(&v.to_ascii_lowercase()).ok_or(ConfigError {
                key: k::ARCHIVE_PATH_PROFILE.to_owned(),
                message: "must be one of portable, posix, windows".to_owned(),
            })?;
        }
        if let Some(v) = get(k::ARCHIVE_VERIFY_ON_COMPLETION) {
            a.verify_on_completion = parse_bool(k::ARCHIVE_VERIFY_ON_COMPLETION, &v)?;
        }
        if let Some(v) = get(k::ARCHIVE_VERIFY_DEPTH) {
            a.verify_depth = VerifyDepth::parse(&v.to_ascii_lowercase()).ok_or(ConfigError {
                key: k::ARCHIVE_VERIFY_DEPTH.to_owned(),
                message: "must be one of existence, light, full".to_owned(),
            })?;
        }
        if let Some(v) = get(k::ARCHIVE_AUTO_DOWNLOAD) {
            a.auto_download = parse_bool(k::ARCHIVE_AUTO_DOWNLOAD, &v)?;
        }
        if let Some(v) = get(k::ARCHIVE_MAX_BACKLOG) {
            a.max_backlog = parse_u32(k::ARCHIVE_MAX_BACKLOG, &v)?;
        }
        if let Some(v) = get(k::ARCHIVE_MAX_AGE_DAYS) {
            a.max_age_days = parse_u32(k::ARCHIVE_MAX_AGE_DAYS, &v)?;
        }
        if let Some(v) = get(k::ARCHIVE_PRIORITY) {
            a.priority = Priority::parse(&v.to_ascii_lowercase()).ok_or(ConfigError {
                key: k::ARCHIVE_PRIORITY.to_owned(),
                message: "must be one of low, normal, high".to_owned(),
            })?;
        }
        if let Some(v) = get(k::ARCHIVE_SIDECARS) {
            a.sidecars = parse_bool(k::ARCHIVE_SIDECARS, &v)?;
        }
        if let Some(v) = get(k::ARCHIVE_MANIFESTS) {
            a.manifests = parse_bool(k::ARCHIVE_MANIFESTS, &v)?;
        }
        if let Some(v) = get(k::ARCHIVE_ARTWORK_FETCH) {
            a.artwork_fetch = parse_bool(k::ARCHIVE_ARTWORK_FETCH, &v)?;
        }
        if let Some(v) = get(k::ARCHIVE_ARTWORK_MAX_BYTES) {
            let n = parse_number(k::ARCHIVE_ARTWORK_MAX_BYTES, &v)?;
            if n == 0 {
                return Err(ConfigError {
                    key: k::ARCHIVE_ARTWORK_MAX_BYTES.to_owned(),
                    message: "must be greater than zero".to_owned(),
                });
            }
            a.artwork_max_bytes = n;
        }
        if let Some(v) = get(k::ARCHIVE_TAG_MODE) {
            a.tag_mode = TagMode::parse(&v.to_ascii_lowercase()).ok_or(ConfigError {
                key: k::ARCHIVE_TAG_MODE.to_owned(),
                message: "must be one of fill_missing, sync".to_owned(),
            })?;
        }
        if let Some(v) = get(k::ARCHIVE_IMPORT_MATCH_THRESHOLD) {
            let n = parse_u32(k::ARCHIVE_IMPORT_MATCH_THRESHOLD, &v)?;
            if !(1..=100).contains(&n) {
                return Err(ConfigError {
                    key: k::ARCHIVE_IMPORT_MATCH_THRESHOLD.to_owned(),
                    message: "must be a percentage between 1 and 100".to_owned(),
                });
            }
            a.import_match_threshold = n;
        }
        if a.template.trim().is_empty() {
            return Err(ConfigError {
                key: k::ARCHIVE_TEMPLATE.to_owned(),
                message: "must not be empty".to_owned(),
            });
        }
        cfg.origins = origins;
        Ok(cfg)
    }

    /// Origin of any key (discovery keys are delegated).
    pub fn origin(&self, key: &str) -> Origin {
        self.origins
            .get(key)
            .copied()
            .unwrap_or_else(|| self.discovery.origin(key))
    }

    /// Effective values, origins and provenance for every key.
    ///
    /// This is what `uguisu config validate` prints and what
    /// `GET /api/v1/settings` returns, so it has to answer the awkward
    /// question as well as the easy one: not just "what is this worth" but
    /// "why is the value I stored not the one in force".
    pub fn describe(&self) -> Vec<KeyDescription> {
        self.describe_rows()
            .into_iter()
            .map(|(key, value, origin)| {
                let spec = crate::settings::spec(&key);
                KeyDescription {
                    // A secret's row is reported as present, never as its
                    // value: this is the answer a browser and `config list`
                    // both get.
                    stored: self.stored.get(&key).map(|stored| redacted(&key, stored)),
                    pinned: self.stored.contains_key(&key) && origin > Origin::Settings,
                    persistable: spec.is_some_and(|s| s.persistable),
                    live: spec.is_some_and(|s| s.live),
                    key,
                    value,
                    origin,
                }
            })
            .collect()
    }

    /// Stored values Uguisu is not using: unknown keys, keys it refuses to
    /// take from the database, and keys the environment shadows.
    ///
    /// Every one of them stays in the table. A row is the only copy of
    /// what somebody meant, and deleting it to tidy up would throw that
    /// away — so they are reported instead, each with its reason.
    #[must_use]
    pub fn unused_stored(&self) -> Vec<(String, String, &'static str)> {
        self.stored
            .iter()
            .filter_map(|(key, value)| {
                let reason = if crate::settings::spec(key).is_none() {
                    "unknown"
                } else if !crate::settings::is_persistable(key) {
                    "not persistable"
                } else if self.env.contains_key(key) {
                    "pinned by the environment"
                } else {
                    return None;
                };
                Some((key.clone(), redacted(key, value), reason))
            })
            .collect()
    }

    #[allow(clippy::too_many_lines)] // one flat row per key; splitting hides the table
    fn describe_rows(&self) -> Vec<(String, String, Origin)> {
        use env_keys as k;
        let mut rows = self.discovery.describe();
        let engine: Vec<(&'static str, String)> = vec![
            (
                k::DATA_DIR,
                self.data.data_dir().map_or_else(
                    |e| format!("(unresolved: {})", e.message),
                    |p| p.display().to_string(),
                ),
            ),
            (k::FEED_MAX_BYTES, self.feed.limits.max_bytes.to_string()),
            (k::FEED_MAX_ITEMS, self.feed.limits.max_items.to_string()),
            (
                k::FEED_REFRESH_CONCURRENCY,
                self.feed.refresh_concurrency.to_string(),
            ),
            (
                k::FEED_REFRESH_TIMEOUT_MS,
                self.feed.refresh_timeout.as_millis().to_string(),
            ),
            (k::FEED_RETAIN_FETCHES, self.feed.retain_fetches.to_string()),
            (k::FEED_REMOVAL_STREAK, self.feed.removal_streak.to_string()),
            (
                k::FEED_MASS_REMOVAL_GUARD_PERCENT,
                self.feed.mass_removal_guard_percent.to_string(),
            ),
            (k::FEED_SCHEDULER, self.feed.scheduler.to_string()),
            (
                k::FEED_REFRESH_INTERVAL_SECS,
                self.feed.refresh_interval.as_secs().to_string(),
            ),
            (
                k::MEDIA_DIR,
                self.data.media_dir().map_or_else(
                    |e| format!("(unresolved: {})", e.message),
                    |p| p.display().to_string(),
                ),
            ),
            (
                k::DOWNLOAD_GLOBAL_CONCURRENCY,
                self.download.global_concurrency.to_string(),
            ),
            (
                k::DOWNLOAD_PER_HOST_CONCURRENCY,
                self.download.per_host_concurrency.to_string(),
            ),
            (
                k::DOWNLOAD_MAX_ATTEMPTS,
                self.download.max_attempts.to_string(),
            ),
            (
                k::DOWNLOAD_BACKOFF_BASE_MS,
                self.download.backoff_base.as_millis().to_string(),
            ),
            (
                k::DOWNLOAD_BACKOFF_MAX_MS,
                self.download.backoff_max.as_millis().to_string(),
            ),
            (
                k::DOWNLOAD_IDLE_TIMEOUT_MS,
                self.download.idle_timeout.as_millis().to_string(),
            ),
            (
                k::DOWNLOAD_PROGRESS_INTERVAL_MS,
                self.download.progress_interval.as_millis().to_string(),
            ),
            (k::DOWNLOAD_MAX_BYTES, self.download.max_bytes.to_string()),
            (
                k::DOWNLOAD_MIN_FREE_BYTES,
                self.download.min_free_bytes.to_string(),
            ),
            (
                k::DOWNLOAD_SHUTDOWN_GRACE_MS,
                self.download.shutdown_grace.as_millis().to_string(),
            ),
            (k::ARCHIVE_TEMPLATE, self.archive.template.clone()),
            (
                k::ARCHIVE_PATH_PROFILE,
                self.archive.path_profile.as_str().to_owned(),
            ),
            (
                k::ARCHIVE_VERIFY_ON_COMPLETION,
                self.archive.verify_on_completion.to_string(),
            ),
            (
                k::ARCHIVE_VERIFY_DEPTH,
                self.archive.verify_depth.as_str().to_owned(),
            ),
            (
                k::ARCHIVE_AUTO_DOWNLOAD,
                self.archive.auto_download.to_string(),
            ),
            (k::ARCHIVE_MAX_BACKLOG, self.archive.max_backlog.to_string()),
            (
                k::ARCHIVE_MAX_AGE_DAYS,
                self.archive.max_age_days.to_string(),
            ),
            (
                k::ARCHIVE_PRIORITY,
                self.archive.priority.as_str().to_owned(),
            ),
            (k::ARCHIVE_SIDECARS, self.archive.sidecars.to_string()),
            (k::ARCHIVE_MANIFESTS, self.archive.manifests.to_string()),
            (
                k::ARCHIVE_ARTWORK_FETCH,
                self.archive.artwork_fetch.to_string(),
            ),
            (
                k::ARCHIVE_ARTWORK_MAX_BYTES,
                self.archive.artwork_max_bytes.to_string(),
            ),
            (
                k::ARCHIVE_TAG_MODE,
                self.archive.tag_mode.as_str().to_owned(),
            ),
            (
                k::ARCHIVE_IMPORT_MATCH_THRESHOLD,
                self.archive.import_match_threshold.to_string(),
            ),
            (
                k::MAINTENANCE_INTERVAL_SECS,
                self.maintenance.interval.as_secs().to_string(),
            ),
            (
                k::EVENTS_RETAIN_DAYS,
                self.maintenance.retain_events_days.to_string(),
            ),
            (
                k::EVENTS_RETAIN_MAX_ROWS,
                self.maintenance.retain_events_max_rows.to_string(),
            ),
        ];
        rows.extend(
            engine
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value, self.origin(key))),
        );
        rows
    }
}

fn parse_positive(key: &str, value: &str) -> Result<usize, ConfigError> {
    let n = parse_number(key, value)?;
    if n == 0 {
        return Err(ConfigError {
            key: key.to_owned(),
            message: "must be at least 1".to_owned(),
        });
    }
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

impl DiscoveryConfig {
    /// Loads defaults overridden by the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Loads defaults overridden by values returned from `lookup` (used by
    /// tests).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Self::from_layers(&Layers::env_only(lookup))
    }

    /// Loads defaults, then stored settings, then the environment.
    #[allow(clippy::too_many_lines)] // one linear pass over every key is clearer than splitting
    pub fn from_layers(layers: &Layers<'_>) -> Result<Self, ConfigError> {
        use env_keys as k;
        let mut cfg = Self::default();
        let mut origins = BTreeMap::new();

        let mut get = |key: &'static str| -> Option<String> {
            let resolved = layers.resolve(key);
            if let Some((_, origin)) = &resolved {
                origins.insert(key, *origin);
            }
            resolved.map(|(v, _)| v)
        };

        if let Some(v) = get(k::APPLE_ENABLED) {
            cfg.apple.enabled = parse_bool(k::APPLE_ENABLED, &v)?;
        }
        if let Some(v) = get(k::APPLE_COUNTRY) {
            if v.len() != 2 || !v.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(ConfigError {
                    key: k::APPLE_COUNTRY.to_owned(),
                    message: "expected a two-letter ISO 3166-1 country code".to_owned(),
                });
            }
            cfg.apple.country = v.to_ascii_uppercase();
        }
        if let Some(v) = get(k::APPLE_LANG) {
            let v = v.to_ascii_lowercase();
            if v != "en_us" && v != "ja_jp" {
                return Err(ConfigError {
                    key: k::APPLE_LANG.to_owned(),
                    message: "Apple accepts only `en_us` or `ja_jp`".to_owned(),
                });
            }
            cfg.apple.lang = Some(v);
        }
        if let Some(v) = get(k::APPLE_BASE_URL) {
            cfg.apple.base_url = parse_base_url(k::APPLE_BASE_URL, &v)?;
        }

        cfg.podcastindex.key = get(k::PODCASTINDEX_KEY).map(Secret::new);
        cfg.podcastindex.secret = get(k::PODCASTINDEX_SECRET).map(Secret::new);
        cfg.podcastindex.enabled = cfg.podcastindex.has_credentials();
        if let Some(v) = get(k::PODCASTINDEX_ENABLED) {
            cfg.podcastindex.enabled = parse_bool(k::PODCASTINDEX_ENABLED, &v)?;
        }
        if let Some(v) = get(k::PODCASTINDEX_BASE_URL) {
            cfg.podcastindex.base_url = parse_base_url(k::PODCASTINDEX_BASE_URL, &v)?;
        }

        if let Some(v) = get(k::GPODDERNET_ENABLED) {
            cfg.gpoddernet.enabled = parse_bool(k::GPODDERNET_ENABLED, &v)?;
        }
        if let Some(v) = get(k::GPODDERNET_BASE_URL) {
            cfg.gpoddernet.base_url = parse_base_url(k::GPODDERNET_BASE_URL, &v)?;
        }

        if let Some(v) = get(k::SOFT_DEADLINE_MS) {
            cfg.search.soft_deadline = parse_millis(k::SOFT_DEADLINE_MS, &v)?;
        }
        if let Some(v) = get(k::HARD_DEADLINE_MS) {
            cfg.search.hard_deadline = parse_millis(k::HARD_DEADLINE_MS, &v)?;
        }
        if let Some(v) = get(k::PROVIDER_TIMEOUT_MS) {
            cfg.search.provider_timeout = parse_millis(k::PROVIDER_TIMEOUT_MS, &v)?;
        }
        if let Some(v) = get(k::LIMIT) {
            let n = parse_number(k::LIMIT, &v)?;
            let max = cfg.search.max_limit;
            if n == 0 || usize::try_from(n).map_or(true, |n| n > max) {
                return Err(ConfigError {
                    key: k::LIMIT.to_owned(),
                    message: format!("must be between 1 and {max}"),
                });
            }
            cfg.search.default_limit = usize::try_from(n).unwrap_or(usize::MAX);
        }
        if cfg.search.soft_deadline > cfg.search.hard_deadline {
            return Err(ConfigError {
                key: k::SOFT_DEADLINE_MS.to_owned(),
                message: "soft deadline must not exceed the hard deadline".to_owned(),
            });
        }

        if let Some(v) = get(k::CACHE_SEARCH_TTL_SECS) {
            cfg.cache.search_ttl = Duration::from_secs(parse_number(k::CACHE_SEARCH_TTL_SECS, &v)?);
        }
        if let Some(v) = get(k::CACHE_LOOKUP_TTL_SECS) {
            cfg.cache.lookup_ttl = Duration::from_secs(parse_number(k::CACHE_LOOKUP_TTL_SECS, &v)?);
        }
        if let Some(v) = get(k::CACHE_MAX_ENTRIES) {
            cfg.cache.max_entries = parse_number(k::CACHE_MAX_ENTRIES, &v)?;
        }

        if let Some(v) = get(k::HTTP_ALLOW_PRIVATE_HOSTS) {
            cfg.network.allow_private_hosts = v
                .split(',')
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect();
        }
        cfg.network.user_agent = get(k::HTTP_USER_AGENT);
        if let Some(v) = get(k::HTTP_CONNECT_TIMEOUT_MS) {
            cfg.network.connect_timeout = parse_millis(k::HTTP_CONNECT_TIMEOUT_MS, &v)?;
        }
        if let Some(v) = get(k::HTTP_REQUEST_TIMEOUT_MS) {
            cfg.network.request_timeout = parse_millis(k::HTTP_REQUEST_TIMEOUT_MS, &v)?;
        }

        cfg.origins = origins;
        Ok(cfg)
    }

    /// Origin of a value settable through the environment (`Default` when unset).
    pub fn origin(&self, key: &str) -> Origin {
        self.origins.get(key).copied().unwrap_or(Origin::Default)
    }

    /// Effective values with origins, secrets redacted — for `config validate`.
    pub fn describe(&self) -> Vec<(String, String, Origin)> {
        use env_keys as k;
        let redact = |s: &Option<Secret<String>>| {
            // Same word as `redacted`, for the value in force rather than a
            // stored row.
            s.as_ref()
                .map_or_else(|| "(unset)".to_owned(), |_| REDACTED.to_owned())
        };
        let rows: Vec<(&'static str, String)> = vec![
            (k::APPLE_ENABLED, self.apple.enabled.to_string()),
            (k::APPLE_COUNTRY, self.apple.country.clone()),
            (
                k::APPLE_LANG,
                self.apple
                    .lang
                    .clone()
                    .unwrap_or_else(|| "(unset)".to_owned()),
            ),
            (k::APPLE_BASE_URL, self.apple.base_url.clone()),
            (
                k::PODCASTINDEX_ENABLED,
                self.podcastindex.enabled.to_string(),
            ),
            (k::PODCASTINDEX_KEY, redact(&self.podcastindex.key)),
            (k::PODCASTINDEX_SECRET, redact(&self.podcastindex.secret)),
            (k::PODCASTINDEX_BASE_URL, self.podcastindex.base_url.clone()),
            (k::GPODDERNET_ENABLED, self.gpoddernet.enabled.to_string()),
            (k::GPODDERNET_BASE_URL, self.gpoddernet.base_url.clone()),
            (
                k::SOFT_DEADLINE_MS,
                self.search.soft_deadline.as_millis().to_string(),
            ),
            (
                k::HARD_DEADLINE_MS,
                self.search.hard_deadline.as_millis().to_string(),
            ),
            (
                k::PROVIDER_TIMEOUT_MS,
                self.search.provider_timeout.as_millis().to_string(),
            ),
            (k::LIMIT, self.search.default_limit.to_string()),
            (
                k::CACHE_SEARCH_TTL_SECS,
                self.cache.search_ttl.as_secs().to_string(),
            ),
            (
                k::CACHE_LOOKUP_TTL_SECS,
                self.cache.lookup_ttl.as_secs().to_string(),
            ),
            (k::CACHE_MAX_ENTRIES, self.cache.max_entries.to_string()),
            (
                k::HTTP_ALLOW_PRIVATE_HOSTS,
                self.network.allow_private_hosts.join(","),
            ),
            (
                k::HTTP_USER_AGENT,
                self.network
                    .user_agent
                    .clone()
                    .unwrap_or_else(|| crate::USER_AGENT.to_owned()),
            ),
            (
                k::HTTP_CONNECT_TIMEOUT_MS,
                self.network.connect_timeout.as_millis().to_string(),
            ),
            (
                k::HTTP_REQUEST_TIMEOUT_MS,
                self.network.request_timeout.as_millis().to_string(),
            ),
        ];
        rows.into_iter()
            .map(|(key, value)| (key.to_owned(), value, self.origin(key)))
            .collect()
    }
}

/// Reads a boolean the way every `UGUISU_*` boolean is read.
///
/// One vocabulary, so a value that works in the environment also works as a
/// command-line flag.
#[must_use]
pub fn boolean(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// What a report prints instead of a credential.
pub const REDACTED: &str = "[redacted]";

/// `value` unless `key` is a secret, in which case [`REDACTED`].
///
/// One function, because a secret leaks through whichever of the four places
/// forgets: the value in force, the stored row, a stored row Uguisu is
/// ignoring, and a stored row it refused to parse.
#[must_use]
pub fn redacted(key: &str, value: &str) -> String {
    if crate::settings::spec(key).is_some_and(|s| s.secret) {
        REDACTED.to_owned()
    } else {
        value.to_owned()
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool, ConfigError> {
    boolean(value).ok_or_else(|| ConfigError {
        key: key.to_owned(),
        message: format!("expected true/false, got `{value}`"),
    })
}

fn parse_number(key: &str, value: &str) -> Result<u64, ConfigError> {
    value.parse::<u64>().map_err(|_| ConfigError {
        key: key.to_owned(),
        message: format!("expected a non-negative integer, got `{value}`"),
    })
}

#[cfg(test)]
mod archive_config_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn one(key: &'static str, value: &'static str) -> Result<Config, ConfigError> {
        Config::from_lookup(|k| (k == key).then(|| value.to_owned()))
    }

    #[test]
    fn archive_defaults_are_off_and_portable() {
        let c = Config::from_lookup(|_| None).unwrap();
        assert_eq!(c.archive, ArchiveConfig::default());
        assert_eq!(c.archive.template, ArchiveConfig::DEFAULT_TEMPLATE);
        assert_eq!(c.archive.path_profile, PathProfile::Portable);
        assert!(
            !c.archive.auto_download,
            "an update must not start downloading on its own"
        );
        assert!(c.archive.verify_on_completion);
        assert_eq!(c.archive.verify_depth, VerifyDepth::Light);
        assert_eq!(c.archive.max_backlog, 3);
        assert_eq!(c.archive.max_age_days, 0);
        assert_eq!(c.archive.priority, Priority::Normal);
    }

    #[test]
    fn unusable_archive_values_name_their_key() {
        for (key, value) in [
            ("UGUISU_ARCHIVE_PATH_PROFILE", "amiga"),
            ("UGUISU_ARCHIVE_VERIFY_DEPTH", "paranoid"),
            ("UGUISU_ARCHIVE_PRIORITY", "urgent"),
            ("UGUISU_ARCHIVE_MAX_BACKLOG", "-1"),
            ("UGUISU_ARCHIVE_MAX_AGE_DAYS", "eternity"),
            ("UGUISU_ARCHIVE_AUTO_DOWNLOAD", "perhaps"),
            ("UGUISU_ARCHIVE_SIDECARS", "sometimes"),
            ("UGUISU_ARCHIVE_MANIFESTS", "later"),
            ("UGUISU_ARCHIVE_ARTWORK_FETCH", "maybe"),
            ("UGUISU_ARCHIVE_ARTWORK_MAX_BYTES", "0"),
            ("UGUISU_ARCHIVE_TAG_MODE", "overwrite"),
            ("UGUISU_ARCHIVE_IMPORT_MATCH_THRESHOLD", "0"),
            ("UGUISU_ARCHIVE_IMPORT_MATCH_THRESHOLD", "101"),
        ] {
            let err = one(key, value).unwrap_err();
            assert_eq!(err.key, key, "{key} = {value}");
            assert!(!err.message.is_empty());
        }
    }

    #[test]
    fn archive_keys_are_documented_and_have_origins() {
        let c = Config::from_lookup(|k| {
            (k == env_keys::ARCHIVE_TEMPLATE).then(|| "{episode.id}.{extension}".to_owned())
        })
        .unwrap();
        assert_eq!(c.origin(env_keys::ARCHIVE_TEMPLATE), Origin::Env);
        assert_eq!(c.origin(env_keys::ARCHIVE_PRIORITY), Origin::Default);
        let described: Vec<String> = c.describe().into_iter().map(|d| d.key).collect();
        for key in [
            env_keys::ARCHIVE_TEMPLATE,
            env_keys::ARCHIVE_PATH_PROFILE,
            env_keys::ARCHIVE_VERIFY_ON_COMPLETION,
            env_keys::ARCHIVE_VERIFY_DEPTH,
            env_keys::ARCHIVE_AUTO_DOWNLOAD,
            env_keys::ARCHIVE_MAX_BACKLOG,
            env_keys::ARCHIVE_MAX_AGE_DAYS,
            env_keys::ARCHIVE_PRIORITY,
            env_keys::ARCHIVE_SIDECARS,
            env_keys::ARCHIVE_MANIFESTS,
            env_keys::ARCHIVE_ARTWORK_FETCH,
            env_keys::ARCHIVE_ARTWORK_MAX_BYTES,
            env_keys::ARCHIVE_TAG_MODE,
            env_keys::ARCHIVE_IMPORT_MATCH_THRESHOLD,
        ] {
            assert!(described.contains(&key.to_owned()), "{key} is described");
            assert!(crate::settings::spec(key).is_some(), "{key} is listed");
        }
    }

    #[test]
    fn asset_defaults_never_reach_out() {
        let c = Config::from_lookup(|_| None).unwrap();
        assert!(
            c.archive.sidecars,
            "the archive describes itself by default"
        );
        assert!(c.archive.manifests);
        assert!(
            !c.archive.artwork_fetch,
            "installing Phase 6 must not start making requests"
        );
        assert_eq!(c.archive.artwork_max_bytes, 16 * 1024 * 1024);
        assert_eq!(c.archive.tag_mode, TagMode::FillMissing);
        assert_eq!(c.archive.import_match_threshold, 85);

        // Every value round-trips through `config show` unchanged, which an
        // f32 threshold would not have done.
        let shown: std::collections::BTreeMap<String, String> =
            c.describe().into_iter().map(|d| (d.key, d.value)).collect();
        assert_eq!(shown[env_keys::ARCHIVE_IMPORT_MATCH_THRESHOLD], "85");
        assert_eq!(shown[env_keys::ARCHIVE_TAG_MODE], "fill_missing");
        assert_eq!(shown[env_keys::ARCHIVE_ARTWORK_FETCH], "false");

        // The CLI spelling is accepted where the value is read.
        let c = one(env_keys::ARCHIVE_TAG_MODE, "fill-missing").unwrap();
        assert_eq!(c.archive.tag_mode, TagMode::FillMissing);
        let c = one(env_keys::ARCHIVE_IMPORT_MATCH_THRESHOLD, "70").unwrap();
        assert_eq!(c.archive.import_match_threshold, 70);
    }
}

fn parse_u32(key: &str, value: &str) -> Result<u32, ConfigError> {
    let n = parse_number(key, value)?;
    u32::try_from(n).map_err(|_| ConfigError {
        key: key.to_owned(),
        message: format!("expected an integer up to {}, got `{value}`", u32::MAX),
    })
}

/// Shortest interval between two automatic refreshes of one podcast.
///
/// Not a matter of taste: an interval below this would have Uguisu
/// hammering an origin that has no way to ask it to stop, and the
/// configuration is the wrong place to make that possible by accident.
pub const MIN_REFRESH_INTERVAL_SECS: u64 = 60;

/// Shortest interval between two maintenance passes.
pub const MIN_MAINTENANCE_INTERVAL_SECS: u64 = 60;

/// Longest interval of either kind: past a year, an interval is not a
/// schedule any more.
pub const MAX_INTERVAL_SECS: u64 = 31_536_000;

fn parse_interval(key: &str, value: &str, min_secs: u64) -> Result<Duration, ConfigError> {
    let secs = parse_number(key, value)?;
    if secs < min_secs {
        return Err(ConfigError {
            key: key.to_owned(),
            message: format!("must be at least {min_secs} seconds"),
        });
    }
    if secs > MAX_INTERVAL_SECS {
        return Err(ConfigError {
            key: key.to_owned(),
            message: format!("must be at most {MAX_INTERVAL_SECS} seconds (a year)"),
        });
    }
    Ok(Duration::from_secs(secs))
}

fn parse_millis(key: &str, value: &str) -> Result<Duration, ConfigError> {
    let ms = parse_number(key, value)?;
    if ms == 0 {
        return Err(ConfigError {
            key: key.to_owned(),
            message: "must be greater than 0".to_owned(),
        });
    }
    Ok(Duration::from_millis(ms))
}

fn parse_base_url(key: &str, value: &str) -> Result<String, ConfigError> {
    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err(ConfigError {
            key: key.to_owned(),
            message: "expected an http(s) URL".to_owned(),
        });
    }
    Ok(value.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::HashMap;

    use super::*;

    fn cfg(vars: &[(&str, &str)]) -> Result<DiscoveryConfig, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        DiscoveryConfig::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_are_sane() {
        let c = DiscoveryConfig::default();
        assert!(c.apple.enabled);
        assert!(!c.podcastindex.enabled);
        assert!(!c.gpoddernet.enabled);
        assert!(c.search.soft_deadline < c.search.hard_deadline);
        assert_eq!(c.origin(env_keys::APPLE_COUNTRY), Origin::Default);
    }

    #[test]
    fn podcastindex_enables_itself_when_credentials_present() {
        let c = cfg(&[
            ("UGUISU_PODCASTINDEX_KEY", "k"),
            ("UGUISU_PODCASTINDEX_SECRET", "s"),
        ])
        .unwrap();
        assert!(c.podcastindex.enabled);
        assert_eq!(c.origin(env_keys::PODCASTINDEX_KEY), Origin::Env);
        let c = cfg(&[("UGUISU_PODCASTINDEX_KEY", "k")]).unwrap();
        assert!(!c.podcastindex.enabled, "key without secret is not enough");
        let c = cfg(&[
            ("UGUISU_PODCASTINDEX_KEY", "k"),
            ("UGUISU_PODCASTINDEX_SECRET", "s"),
            ("UGUISU_DISCOVERY_PODCASTINDEX_ENABLED", "false"),
        ])
        .unwrap();
        assert!(!c.podcastindex.enabled, "explicit flag wins");
    }

    #[test]
    fn validates_values() {
        assert!(cfg(&[("UGUISU_APPLE_COUNTRY", "USA")]).is_err());
        assert!(cfg(&[("UGUISU_APPLE_LANG", "de_de")]).is_err());
        assert!(cfg(&[("UGUISU_DISCOVERY_APPLE_ENABLED", "maybe")]).is_err());
        assert!(cfg(&[("UGUISU_DISCOVERY_LIMIT", "0")]).is_err());
        assert!(cfg(&[("UGUISU_DISCOVERY_LIMIT", "101")]).is_err());
        assert!(cfg(&[("UGUISU_DISCOVERY_LIMIT", "100")]).is_ok());
        assert!(cfg(&[("UGUISU_DISCOVERY_APPLE_BASE_URL", "itunes.apple.com")]).is_err());
        assert!(
            cfg(&[
                ("UGUISU_DISCOVERY_SOFT_DEADLINE_MS", "9000"),
                ("UGUISU_DISCOVERY_HARD_DEADLINE_MS", "8000")
            ])
            .is_err()
        );
        let c = cfg(&[
            ("UGUISU_APPLE_COUNTRY", "de"),
            ("UGUISU_APPLE_LANG", "EN_US"),
        ])
        .unwrap();
        assert_eq!(c.apple.country, "DE");
        assert_eq!(c.apple.lang.as_deref(), Some("en_us"));
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let c = cfg(&[(
            "UGUISU_DISCOVERY_GPODDERNET_BASE_URL",
            "http://127.0.0.1:9/",
        )])
        .unwrap();
        assert_eq!(c.gpoddernet.base_url, "http://127.0.0.1:9");
    }

    #[test]
    fn describe_redacts_secrets_and_reports_origins() {
        let c = cfg(&[
            ("UGUISU_PODCASTINDEX_KEY", "k"),
            ("UGUISU_PODCASTINDEX_SECRET", "s"),
        ])
        .unwrap();
        let rows = c.describe();
        let key_row = rows
            .iter()
            .find(|r| r.0 == env_keys::PODCASTINDEX_KEY)
            .unwrap();
        assert_eq!(key_row.1, "[redacted]");
        assert_eq!(key_row.2, Origin::Env);
        assert!(
            !rows
                .iter()
                .any(|r| r.1.contains('k') && r.0.ends_with("_SECRET"))
        );
        assert_eq!(
            rows.len(),
            crate::settings::in_scope(crate::settings::Scope::Discovery).count()
        );
    }

    #[test]
    fn stored_value_keeps_data_dirs() {
        let mut c = Config::from_lookup(|_| None).unwrap();
        c.data.data_dir = Some(PathBuf::from("/srv/uguisu"));
        c.data.media_dir = Some(PathBuf::from("/mnt/archive"));
        let next = c
            .with_stored(env_keys::FEED_REMOVAL_STREAK, Some("3"))
            .unwrap();
        assert_eq!(next.feed.removal_streak, 3);
        assert_eq!(next.data, c.data);
        assert_eq!(
            next.with_stored(env_keys::FEED_REMOVAL_STREAK, None)
                .unwrap()
                .data,
            c.data
        );
    }

    #[test]
    fn the_removal_guard_is_a_percentage() {
        let key = env_keys::FEED_MASS_REMOVAL_GUARD_PERCENT;
        let with = |value: &str| Config::from_lookup(|k| (k == key).then(|| value.to_owned()));
        assert_eq!(with("100").unwrap().feed.mass_removal_guard_percent, 100);
        for value in ["0", "101", "256", "half"] {
            assert_eq!(with(value).unwrap_err().key, key, "{value}");
        }
    }

    #[test]
    fn a_full_config_parses_every_key() {
        let map: HashMap<String, String> = [
            ("UGUISU_DATA_DIR", "/tmp/uguisu-test"),
            ("UGUISU_FEED_MAX_ITEMS", "10"),
            ("UGUISU_FEED_REFRESH_TIMEOUT_MS", "1500"),
            ("UGUISU_FEED_REMOVAL_STREAK", "3"),
            ("UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT", "90"),
            ("UGUISU_APPLE_COUNTRY", "de"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        let c = Config::from_lookup(|k| map.get(k).cloned()).unwrap();
        assert_eq!(
            c.data.data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/uguisu-test"))
        );
        assert_eq!(
            c.data.database_path().unwrap().file_name().unwrap(),
            "uguisu.db"
        );
        assert_eq!(c.feed.limits.max_items, 10);
        assert_eq!(c.feed.refresh_timeout, Duration::from_millis(1500));
        assert_eq!(c.feed.removal_streak, 3);
        assert_eq!(c.feed.mass_removal_guard_percent, 90);
        assert_eq!(c.discovery.apple.country, "DE");
        assert_eq!(c.origin(env_keys::DATA_DIR), Origin::Env);
        assert_eq!(c.origin(env_keys::APPLE_COUNTRY), Origin::Env);
        assert_eq!(c.origin(env_keys::FEED_MAX_BYTES), Origin::Default);
        let rows = c.describe();
        // `describe` and the registry are two hand-written lists of the same
        // keys, so they are compared key by key rather than by length: a
        // pair of offsetting mistakes would pass a count.
        let described: Vec<&str> = rows.iter().map(|d| d.key.as_str()).collect();
        let registered: Vec<&str> = crate::settings::all().collect();
        assert_eq!(described, registered);
        assert!(
            Config::from_lookup(|k| (k == "UGUISU_FEED_MAX_BYTES").then(|| "10".to_owned()))
                .is_err()
        );
        assert!(
            Config::from_lookup(
                |k| (k == "UGUISU_FEED_REFRESH_CONCURRENCY").then(|| "0".to_owned())
            )
            .is_err()
        );
    }

    #[test]
    fn download_and_media_keys_parse_and_validate() {
        let one = |k: &'static str, v: &'static str| {
            Config::from_lookup(move |key| (key == k).then(|| v.to_owned()))
        };
        let c = Config::from_lookup(|k| {
            [
                ("UGUISU_DATA_DIR", "/tmp/uguisu-d"),
                ("UGUISU_MEDIA_DIR", "/mnt/podcasts"),
                (
                    "UGUISU_ARCHIVE_TEMPLATE",
                    "{podcast.title}/{episode.title}.{extension}",
                ),
                ("UGUISU_ARCHIVE_PATH_PROFILE", "windows"),
                ("UGUISU_ARCHIVE_VERIFY_ON_COMPLETION", "false"),
                ("UGUISU_ARCHIVE_VERIFY_DEPTH", "full"),
                ("UGUISU_ARCHIVE_AUTO_DOWNLOAD", "true"),
                ("UGUISU_ARCHIVE_MAX_BACKLOG", "5"),
                ("UGUISU_ARCHIVE_MAX_AGE_DAYS", "30"),
                ("UGUISU_ARCHIVE_PRIORITY", "high"),
                ("UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY", "6"),
                ("UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY", "2"),
                ("UGUISU_DOWNLOAD_MAX_ATTEMPTS", "3"),
                ("UGUISU_DOWNLOAD_BACKOFF_BASE_MS", "100"),
                ("UGUISU_DOWNLOAD_BACKOFF_MAX_MS", "5000"),
                ("UGUISU_DOWNLOAD_IDLE_TIMEOUT_MS", "2000"),
                ("UGUISU_DOWNLOAD_PROGRESS_INTERVAL_MS", "250"),
                ("UGUISU_DOWNLOAD_MAX_BYTES", "2097152"),
                ("UGUISU_DOWNLOAD_MIN_FREE_BYTES", "0"),
                ("UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS", "1000"),
            ]
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| (*v).to_owned())
        })
        .unwrap();
        assert_eq!(
            c.data.media_dir().unwrap(),
            std::path::PathBuf::from("/mnt/podcasts")
        );
        assert_eq!(c.download.global_concurrency, 6);
        assert_eq!(c.download.per_host_concurrency, 2);
        assert_eq!(c.download.max_attempts, 3);
        assert_eq!(c.download.backoff_base, Duration::from_millis(100));
        assert_eq!(c.download.backoff_max, Duration::from_secs(5));
        assert_eq!(c.download.idle_timeout, Duration::from_secs(2));
        assert_eq!(c.download.progress_interval, Duration::from_millis(250));
        assert_eq!(c.download.max_bytes, 2_097_152);
        assert_eq!(c.download.min_free_bytes, 0);
        assert_eq!(c.download.shutdown_grace, Duration::from_secs(1));
        assert_eq!(c.origin(env_keys::MEDIA_DIR), Origin::Env);
        assert_eq!(
            c.archive.template,
            "{podcast.title}/{episode.title}.{extension}"
        );
        assert_eq!(c.archive.path_profile, PathProfile::Windows);
        assert!(!c.archive.verify_on_completion);
        assert_eq!(c.archive.verify_depth, VerifyDepth::Full);
        assert!(c.archive.auto_download);
        assert_eq!(c.archive.max_backlog, 5);
        assert_eq!(c.archive.max_age_days, 30);
        assert_eq!(c.archive.priority, Priority::High);

        // Defaults and the derived media directory.
        let d = one("UGUISU_DATA_DIR", "/tmp/uguisu-x").unwrap();
        assert_eq!(
            d.data.media_dir().unwrap(),
            std::path::PathBuf::from("/tmp/uguisu-x").join("media")
        );
        assert_eq!(d.download, DownloadConfig::default());
        assert_eq!(d.download.global_concurrency, 3);
        assert_eq!(d.download.per_host_concurrency, 1);

        // Validation.
        assert_eq!(
            one("UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY", "4")
                .unwrap_err()
                .key,
            "UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY"
        );
        assert!(one("UGUISU_DOWNLOAD_MAX_ATTEMPTS", "0").is_err());
        assert!(one("UGUISU_DOWNLOAD_MAX_BYTES", "1000").is_err());
        assert!(one("UGUISU_DOWNLOAD_BACKOFF_MAX_MS", "10").is_err());
        assert!(one("UGUISU_DOWNLOAD_IDLE_TIMEOUT_MS", "x").is_err());
        assert!(
            one("UGUISU_DOWNLOAD_BACKOFF_BASE_MS", "999999999999")
                .unwrap_err()
                .key
                .ends_with("BACKOFF_MAX_MS")
        );
    }

    #[test]
    fn interval_keys_parse_and_are_floored() {
        let one = |k: &'static str, v: &'static str| {
            Config::from_lookup(move |key| (key == k).then(|| v.to_owned()))
        };
        let d = Config::default();
        assert!(d.feed.scheduler, "a daemon that never refreshes is not one");
        assert_eq!(d.feed.refresh_interval, Duration::from_secs(3600));
        assert_eq!(d.maintenance.interval, Duration::from_secs(86_400));
        assert_eq!(d.maintenance.retain_events_days, 30);
        assert_eq!(d.maintenance.retain_events_max_rows, 100_000);

        let c = Config::from_lookup(|k| {
            [
                ("UGUISU_FEED_SCHEDULER", "off"),
                ("UGUISU_FEED_REFRESH_INTERVAL_SECS", "900"),
                ("UGUISU_MAINTENANCE_INTERVAL_SECS", "3600"),
                ("UGUISU_EVENTS_RETAIN_DAYS", "0"),
                ("UGUISU_EVENTS_RETAIN_MAX_ROWS", "0"),
            ]
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| (*v).to_owned())
        })
        .unwrap();
        assert!(!c.feed.scheduler);
        assert_eq!(c.feed.refresh_interval, Duration::from_secs(900));
        assert_eq!(c.maintenance.interval, Duration::from_secs(3600));
        // Zero is "no limit" here, as it is for the archive policy's
        // backlog and age bounds — not "prune everything".
        assert_eq!(c.maintenance.retain_events_days, 0);
        assert_eq!(c.maintenance.retain_events_max_rows, 0);
        assert_eq!(c.origin(env_keys::FEED_SCHEDULER), Origin::Env);

        // The floor is the part that matters: an interval below it would
        // have Uguisu polling an origin that cannot ask it to stop.
        for key in [
            env_keys::FEED_REFRESH_INTERVAL_SECS,
            env_keys::MAINTENANCE_INTERVAL_SECS,
        ] {
            let err = Config::from_lookup(|k| (k == key).then(|| "59".to_owned())).unwrap_err();
            assert_eq!(err.key, key);
            assert!(err.message.contains("at least 60"), "{}", err.message);
            assert!(Config::from_lookup(|k| (k == key).then(|| "60".to_owned())).is_ok());
            let year = MAX_INTERVAL_SECS.to_string();
            assert!(Config::from_lookup(|k| (k == key).then(|| year.clone())).is_ok());
            let err =
                Config::from_lookup(|k| (k == key).then(|| (MAX_INTERVAL_SECS + 1).to_string()))
                    .unwrap_err();
            assert!(err.message.contains("at most"), "{}", err.message);
        }
        assert!(one("UGUISU_FEED_SCHEDULER", "sometimes").is_err());
    }

    #[test]
    fn allow_private_hosts_is_split_and_lowercased() {
        let c = cfg(&[("UGUISU_HTTP_ALLOW_PRIVATE_HOSTS", " NAS.local, printer ,")]).unwrap();
        assert_eq!(c.network.allow_private_hosts, vec!["nas.local", "printer"]);
    }
    fn layered(stored: &[(&str, &str)], env: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let stored: BTreeMap<String, String> = stored
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_layers(&Layers::new(stored, move |k: &str| env.get(k).cloned()))
    }

    #[test]
    fn stored_beats_default_environment_beats_both() {
        let c = layered(&[], &[]).unwrap();
        assert_eq!(c.feed.refresh_concurrency, 8);
        assert_eq!(
            c.origin(env_keys::FEED_REFRESH_CONCURRENCY),
            Origin::Default
        );

        let c = layered(&[(env_keys::FEED_REFRESH_CONCURRENCY, "4")], &[]).unwrap();
        assert_eq!(c.feed.refresh_concurrency, 4);
        assert_eq!(
            c.origin(env_keys::FEED_REFRESH_CONCURRENCY),
            Origin::Settings
        );

        let c = layered(
            &[(env_keys::FEED_REFRESH_CONCURRENCY, "4")],
            &[(env_keys::FEED_REFRESH_CONCURRENCY, "16")],
        )
        .unwrap();
        assert_eq!(
            c.feed.refresh_concurrency, 16,
            "an operator who exported it gets to rely on it"
        );
        assert_eq!(c.origin(env_keys::FEED_REFRESH_CONCURRENCY), Origin::Env);
        assert!(c.is_pinned(env_keys::FEED_REFRESH_CONCURRENCY));
        assert_eq!(c.stored(env_keys::FEED_REFRESH_CONCURRENCY), Some("4"));
    }

    /// A credential can only get into the table by a hand-written row — the
    /// API and the CLI both refuse an unstorable key — and a report that
    /// echoed it would hand the operator's Podcast Index secret to anything
    /// that can read `GET /api/v1/settings`.
    #[test]
    fn a_stored_secret_is_reported_but_never_echoed() {
        let c = layered(&[(env_keys::PODCASTINDEX_SECRET, "s3cret-value")], &[]).unwrap();

        let row = c
            .describe()
            .into_iter()
            .find(|k| k.key == env_keys::PODCASTINDEX_SECRET)
            .expect("every key is described");
        assert_eq!(
            row.value, "(unset)",
            "the engine never reads a secret from the database, so nothing \
             is in force"
        );
        assert_eq!(
            row.stored.as_deref(),
            Some(REDACTED),
            "the row is reported as present, not as its value"
        );

        let unused = c.unused_stored();
        let (key, value, reason) = unused
            .iter()
            .find(|(key, _, _)| key == env_keys::PODCASTINDEX_SECRET)
            .expect("a stored unstorable key is reported");
        assert_eq!(key, env_keys::PODCASTINDEX_SECRET);
        assert_eq!(value, REDACTED);
        assert_eq!(*reason, "not persistable");

        let printed = format!("{:?}{:?}", c.describe(), unused);
        assert!(
            !printed.contains("s3cret-value"),
            "nothing a report prints may be the credential: {printed}"
        );

        // And the value in force, when the environment does supply one.
        let from_env = layered(&[], &[(env_keys::PODCASTINDEX_SECRET, "s3cret-value")]).unwrap();
        let described = format!("{:?}", from_env.describe());
        assert!(described.contains(REDACTED) && !described.contains("s3cret-value"));
    }

    #[test]
    fn an_empty_value_means_unset() {
        // `UGUISU_APPLE_LANG=` in a compose file means "leave it alone",
        // not "set it to nothing" — and the same has to hold for a row
        // somebody blanked through the API.
        let c = layered(
            &[(env_keys::FEED_REFRESH_CONCURRENCY, "   ")],
            &[(env_keys::FEED_REFRESH_CONCURRENCY, "")],
        )
        .unwrap();
        assert_eq!(c.feed.refresh_concurrency, 8);
        assert_eq!(
            c.origin(env_keys::FEED_REFRESH_CONCURRENCY),
            Origin::Default
        );
        assert!(!c.is_pinned(env_keys::FEED_REFRESH_CONCURRENCY));
    }

    #[test]
    fn the_database_supplies_no_secret() {
        let c = layered(
            &[
                (env_keys::PODCASTINDEX_KEY, "stolen"),
                (env_keys::PODCASTINDEX_SECRET, "stolen"),
                (env_keys::DATA_DIR, "/tmp/elsewhere"),
                (env_keys::MEDIA_DIR, "/tmp/elsewhere"),
                (env_keys::HTTP_ALLOW_PRIVATE_HOSTS, "169.254.169.254"),
            ],
            &[],
        )
        .unwrap();
        assert!(c.discovery.podcastindex.key.is_none());
        assert!(c.discovery.podcastindex.secret.is_none());
        assert_eq!(c.data.data_dir, None);
        assert_eq!(c.data.media_dir, None);
        assert!(c.discovery.network.allow_private_hosts.is_empty());

        // They stay in the table and are reported, rather than being
        // silently honoured or silently dropped.
        let unused = c.unused_stored();
        assert_eq!(unused.len(), 5);
        assert!(unused.iter().all(|(_, _, why)| *why == "not persistable"));
    }

    #[test]
    fn a_stored_value_validates_like_env() {
        let base = layered(&[], &[]).unwrap();

        // The same message the environment variable produces, because it
        // is the same parser.
        let stored_err = base
            .with_stored(env_keys::FEED_MAX_BYTES, Some("10"))
            .unwrap_err();
        let env_err =
            Config::from_lookup(|k| (k == env_keys::FEED_MAX_BYTES).then(|| "10".to_owned()))
                .unwrap_err();
        assert_eq!(stored_err, env_err);

        // And a cross-field rule no per-key validator could see: the
        // per-host limit may not exceed the global one.
        let err = base
            .with_stored(env_keys::DOWNLOAD_PER_HOST_CONCURRENCY, Some("16"))
            .unwrap_err();
        assert_eq!(err.key, env_keys::DOWNLOAD_PER_HOST_CONCURRENCY);

        let ok = base
            .with_stored(env_keys::DOWNLOAD_PER_HOST_CONCURRENCY, Some("2"))
            .unwrap();
        assert_eq!(ok.download.per_host_concurrency, 2);
        assert_eq!(base.download.per_host_concurrency, 1, "nothing was mutated");

        // Clearing puts the default back.
        let cleared = ok
            .with_stored(env_keys::DOWNLOAD_PER_HOST_CONCURRENCY, None)
            .unwrap();
        assert_eq!(cleared.download.per_host_concurrency, 1);
        assert_eq!(
            cleared.origin(env_keys::DOWNLOAD_PER_HOST_CONCURRENCY),
            Origin::Default
        );
    }

    #[test]
    fn describe_names_an_ignored_stored_value() {
        let c = layered(
            &[
                (env_keys::ARCHIVE_AUTO_DOWNLOAD, "true"),
                (env_keys::FEED_RETAIN_FETCHES, "10"),
                ("UGUISU_SOMETHING_A_LATER_VERSION_REMOVED", "1"),
            ],
            &[(env_keys::ARCHIVE_AUTO_DOWNLOAD, "false")],
        )
        .unwrap();

        // The dangerous one: the operator said no, so the answer is no.
        assert!(!c.archive.auto_download);
        let row = c
            .describe()
            .into_iter()
            .find(|d| d.key == env_keys::ARCHIVE_AUTO_DOWNLOAD)
            .unwrap();
        assert_eq!(row.origin, Origin::Env);
        assert_eq!(row.value, "false");
        assert_eq!(row.stored.as_deref(), Some("true"));
        assert!(row.pinned, "a user who stored `true` has to be told why");
        assert!(row.persistable);
        assert!(row.live);

        let row = c
            .describe()
            .into_iter()
            .find(|d| d.key == env_keys::FEED_RETAIN_FETCHES)
            .unwrap();
        assert_eq!(row.origin, Origin::Settings);
        assert!(!row.pinned);

        let unused = c.unused_stored();
        assert_eq!(
            unused,
            vec![
                (
                    env_keys::ARCHIVE_AUTO_DOWNLOAD.to_owned(),
                    "true".to_owned(),
                    "pinned by the environment"
                ),
                (
                    "UGUISU_SOMETHING_A_LATER_VERSION_REMOVED".to_owned(),
                    "1".to_owned(),
                    "unknown"
                ),
            ]
        );
    }

    #[test]
    fn origins_rank_in_precedence_order() {
        assert!(Origin::Default < Origin::Settings);
        assert!(Origin::Settings < Origin::Env);
        assert!(Origin::Env < Origin::Cli);
        assert_eq!(Origin::Settings.as_str(), "settings");
        assert_eq!(Origin::Env.rank(), 2);
        assert_eq!(Origin::Cli.to_string(), "cli");
    }
}
