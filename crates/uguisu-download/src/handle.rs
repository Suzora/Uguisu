//! The in-memory side of a running job: its cancellation token, why it
//! was told to stop, and its live progress.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use uguisu_core::download::ProgressSnapshot;

/// Why a running job's token was cancelled. `User` means the database
/// already holds the final state (cancelled or paused by a command); the
/// worker persists the others itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// A user command already wrote `cancelled`/`paused`.
    User,
    /// `pause-all`: the worker writes `paused(paused_all)`.
    PausedAll,
    /// Another worker hit a full disk: the worker writes `paused(disk_full)`.
    DiskFull,
    /// Graceful shutdown: the worker writes `queued(shutdown)`.
    Shutdown,
}

/// Control block of a running job.
#[derive(Debug)]
pub struct JobHandle {
    cancel: CancellationToken,
    stop_reason: Mutex<Option<StopReason>>,
    progress: Mutex<Option<ProgressSnapshot>>,
}

impl JobHandle {
    /// A handle whose token is a child of `parent` (the service's shutdown token).
    #[must_use]
    pub fn new(parent: &CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            cancel: parent.child_token(),
            stop_reason: Mutex::new(None),
            progress: Mutex::new(None),
        })
    }

    /// The job's token (passed to the HTTP request).
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Asks the worker to stop for `reason`.
    pub fn stop(&self, reason: StopReason) {
        let mut r = self
            .stop_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if r.is_none() {
            *r = Some(reason);
        }
        drop(r);
        self.cancel.cancel();
    }

    /// Why the worker was told to stop, if it was.
    #[must_use]
    pub fn stop_reason(&self) -> Option<StopReason> {
        *self
            .stop_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the token fired.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Publishes a live snapshot.
    pub fn set_progress(&self, snapshot: ProgressSnapshot) {
        *self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(snapshot);
    }

    /// The latest snapshot.
    #[must_use]
    pub fn progress(&self) -> Option<ProgressSnapshot> {
        self.progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
