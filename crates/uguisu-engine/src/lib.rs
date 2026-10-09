//! The application layer that every frontend uses.
//!
//! [`Engine`] composes storage, the feed HTTP client, the discovery stack and
//! the event bus into the services the CLI and the server call: library,
//! refresh, inspect, downloads, archive, and the service modules
//! ([`scheduler`], [`settings`], [`search`], [`discovery`]).
//!
//! Two rules hold across all of them. Nothing long-lived starts unless a
//! frontend asks: [`Engine::start_downloads`], [`Engine::start_refresh_scheduler`]
//! and [`Engine::start_search_index`] are the only entry points, and only
//! `uguisu serve` calls them. And the engine never enqueues an episode by
//! itself — it emits `episode.discovered` and the archive policy decides
//! (ADR 0010, ADR 0018, ADR 0023).
//!
//! One process owns a data directory: [`Engine::open`] takes `uguisu.lock`
//! and fails with [`UguisuError::Locked`] otherwise (ADR 0016).
//!
//! See `docs/ARCHITECTURE.md` §4, `docs/FEED_ENGINE.md`,
//! `docs/DOWNLOAD_ENGINE.md` and `docs/SERVICE.md`.
pub mod archive;
pub mod archive_meta;
pub mod artwork;
pub mod auth;
pub mod coordinator;
pub mod db;
pub mod discovery;
pub mod duplicates;
pub mod events;
pub mod import;
pub mod inspect;
pub mod library;
pub mod lock;
pub mod migration;
pub mod opml;
pub mod orphans;
pub mod rebuild;
pub mod refresh;
pub mod restore;
pub mod scheduler;
pub mod search;
pub mod settings;
pub mod sync;
pub mod tagging;

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uguisu_core::UguisuError;
use uguisu_core::config::{Config, DataConfig};
use uguisu_discovery::{Discovery, assemble_with_cache_store};
use uguisu_download::{Deps, DownloadService, Fs4Probe};
use uguisu_http::{ClientConfig, HostThrottles, HttpClient, Profile, RetryPolicy, ThrottleConfig};
use uguisu_storage::Storage;

pub use coordinator::{Coordinator, RefreshAllEntry};
pub use events::{EventBus, Subscription};
pub use inspect::{Inspection, inspect};
pub use lock::LockFile;
pub use refresh::RefreshOptions;

/// Everything the engine needs to start.
///
/// The whole `Config`, not a subset of its fields: the settings layer needs the
/// provenance that came with it. Which layer a value arrived from decides
/// whether the settings API may change it (ADR 0028), and a struct that
/// copied five fields across threw exactly that away.
pub type EngineConfig = Config;

/// A mutex guard, taking a poisoned lock rather than propagating a panic
/// that already happened somewhere else. The state behind these locks is
/// a set of identifiers and a handle; there is no invariant a panicking
/// thread could have left half-applied.
pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The three HTTP clients the engine owns, all under one network policy
/// (`docs/SECURITY.md` §3.1) and each with the profile its traffic needs.
///
/// Built once, here: every one of them captures the configuration it was
/// given, which is why the `UGUISU_HTTP_*` keys are `restart_required`
/// rather than live (ADR 0028).
fn clients(
    network: &uguisu_core::config::NetworkConfig,
) -> Result<(HttpClient, HttpClient, HttpClient), UguisuError> {
    let named = |what: &str, e: uguisu_http::HttpError| {
        UguisuError::Config(format!("{what} http client: {e}"))
    };
    let feed = HttpClient::new(
        Profile::Feed,
        ClientConfig {
            retry: RetryPolicy::default(),
            ..ClientConfig::from_network(network)
        },
    )
    .map_err(|e| named("feed", e))?;
    // Media transfers: no client-side retries (the queue owns the retry
    // schedule) and no decompression.
    let media = HttpClient::new(
        Profile::Media,
        ClientConfig {
            retry: RetryPolicy::none(),
            ..ClientConfig::from_network(network)
        },
    )
    .map_err(|e| named("media", e))?;
    // Artwork: untrusted URLs, with a cap several orders of magnitude
    // below a media transfer.
    let artwork = HttpClient::new(
        Profile::Artwork,
        ClientConfig {
            retry: RetryPolicy::default(),
            ..ClientConfig::from_network(network)
        },
    )
    .map_err(|e| named("artwork", e))?;
    Ok((feed, media, artwork))
}

