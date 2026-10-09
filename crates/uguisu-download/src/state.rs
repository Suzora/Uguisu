//! The download job state machine (ADR 0018), as one pure table.
//!
//! Every persisted state change consults [`transition`] first and then
//! writes the result with a compare-and-set update, so the database can
//! never hold a transition this table does not allow.

use uguisu_core::download::DownloadState;

/// The stable `state_reason` vocabulary (`docs/DOWNLOAD_ENGINE.md`).
pub mod reason {
    /// Queued by a user command.
    pub const USER: &str = "user";
    /// Re-queued from `failed`/`cancelled` by a user retry or enqueue.
    pub const REQUEUED: &str = "requeued";
    /// Found `downloading` at startup: the previous process died.
    pub const RECOVERED: &str = "recovered";
    /// Parked by a graceful shutdown.
    pub const SHUTDOWN: &str = "shutdown";
    /// Re-queued from `paused`.
    pub const RESUMED: &str = "resumed";
    /// The episode's enclosure URL changed; the transfer restarts.
    pub const SOURCE_CHANGED: &str = "source_changed";
    /// Paused by `pause-all`.
    pub const PAUSED_ALL: &str = "paused_all";
    /// Paused because the media file system is full.
    pub const DISK_FULL: &str = "disk_full";
    /// Failed: the attempt budget is spent.
    pub const MAX_ATTEMPTS: &str = "max_attempts";
    /// Failed: the error kind is never retried.
    pub const NOT_RETRYABLE: &str = "not_retryable";
    /// Failed: the body did not validate.
    pub const VALIDATION: &str = "validation";
    /// Failed: a different file already exists at the target path.
    pub const TARGET_EXISTS: &str = "target_exists";
    /// Failed: the rename after a complete download failed.
    pub const FINALIZATION: &str = "finalization";
    /// Failed: neither the `.part` nor the target survived a crash while finalizing.
    pub const FINALIZATION_LOST: &str = "finalization_lost";
    /// Failed: the episode or its enclosure no longer exists.
    pub const SOURCE_MISSING: &str = "source_missing";
    /// Re-queued from `completed` because the archived file is gone (ADR 0060).
    pub const REDOWNLOAD: &str = "redownload";
}

/// What happened to a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobEvent {
    /// A worker claimed the job.
    Claim,
    /// The whole body is on disk and validated.
    BodyComplete,
    /// A retryable failure with attempts left.
    RetryableError,
    /// A failure that ends the job (`not_retryable`, `max_attempts`, `validation`, …).
    FatalError,
    /// The media file system is full.
    DiskFull,
    /// The user cancelled.
    UserCancel,
    /// The user paused this job.
    UserPause,
    /// The user resumed this job.
    UserResume,
    /// The user retried (or re-enqueued) this job.
    UserRetry,
    /// `pause-all`.
    PauseAll,
    /// `resume-all`.
    ResumeAll,
    /// Graceful shutdown parked the job.
    Shutdown,
    /// Startup found the job held by a dead worker.
    Recovered,
    /// The rename succeeded.
    RenameOk,
    /// The rename failed for good.
    RenameFailed,
    /// Startup found the target of a `finalizing` job in place.
    RecoveredComplete,
    /// Startup found neither `.part` nor target of a `finalizing` job.
    FinalizationLost,
    /// The user asked for a completed job's file again because it is gone.
    Redownload,
}

impl JobEvent {
    /// Every variant.
    pub const ALL: [Self; 18] = [
        Self::Claim,
        Self::BodyComplete,
        Self::RetryableError,
        Self::FatalError,
        Self::DiskFull,
        Self::UserCancel,
        Self::UserPause,
        Self::UserResume,
        Self::UserRetry,
        Self::PauseAll,
        Self::ResumeAll,
        Self::Shutdown,
        Self::Recovered,
        Self::RenameOk,
        Self::RenameFailed,
        Self::RecoveredComplete,
        Self::FinalizationLost,
        Self::Redownload,
    ];
}

/// A transition the table does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot apply {event:?} to a {from} job")]
pub struct InvalidTransition {
    /// State the job was in.
    pub from: DownloadState,
    /// What was attempted.
    pub event: JobEvent,
}

