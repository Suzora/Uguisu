//! What the download engine needs from its host: storage, the media HTTP
//! client, an event sink, configuration and the media directory.

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uguisu_core::config::DownloadConfig;
use uguisu_core::events::Event;
use uguisu_http::{HostThrottles, HttpClient};
use uguisu_storage::Storage;

use crate::space::SpaceProbe;

/// Receives events after their transaction committed (progress events are
/// live-only and go through here without a transaction).
pub trait EventSink: Send + Sync + std::fmt::Debug {
    /// Delivers events in order.
    fn publish(&self, events: &[Event]);
}

/// A sink that drops everything (tests, tools).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

impl EventSink for NoopSink {
    fn publish(&self, _events: &[Event]) {}
}

/// A sink that keeps every event (tests).
#[derive(Debug, Default)]
pub struct CollectingSink {
    events: std::sync::Mutex<Vec<Event>>,
}

impl CollectingSink {
    /// Everything published so far.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Event names in order.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.events().iter().map(Event::name).collect()
    }
}

impl EventSink for CollectingSink {
    fn publish(&self, events: &[Event]) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(events);
    }
}

/// Everything a resolver needs to name one job's destination.
#[derive(Debug, Clone, Copy)]
pub struct DestinationRequest<'a> {
    /// The podcast the episode belongs to.
    pub podcast: &'a uguisu_core::model::Podcast,
    /// The episode being downloaded.
    pub episode: &'a uguisu_core::model::Episode,
    /// The file extension chosen for the transfer, without the dot.
    pub extension: &'a str,
}

/// Names where a finished download should end up.
///
/// This is the seam the archive template plugs into (ADR 0022). The
/// download engine does not know what a template is: it asks, checks that
/// the answer is a usable relative path that no other job has claimed, and
/// otherwise keeps the identifier layout, which is always
/// available because it is built from identifiers alone.
pub trait DestinationResolver: Send + Sync + std::fmt::Debug {
    /// The target path relative to the media directory, POSIX separators,
    /// or `None` to use the identifier layout.
    fn target(&self, request: &DestinationRequest<'_>) -> Option<String>;
}

/// The identifier layout: `<podcast id>/<episode id>.<ext>`.
///
/// Always answers, never collides, and needs nothing but the identifiers —
/// which is what makes it a safe fallback when a template cannot be
/// rendered.
#[derive(Debug, Default, Clone, Copy)]
pub struct IdentityLayout;

impl DestinationResolver for IdentityLayout {
    fn target(&self, _request: &DestinationRequest<'_>) -> Option<String> {
        None
    }
}

/// Failure injection for crash tests: the worker returns without touching
/// the database when it reaches an armed point ("silent death").
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    /// After at least this many bytes of the attempt were written (unflushed).
    AfterBytes(u64),
    /// Before the `downloading → finalizing` write.
    BeforeFinalizingWrite,
    /// After it, before the rename.
    AfterFinalizingWrite,
    /// After the rename, before the `finalizing → completed` write.
    AfterRename,
    /// Fail the file write at this byte offset with the given I/O error kind.
    WriteError(u64, std::io::ErrorKind),
}

/// Holds the armed fail point; one shot.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default)]
pub struct FailInjector {
    point: std::sync::Mutex<Option<FailPoint>>,
    hit: std::sync::atomic::AtomicBool,
}

#[cfg(any(test, feature = "testing"))]
impl FailInjector {
    /// Arms one point.
    #[must_use]
    pub fn armed(point: FailPoint) -> Arc<Self> {
        let s = Self::default();
        *s.point
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(point);
        Arc::new(s)
    }

    /// The armed point, if any and not yet hit.
    #[must_use]
    pub fn armed_point(&self) -> Option<FailPoint> {
        if self.hit.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        *self
            .point
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Marks the point as hit (it fires once).
    pub fn fire(&self) {
        self.hit.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether the point fired.
    #[must_use]
    pub fn was_hit(&self) -> bool {
        self.hit.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Everything a worker and the scheduler need.
#[derive(Debug, Clone)]
pub struct Deps {
    /// The database.
    pub storage: Storage,
    /// A [`Profile::Media`](uguisu_http::Profile::Media) client.
    pub client: HttpClient,
    /// Per-host admission.
    pub hosts: Arc<HostThrottles>,
    /// Where events go after commit.
    pub sink: Arc<dyn EventSink>,
    /// Settings.
    pub config: DownloadConfig,
    /// Root of the media tree (created lazily).
    pub media_dir: PathBuf,
    /// Free-space query.
    pub space: Arc<dyn SpaceProbe>,
    /// Names the final path of a finished download.
    pub destinations: Arc<dyn DestinationResolver>,
    /// Cancelled on shutdown; every job token is a child of it.
    pub shutdown: CancellationToken,
    /// Crash injection (tests only).
    #[cfg(any(test, feature = "testing"))]
    pub injector: Option<Arc<FailInjector>>,
}

impl Deps {
    /// Bundles the dependencies without crash injection. Consumers use this
    /// rather than a struct literal so they compile whether or not the
    /// `testing` feature (which adds the injector field) is enabled by
    /// another crate in the build.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: Storage,
        client: HttpClient,
        hosts: Arc<HostThrottles>,
        sink: Arc<dyn EventSink>,
        config: DownloadConfig,
        media_dir: PathBuf,
        space: Arc<dyn SpaceProbe>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            storage,
            client,
            hosts,
            sink,
            config,
            media_dir,
            space,
            destinations: Arc::new(IdentityLayout),
            shutdown,
            #[cfg(any(test, feature = "testing"))]
            injector: None,
        }
    }

    /// Uses `resolver` to name finished downloads instead of the
    /// identifier layout.
    #[must_use]
    pub fn with_destinations(mut self, resolver: Arc<dyn DestinationResolver>) -> Self {
        self.destinations = resolver;
        self
    }

    /// Arms crash injection (tests only).
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn with_injector(mut self, injector: Option<Arc<FailInjector>>) -> Self {
        self.injector = injector;
        self
    }

    /// The armed fail point, when injection is compiled in and armed.
    #[must_use]
    pub fn fail_point(&self) -> Option<FailPointView> {
        #[cfg(any(test, feature = "testing"))]
        {
            self.injector
                .as_ref()
                .and_then(|i| i.armed_point().map(|p| FailPointView { point: p }))
        }
        #[cfg(not(any(test, feature = "testing")))]
        {
            None
        }
    }

    /// Marks the armed point as fired.
    pub fn fire_fail_point(&self) {
        #[cfg(any(test, feature = "testing"))]
        if let Some(i) = &self.injector {
            i.fire();
        }
    }
}

/// The armed point, as seen by the worker.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Copy)]
pub struct FailPointView {
    /// The point.
    pub point: FailPoint,
}

/// The armed point, as seen by the worker (never present without injection).
#[cfg(not(any(test, feature = "testing")))]
#[derive(Debug, Clone, Copy)]
pub struct FailPointView {}