struct Inner {
    /// What the process was started with: defaults, then the environment
    /// (and, later, command-line flags). Kept because every re-read of the
    /// `settings` table has to be merged onto the same base — reading the
    /// environment again could give a different answer, and a
    /// configuration nobody asked for.
    base_config: EngineConfig,
    /// The configuration in force, and the stored rows it could not use.
    /// Written only by `reload_settings`; a snapshot is handed out whole,
    /// so no caller can read half of one change.
    effective: std::sync::RwLock<settings::Effective>,
    storage: Storage,
    feed_client: HttpClient,
    discovery: Discovery,
    /// Fetches podcast artwork: small bodies, untrusted URLs.
    artwork_client: HttpClient,
    bus: EventBus,
    coordinator: Coordinator,
    downloads: DownloadService,
    /// The task that archives finished downloads, once started.
    archive_watch: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The feed-refresh loop and what it is doing (ADR 0027).
    scheduler: scheduler::SchedulerState,
    /// The background search-index build, once started (ADR 0029).
    search_build: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The background check that recorded files exist, once started.
    archive_check: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Stops the engine's own background tasks on close.
    shutdown: CancellationToken,
    // Released when the last handle drops.
    _lock: LockFile,
}

/// A cheap-to-clone handle on the running engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("data_dir", &self.inner.base_config.data.data_dir)
            .field("database", &self.inner.storage.path())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Locks the data directory, opens (and migrates) the database, builds
    /// the HTTP and discovery stacks and reconciles the download queue
    /// (jobs interrupted by a crash go back to `queued`; no worker starts).
    pub async fn open(config: EngineConfig) -> Result<Self, UguisuError> {
        Self::open_inner(
            config,
            #[cfg(feature = "testing")]
            None,
        )
        .await
    }

    /// [`open`](Self::open) with a fail point armed for crash tests.
    #[cfg(feature = "testing")]
    pub async fn open_with_injector(
        config: EngineConfig,
        injector: Option<Arc<uguisu_download::deps::FailInjector>>,
    ) -> Result<Self, UguisuError> {
        Self::open_inner(config, injector).await
    }

    async fn open_inner(
        config: EngineConfig,
        #[cfg(feature = "testing")] injector: Option<Arc<uguisu_download::deps::FailInjector>>,
    ) -> Result<Self, UguisuError> {
        let dir = config
            .data
            .data_dir()
            .map_err(|e| UguisuError::Config(e.to_string()))?;
        std::fs::create_dir_all(&dir).map_err(|e| {
            UguisuError::Config(format!(
                "cannot create data directory {}: {e}",
                dir.display()
            ))
        })?;
        let lock = LockFile::acquire(&dir.join(DataConfig::LOCK_FILE))?;
        let storage = Storage::open_path(&dir.join(DataConfig::DATABASE_FILE)).await?;
        // The stored layer (ADR 0028) comes before anything is built: the
        // stacks below capture their configuration once, which is what
        // makes a key `restart_required` rather than ignored.
        let effective = {
            let mut reader = storage.reader().await?;
            settings::merge_stored(&config, &uguisu_storage::settings::list(&mut reader).await?)
        };
        let built = Arc::clone(&effective.config);
        let (feed_client, media_client, artwork_client) = clients(&built.discovery.network)?;
        // The discovery cache gets a second tier over the database, so a
        // restart does not throw away what providers already answered
        // (ADR 0030). Derived data only: every row expires on its own.
        let discovery = assemble_with_cache_store(
            built.discovery.clone(),
            Some(Arc::new(discovery::SqliteCacheStore::new(storage.clone()))),
        )
        .map_err(|e| UguisuError::Config(format!("discovery: {e}")))?;
        let media_dir = config
            .data
            .media_dir()
            .map_err(|e| UguisuError::Config(e.to_string()))?;
        let bus = EventBus::default();
        let deps = Deps::new(
            storage.clone(),
            media_client,
            Arc::new(HostThrottles::new(ThrottleConfig::concurrency_only(
                built.download.per_host_concurrency.max(1),
            ))),
            Arc::new(bus.clone()),
            built.download.clone(),
            media_dir.clone(),
            Arc::new(Fs4Probe),
            CancellationToken::new(),
        );
        let deps =
            deps.with_destinations(Arc::new(archive::TemplateDestinations::new(&built.archive)));
        #[cfg(feature = "testing")]
        let deps = deps.with_injector(injector);
        let downloads = DownloadService::new(deps);
        let report = downloads.reconcile(false).await?;
        if report.recovered > 0 || report.finalization_lost > 0 || !report.orphan_parts.is_empty() {
            tracing::warn!(
                recovered = report.recovered,
                finalized = report.finalized,
                finalization_lost = report.finalization_lost,
                orphan_parts = report.orphan_parts.len(),
                "download queue reconciled after an unclean stop"
            );
        }
        tracing::info!(data_dir = %dir.display(), media_dir = %media_dir.display(), "engine open");
        let engine = Self {
            inner: Arc::new(Inner {
                effective: std::sync::RwLock::new(effective.clone()),
                base_config: config,
                storage,
                feed_client,
                discovery,
                artwork_client,
                bus,
                coordinator: Coordinator::default(),
                downloads,
                archive_watch: std::sync::Mutex::new(None),
                scheduler: scheduler::SchedulerState::default(),
                search_build: std::sync::Mutex::new(None),
                archive_check: std::sync::Mutex::new(None),
                shutdown: CancellationToken::new(),
                _lock: lock,
            }),
        };
        engine.install_settings(effective).await?;
        // Repair the archive record from the database alone. Whether the
        // files are still there is the server's background check: a `stat`
        // per file would make every command wait on the archive's size.
        engine.repair_archive().await?;
        Ok(engine)
    }

    /// The configuration in force.
    ///
    /// A snapshot, not a borrow: a stored setting can change while the
    /// process runs, and a caller that read one value must not find the
    /// next one changed underneath it. Cheap to clone, and nothing holds
    /// the lock across an await.
    #[must_use]
    pub fn config(&self) -> Arc<EngineConfig> {
        self.inner
            .effective
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .clone()
    }

    /// The database.
    #[must_use]
    pub fn storage(&self) -> &Storage {
        &self.inner.storage
    }

    /// The HTTP client used for feed fetches (`Profile::Feed`).
    #[must_use]
    pub fn feed_client(&self) -> &HttpClient {
        &self.inner.feed_client
    }

    /// The client artwork is fetched with (`Profile::Artwork`).
    #[must_use]
    pub fn artwork_client(&self) -> &HttpClient {
        &self.inner.artwork_client
    }

    /// The discovery stack (search engine, providers, resolver).
    #[must_use]
    pub fn discovery(&self) -> &Discovery {
        &self.inner.discovery
    }

    /// The event bus.
    #[must_use]
    pub fn bus(&self) -> &EventBus {
        &self.inner.bus
    }

    /// In-flight refresh bookkeeping.
    #[must_use]
    pub fn coordinator(&self) -> &Coordinator {
        &self.inner.coordinator
    }

    /// Subscribes to committed domain events.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        self.inner.bus.subscribe()
    }

    /// The download queue (enqueue, inspect, control). Workers run only
    /// after [`start_downloads`](Self::start_downloads).
    #[must_use]
    pub fn downloads(&self) -> &DownloadService {
        &self.inner.downloads
    }

    /// Starts the download workers in this process, and with them the
    /// task that records each finished transfer as an archive artifact
    /// (idempotent). A frontend that only inspects the queue starts
    /// neither.
    pub fn start_downloads(&self) {
        self.inner.downloads.start();
        self.start_archive_watcher();
    }

    /// Stops the download workers (running jobs are parked as
    /// `queued(shutdown)` within the configured grace period) and closes
    /// the database pools. The lock is released when the last handle
    /// drops.
    pub async fn close(&self) {
        // Tell the watcher to stop, then park the queue, then wait for the
        // watcher, then close the pools. The queue goes first because a
        // worker can be holding the single writer connection that the
        // watcher's last registration needs; waiting for the watcher before
        // releasing the workers would deadlock the two against each other.
        // Nothing here closes the database, so the watcher's final write is
        // still safe.
        let config = self.config();
        let grace = config.download.shutdown_grace;
        self.inner.shutdown.cancel();
        self.inner.downloads.shutdown(grace).await;
        // The scheduler starts no new refresh once the token is cancelled,
        // and the ones it has see the same token through `RefreshOptions`,
        // so this waits for a fetch to abort rather than for a feed to
        // arrive.
        self.stop_refresh_scheduler(grace).await;
        // A half-built index is not a loss: the state row says it is not
        // ready and the next start builds it again.
        self.stop_search_index(grace).await;
        self.stop_archive_check(grace).await;
        let task = self
            .inner
            .archive_watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            // Bounded: an archive step that will not finish must not hold
            // the process open. Whatever it did not record is registered by
            // the next start's reconciliation.
            if tokio::time::timeout(grace, task).await.is_err() {
                tracing::warn!(
                    "archive watcher did not stop in time; reconciliation will finish its work"
                );
            }
        }
        // With the watcher stopped and the queue parked, nothing can mark
        // a manifest stale any more, so this is the one moment where
        // writing them all is meaningful: after a clean close, every
        // manifest agrees with the index. Bounded by the same grace period
        // as everything else - what is not written stays marked stale.
        if config.archive.manifests {
            let deadline = std::time::Instant::now() + grace;
            self.flush_manifests_until(deadline).await;
        }
        self.inner.storage.close().await;
    }
}