/// The target state of `event` applied to `from`.
#[allow(clippy::match_same_arms)] // one arm per documented edge keeps the table readable
pub const fn transition(
    from: DownloadState,
    event: JobEvent,
) -> Result<DownloadState, InvalidTransition> {
    use DownloadState as S;
    use JobEvent as E;
    let to = match (from, event) {
        (S::Queued | S::Retrying, E::Claim) => S::Downloading,
        (S::Queued | S::Retrying | S::Downloading | S::Paused, E::UserCancel) => S::Cancelled,
        (S::Queued | S::Retrying | S::Downloading, E::UserPause | E::PauseAll) => S::Paused,
        (S::Retrying | S::Failed | S::Cancelled, E::UserRetry) => S::Queued,
        (S::Downloading, E::BodyComplete) => S::Finalizing,
        (S::Downloading, E::RetryableError) => S::Retrying,
        (S::Downloading, E::FatalError) => S::Failed,
        (S::Downloading, E::DiskFull) => S::Paused,
        (S::Downloading, E::Shutdown | E::Recovered) => S::Queued,
        (S::Finalizing, E::RenameOk | E::RecoveredComplete) => S::Completed,
        (S::Finalizing, E::RenameFailed | E::FinalizationLost) => S::Failed,
        (S::Paused, E::UserResume | E::ResumeAll) => S::Queued,
        (S::Completed, E::Redownload) => S::Queued,
        _ => return Err(InvalidTransition { from, event }),
    };
    Ok(to)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::*;
    use DownloadState as S;
    use JobEvent as E;

    #[test]
    fn happy_path_and_recovery_paths() {
        assert_eq!(transition(S::Queued, E::Claim), Ok(S::Downloading));
        assert_eq!(
            transition(S::Downloading, E::BodyComplete),
            Ok(S::Finalizing)
        );
        assert_eq!(transition(S::Finalizing, E::RenameOk), Ok(S::Completed));
        assert_eq!(
            transition(S::Downloading, E::RetryableError),
            Ok(S::Retrying)
        );
        assert_eq!(transition(S::Retrying, E::Claim), Ok(S::Downloading));
        assert_eq!(transition(S::Downloading, E::Recovered), Ok(S::Queued));
        assert_eq!(transition(S::Downloading, E::Shutdown), Ok(S::Queued));
        assert_eq!(
            transition(S::Finalizing, E::RecoveredComplete),
            Ok(S::Completed)
        );
        assert_eq!(
            transition(S::Finalizing, E::FinalizationLost),
            Ok(S::Failed)
        );
        assert_eq!(transition(S::Downloading, E::DiskFull), Ok(S::Paused));
        assert_eq!(transition(S::Paused, E::ResumeAll), Ok(S::Queued));
        assert_eq!(transition(S::Failed, E::UserRetry), Ok(S::Queued));
        assert_eq!(transition(S::Cancelled, E::UserRetry), Ok(S::Queued));
        assert!(
            transition(S::Finalizing, E::UserCancel).is_err(),
            "finalization is not interruptible"
        );
        assert!(transition(S::Queued, E::UserResume).is_err());
        assert!(transition(S::Completed, E::UserRetry).is_err());
    }

    #[test]
    fn completed_leaves_only_by_redownload() {
        for e in E::ALL {
            assert_eq!(
                transition(S::Completed, e).is_ok(),
                e == E::Redownload,
                "{e:?} leaves completed"
            );
        }
        assert_eq!(transition(S::Completed, E::Redownload), Ok(S::Queued));
        for s in S::ALL {
            if s != S::Completed {
                assert!(transition(s, E::Redownload).is_err(), "{s} redownloads");
            }
        }
        let mut reachable: HashSet<S> = HashSet::from([S::Queued]);
        let mut frontier = vec![S::Queued];
        while let Some(s) = frontier.pop() {
            for e in E::ALL {
                if let Ok(t) = transition(s, e)
                    && reachable.insert(t)
                {
                    frontier.push(t);
                }
            }
        }
        assert_eq!(reachable.len(), S::ALL.len(), "{reachable:?}");
        for s in S::ALL {
            if s != S::Completed {
                assert!(
                    E::ALL.iter().any(|e| transition(s, *e).is_ok()),
                    "{s} is a dead end"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn the_table_is_total_worker_scoped(
            s in prop::sample::select(S::ALL.to_vec()),
            e in prop::sample::select(E::ALL.to_vec()),
        ) {
            let r = transition(s, e);
            // Worker-only events require the worker to hold the job.
            if matches!(e, E::BodyComplete | E::RetryableError | E::FatalError | E::DiskFull | E::Shutdown) {
                prop_assert_eq!(r.is_ok(), s == S::Downloading);
            }
            if matches!(e, E::RenameOk | E::RenameFailed | E::RecoveredComplete | E::FinalizationLost) {
                prop_assert_eq!(r.is_ok(), s == S::Finalizing);
            }
            // A successful transition never lands on the same active state twice.
            if let Ok(t) = r {
                prop_assert!(!(t == s && t.is_active()));
            }
        }
    }
}
