//! The download job system (`docs/DOWNLOAD_ENGINE.md`).
//!
//! A persistent queue with bounded global and per-host concurrency,
//! streaming downloads to `.part` files with inline hashing, validated
//! `Range` resumption, atomic finalization, retry schedules, progress
//! events and startup crash recovery. Never exposes a partial file as
//! archive content.
//!
//! Layering: this crate owns the state machine ([`state`]), the retry plan
//! ([`retry`]), error classification ([`error`]), destination paths
//! ([`paths`]), progress smoothing ([`progress`]), the disk-space probe
//! ([`space`]) and container sniffing ([`sniff`]); it depends on
//! `uguisu-core`, `uguisu-http` (the only HTTP client) and `uguisu-storage`
//! (the persistence port). The engine wires it and publishes its events.
//!
//! State machine: ADR 0018; resume and finalization: ADR 0019;
//! destinations: the rendered template path (ADR 0022), with the
//! identifier layout of ADR 0020 as the fallback.

pub mod deps;
pub mod error;
pub mod handle;
pub mod paths;
pub mod progress;
pub mod retry;
pub(crate) mod scheduler;
pub mod service;
pub mod sniff;
pub mod space;
pub mod state;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod worker;

pub use deps::{
    CollectingSink, Deps, DestinationRequest, DestinationResolver, EventSink, IdentityLayout,
    NoopSink,
};
pub use error::{DownloadError, classify};
pub use handle::{JobHandle, StopReason};
pub use paths::{Destination, PathError};
pub use progress::ProgressMeter;
pub use retry::RetryPlan;
pub use service::{
    DownloadService, DownloadStats, EnqueueOutcome, EnqueueSummary, JobDetail, JobFilter, JobPage,
    ReconcileReport, SkippedEpisode, part_files,
};
pub use space::{FixedSpace, Fs4Probe, SpaceProbe};
pub use state::{InvalidTransition, JobEvent, reason, transition};
pub use worker::{Claimed, JobOutcome, claim, run_job};
