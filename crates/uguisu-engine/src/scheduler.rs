//! The feed-refresh scheduler (ADR 0027).
//!
//! One task, started by `uguisu serve` and by nothing else: this is the only
//! place in Uguisu where work begins without somebody asking for it. The loop
//! returns on shutdown, reads the persisted pause, fills the free capacity
//! with podcasts the database says are due, and sleeps until the next one is,
//! woken early when a slot frees or a due time changes. An idle process wakes
//! at most every 15 minutes and asks three indexed questions.
//!
//! The daily housekeeping runs in the same task, on its own due time, so an
//! empty library does not stop it and a busy one does not delay it.
//!
//! Refreshes go through [`Engine::refresh_podcast`], so an automatic refresh
//! is the same code path, HTTP stack and SSRF policy as a manual one
//! (`docs/SECURITY.md` §3.1) — the scheduler opens no network path of its
//! own. The coalescer (ADR 0016) keeps a tick that collides with a
//! `podcast refresh` from fetching the feed twice.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uguisu_core::UguisuError;
use uguisu_core::ids::PodcastId;
use uguisu_core::model::{FetchState, PodcastStatus};
use uguisu_core::redact;
use uguisu_core::schedule::{self, SchedulerControl};
use uguisu_core::{Event, EventKind};
use uguisu_storage::{podcasts, scheduler as scheduler_repo};

use crate::refresh::RefreshOptions;
use crate::{Engine, lock};

/// Longest a pass may sleep. Not a refresh interval: an upper bound on
/// how stale the loop's own picture of the library may get, so a due time
/// written by another part of the process is acted on without needing a
/// wake-up call.
const MAX_TICK: Duration = Duration::from_secs(15 * 60);

/// Shortest a pass may sleep when something is already due.
const MIN_TICK: Duration = Duration::from_secs(1);

/// Shortest interval between two automatic refreshes of one podcast,
/// whatever the database says.
///
/// The refresh pipeline writes a back-off on every outcome, so this is
/// not the retry policy — it is the floor under a podcast whose refresh
/// fails *before* the pipeline can write anything (a lock error, a
/// cancelled task). Without it such a podcast stays due and the loop
/// spins on it.
const MIN_SPACING: Duration = Duration::from_secs(60);

/// What the scheduler is doing, for `scheduler status`, the API and the
/// status page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SchedulerStatus {
    /// Whether automatic refreshing is configured at all
    /// (`UGUISU_FEED_SCHEDULER`).
    pub enabled: bool,
    /// Whether this process is running the loop. `false` in a one-shot
    /// CLI command, which is not a fault: nothing schedules but `serve`.
    pub running: bool,
    /// Whether the persisted pause is set.
    pub paused: bool,
    /// Why it is paused.
    pub paused_reason: Option<String>,
    /// Since when.
    #[serde(with = "time::serde::rfc3339::option")]
    pub paused_at: Option<OffsetDateTime>,
    /// Refreshes the scheduler has running right now.
    pub inflight: u32,
    /// How many it may run at once.
    pub concurrency: u32,
    /// The default interval between refreshes of one podcast.
    pub interval_secs: u64,
    /// Podcasts due for a refresh at this moment.
    pub due_now: u32,
    /// When the next planned refresh falls due.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_due_at: Option<OffsetDateTime>,
    /// When housekeeping last ran.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_maintenance_at: Option<OffsetDateTime>,
}

/// What one pass did, published as `scheduler.tick` and returned to tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pass {
    /// Podcasts the pass found due within its capacity.
    pub due: u32,
    /// Refreshes it started.
    pub started: u32,
    /// Whether the persisted pause stopped it.
    pub paused: bool,
}

/// What the housekeeping pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MaintenanceReport {
    /// Event rows deleted.
    pub events_pruned: u64,
    /// Expired discovery cache rows deleted.
    pub cache_expired: u64,
    /// Sessions that can no longer authenticate anything, deleted.
    pub sessions_pruned: u64,
}

/// The scheduler's share of the engine's state.
#[derive(Default)]
pub(crate) struct SchedulerState {
    /// Wakes the loop early: a slot freed, a pause lifted, a due time
    /// changed.
    pub(crate) wake: Notify,
    /// Podcasts this loop is refreshing right now. Its own set rather
    /// than the coordinator's count, so the scheduler bounds *its* work
    /// and a person's `refresh --all` is still their own business.
    inflight: Mutex<HashSet<PodcastId>>,
    /// Podcasts that may not be started again before this instant.
    floor: Mutex<HashMap<PodcastId, OffsetDateTime>>,
    /// When housekeeping is next due. Held in memory and re-derived from
    /// `scheduler_control` on the first pass, which is what keeps a
    /// restart from running it on every boot.
    next_maintenance: Mutex<Option<OffsetDateTime>>,
    /// The loop, once started.
    task: Mutex<Option<JoinHandle<()>>>,
    /// The refreshes a pass has spawned and nobody has joined yet.
    ///
    /// Held so that a shutdown can wait for a fetch to abort and its
    /// transaction to commit *before* the pools close, and so that a
    /// one-shot `scheduler run` can wait for the work it started rather
    /// than exiting out from under it.
    refreshes: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for SchedulerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulerState")
            .field("inflight", &lock(&self.inflight).len())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Starts the feed-refresh loop in this process (idempotent).
    ///
    /// Only `uguisu serve` calls this. A one-shot command that started it
    /// would refresh feeds nobody asked about and then exit in the middle
    /// of doing so.
    pub fn start_refresh_scheduler(&self) {
        let mut slot = lock(&self.inner.scheduler.task);
        if slot.is_some() {
            return;
        }
        let engine = self.clone();
        let cancel = self.inner.shutdown.clone();
        *slot = Some(tokio::spawn(async move {
            tracing::info!("feed refresh scheduler started");
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let sleep = engine.scheduler_tick().await;
                tokio::select! {
                    () = engine.inner.scheduler.wake.notified() => {}
                    () = tokio::time::sleep(sleep) => {}
                    () = cancel.cancelled() => break,
                }
            }
            tracing::info!("feed refresh scheduler stopped");
        }));
    }

    /// Whether the loop is running in this process.
    #[must_use]
    pub fn refresh_scheduler_running(&self) -> bool {
        lock(&self.inner.scheduler.task).is_some()
    }

    /// Wakes the loop, if it is running.
    pub fn wake_scheduler(&self) {
        self.inner.scheduler.wake.notify_one();
    }

    /// One pass, then how long to sleep.
    ///
    /// A storage failure is treated exactly like a pause: warn, start
    /// nothing, look again later. Returning instead would end automatic
    /// refreshing for the life of the process because one query failed
    /// once.
    async fn scheduler_tick(&self) -> Duration {
        let config = self.config();
        // Housekeeping runs whether or not feeds are being refreshed, and
        // whether or not the scheduler is paused: it deletes derived data
        // to keep the database bounded, which is not what somebody is
        // switching off when they switch off automatic refreshing.
        let pass = if config.feed.scheduler {
            match self
                .run_scheduler_pass(config.feed.refresh_concurrency)
                .await
            {
                Ok(pass) => pass,
                Err(e) => {
                    tracing::warn!(error = %e, "scheduler pass failed; retrying later");
                    Pass::default()
                }
            }
        } else {
            Pass::default()
        };
        let next_maintenance = self.maintenance_pass(&config).await;
        let sleep = self
            .next_wake(config.feed.scheduler, next_maintenance)
            .await;
        if pass.due > 0 || pass.started > 0 {
            // Transient, like download progress: a heartbeat for a
            // dashboard, not a fact worth keeping for a year.
            self.bus().publish(&[Event::now(
                None,
                None,
                EventKind::SchedulerTick {
                    due: u64::from(pass.due),
                    started: u64::from(pass.started),
                    sleep_ms: u64::try_from(sleep.as_millis()).unwrap_or(u64::MAX),
                },
            )]);
        }
        sleep
    }

    /// Starts what the free capacity allows. Public for tests, which need
    /// a pass without a running loop and without a timer.
    pub async fn run_scheduler_pass(&self, concurrency: usize) -> Result<Pass, UguisuError> {
        if self.paused_or_unreadable().await {
            return Ok(Pass {
                paused: true,
                ..Pass::default()
            });
        }
        let now = OffsetDateTime::now_utc();
        let capacity = concurrency
            .max(1)
            .saturating_sub(lock(&self.inner.scheduler.inflight).len());
        if capacity == 0 {
            return Ok(Pass::default());
        }
        let excluded = self.excluded(now);
        let due = {
            let mut reader = self.storage().reader().await?;
            podcasts::due_ids(
                &mut reader,
                now,
                &excluded,
                u32::try_from(capacity).unwrap_or(u32::MAX),
            )
            .await?
        };
        let mut pass = Pass {
            due: u32::try_from(due.len()).unwrap_or(u32::MAX),
            ..Pass::default()
        };
        for id in due {
            // Claimed here, in the loop, before anything is awaited: two
            // passes must not hand out the same slot, and the task that
            // clears it is spawned below.
            if !lock(&self.inner.scheduler.inflight).insert(id) {
                continue;
            }
            lock(&self.inner.scheduler.floor).insert(id, now + MIN_SPACING);
            pass.started += 1;
            let engine = self.clone();
            let cancel = self.inner.shutdown.clone();
            let handle = tokio::spawn(async move {
                let result = engine
                    .refresh_podcast(
                        id,
                        RefreshOptions {
                            force: false,
                            cancel: Some(cancel),
                        },
                    )
                    .await;
                match result {
                    Ok(report) => {
                        tracing::debug!(podcast = %id, outcome = report.outcome.as_str(), "scheduled refresh");
                    }
                    // One podcast's failure releases its slot and nothing
                    // else: the loop keeps its other feeds moving.
                    Err(e) => {
                        tracing::warn!(podcast = %id, error = %redact::urls(&e.to_string()), "scheduled refresh failed");
                    }
                }
                lock(&engine.inner.scheduler.inflight).remove(&id);
                engine.wake_scheduler();
            });
            {
                // Finished handles are dropped on the way in, so a daemon
                // that runs for a month does not accumulate one per
                // refresh it ever started.
                let mut held = lock(&self.inner.scheduler.refreshes);
                held.retain(|h| !h.is_finished());
                held.push(handle);
            }
        }
        Ok(pass)
    }

    /// Podcasts the due query must skip: the ones this loop is already
    /// refreshing, the ones any caller is refreshing, and the ones inside
    /// their spacing floor. Excluding them *in the query* is what makes a
    /// pass fill its capacity instead of spending it on work in flight.
    fn excluded(&self, now: OffsetDateTime) -> Vec<PodcastId> {
        let mut floor = lock(&self.inner.scheduler.floor);
        floor.retain(|_, until| *until > now);
        let mut out: HashSet<PodcastId> = floor.keys().copied().collect();
        out.extend(lock(&self.inner.scheduler.inflight).iter().copied());
        out.extend(self.coordinator().inflight_ids());
        let mut out: Vec<PodcastId> = out.into_iter().collect();
        out.sort_unstable();
        out
    }

    /// The persisted pause — and an unreadable one counts as paused, the
    /// same judgement the download queue makes.
    async fn paused_or_unreadable(&self) -> bool {
        match self.scheduler_control().await {
            Ok(control) => control.paused,
            Err(e) => {
                tracing::warn!(error = %e, "scheduler control unreadable; treating as paused");
                true
            }
        }
    }

    /// Runs housekeeping if it is due, and says when it is due next.
    ///
    /// Independent of the feed cadence in both directions: this is
    /// computed from when it last ran, not from when a podcast is next
    /// due, so neither can starve the other.
    async fn maintenance_pass(
        &self,
        config: &uguisu_core::config::Config,
    ) -> Option<OffsetDateTime> {
        let interval = config.maintenance.interval;
        let now = OffsetDateTime::now_utc();
        // Read out of the lock before anything is awaited: a guard must
        // never be held across an await point.
        let known = *lock(&self.inner.scheduler.next_maintenance);
        let due = if let Some(at) = known {
            at
        } else {
            // First pass in this process: ask the row when it last ran,
            // so a restart does not mean another run.
            let Ok(control) = self.scheduler_control().await else {
                tracing::warn!("scheduler control unreadable; deferring maintenance");
                return None;
            };
            let at = control
                .last_maintenance_at
                .map_or(now, |last| schedule::after(last, interval));
            *lock(&self.inner.scheduler.next_maintenance) = Some(at);
            at
        };
        if due > now {
            return Some(due);
        }
        match self.run_maintenance().await {
            Ok(report) => tracing::info!(
                events_pruned = report.events_pruned,
                cache_expired = report.cache_expired,
                "maintenance"
            ),
            // Housekeeping that fails is housekeeping postponed, not a
            // reason to stop refreshing feeds.
            Err(e) => tracing::warn!(error = %e, "maintenance failed; retrying next interval"),
        }
        let next = schedule::after(OffsetDateTime::now_utc(), interval);
        *lock(&self.inner.scheduler.next_maintenance) = Some(next);
        Some(next)
    }

    /// Prunes the event log and drops expired discovery cache rows.
    ///
    /// Nothing here touches media, an episode or a podcast: everything it
    /// deletes is derived data whose loss costs history or a round trip.
    /// Both limits treat `0` as "no limit", the way the archive policy's
    /// bounds do — never as "delete everything".
    pub async fn run_maintenance(&self) -> Result<MaintenanceReport, UguisuError> {
        let config = self.config();
        let now = OffsetDateTime::now_utc();
        let days = i64::from(config.maintenance.retain_events_days);
        let before = if days == 0 {
            OffsetDateTime::UNIX_EPOCH
        } else {
            now - time::Duration::days(days)
        };
        let max_rows = match config.maintenance.retain_events_max_rows {
            0 => u64::MAX,
            n => n,
        };
        let mut tx = self.storage().begin().await?;
        let events_pruned = uguisu_storage::events::prune(&mut tx, before, max_rows).await?;
        let cache_expired = uguisu_storage::discovery::expire_cache(&mut tx, now).await?;
        let sessions_pruned = uguisu_storage::auth::prune_sessions(&mut tx, now).await?;
        scheduler_repo::set_last_maintenance(&mut tx, now).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        *lock(&self.inner.scheduler.next_maintenance) =
            Some(schedule::after(now, config.maintenance.interval));
        Ok(MaintenanceReport {
            events_pruned,
            cache_expired,
            sessions_pruned,
        })
    }

    /// How long until something is worth looking at: the earliest of the
    /// next planned refresh, the next spacing floor to expire and the
    /// next housekeeping pass, clamped so an idle process still looks
    /// around now and then and a busy one does not spin.
    async fn next_wake(
        &self,
        consider_feeds: bool,
        next_maintenance: Option<OffsetDateTime>,
    ) -> Duration {
        let now = OffsetDateTime::now_utc();
        if !consider_feeds {
            return match next_maintenance {
                Some(at) if at > now => Duration::try_from(at - now)
                    .unwrap_or(MAX_TICK)
                    .clamp(MIN_TICK, MAX_TICK),
                _ => MAX_TICK,
            };
        }
        let next_feed_due = {
            let Ok(mut reader) = self.storage().reader().await else {
                return MAX_TICK;
            };
            podcasts::next_due_at(&mut reader).await.ok().flatten()
        };
        let next_floor = lock(&self.inner.scheduler.floor).values().copied().min();
        let next = [next_feed_due, next_floor, next_maintenance]
            .into_iter()
            .flatten()
            .min();
        match next {
            Some(at) if at > now => Duration::try_from(at - now)
                .unwrap_or(MAX_TICK)
                .clamp(MIN_TICK, MAX_TICK),
            // Due now, or nothing planned: a podcast that has never been
            // fetched has no due time and is still due, so the floor
            // below is what keeps this from being a busy loop.
            Some(_) => MIN_TICK,
            None => MAX_TICK,
        }
    }

    /// The persisted scheduler control row.
    pub async fn scheduler_control(&self) -> Result<SchedulerControl, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(scheduler_repo::control_get(&mut reader).await?)
    }

    /// Stops automatic refreshing until it is resumed, across restarts.
    ///
    /// Only the scheduler: a `podcast refresh` still refreshes, and
    /// transfers are a separate decision with a separate pause.
    pub async fn pause_scheduler(
        &self,
        reason: Option<&str>,
    ) -> Result<SchedulerControl, UguisuError> {
        self.set_scheduler_paused(true, reason).await
    }

    /// Resumes automatic refreshing.
    pub async fn resume_scheduler(&self) -> Result<SchedulerControl, UguisuError> {
        let control = self.set_scheduler_paused(false, None).await?;
        self.wake_scheduler();
        Ok(control)
    }

    async fn set_scheduler_paused(
        &self,
        paused: bool,
        reason: Option<&str>,
    ) -> Result<SchedulerControl, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        let control = scheduler_repo::control_set(&mut tx, paused, reason, now).await?;
        let events = vec![Event::now(
            None,
            None,
            if paused {
                EventKind::SchedulerPaused {
                    reason: reason.unwrap_or("user").to_owned(),
                }
            } else {
                EventKind::SchedulerResumed {}
            },
        )];
        uguisu_storage::events::insert_all(&mut tx, &events).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(&events);
        tracing::info!(paused, reason, "scheduler control changed");
        Ok(control)
    }

    /// Takes one podcast out of the schedule. Its files, episodes and
    /// history are untouched, and a manual refresh still works — this is
    /// only about what happens unasked.
    pub async fn pause_podcast(&self, id: PodcastId) -> Result<PodcastStatus, UguisuError> {
        self.set_podcast_paused(id, true).await
    }

    /// Puts a paused or archived podcast back in the schedule, due
    /// somewhere in the next interval rather than immediately: resuming two
    /// hundred podcasts must not produce two hundred simultaneous fetches.
    pub async fn resume_podcast(&self, id: PodcastId) -> Result<PodcastStatus, UguisuError> {
        let status = self.set_podcast_paused(id, false).await?;
        self.wake_scheduler();
        Ok(status)
    }

    /// Stops refreshing a podcast for good, keeping everything it has
    /// (ADR 0055): `archived`, with its current source `disabled`. `resume`
    /// undoes it.
    pub async fn archive_podcast(&self, id: PodcastId) -> Result<PodcastStatus, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        let Some(podcast) = podcasts::get(&mut tx, id).await? else {
            return Err(UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: id.to_string(),
            });
        };
        if podcast.status != PodcastStatus::Archived {
            podcasts::set_status(&mut tx, id, PodcastStatus::Archived, now).await?;
            if let Some(source) = uguisu_storage::sources::current(&mut tx, id).await? {
                uguisu_storage::sources::set_state(&mut tx, source.id, FetchState::Disabled, now)
                    .await?;
            }
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            tracing::info!(podcast = %id, status = "archived", "podcast schedule changed");
        }
        Ok(PodcastStatus::Archived)
    }

    async fn set_podcast_paused(
        &self,
        id: PodcastId,
        paused: bool,
    ) -> Result<PodcastStatus, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let interval = self.config().feed.refresh_interval;
        let target = if paused {
            PodcastStatus::Paused
        } else {
            PodcastStatus::Active
        };
        let mut tx = self.storage().begin().await?;
        let Some(podcast) = podcasts::get(&mut tx, id).await? else {
            return Err(UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: id.to_string(),
            });
        };
        if podcast.status == target {
            return Ok(target);
        }
        if !paused
            && !matches!(
                podcast.status,
                PodcastStatus::Paused | PodcastStatus::Archived
            )
        {
            return Err(UguisuError::Conflict(format!(
                "podcast is {}, not paused or archived",
                podcast.status.as_str()
            )));
        }
        if paused && podcast.status == PodcastStatus::Archived {
            return Err(UguisuError::Conflict(
                "podcast is archived; resume it before pausing it".to_owned(),
            ));
        }
        podcasts::set_status(&mut tx, id, target, now).await?;
        if podcast.status == PodcastStatus::Archived
            && let Some(source) = uguisu_storage::sources::current(&mut tx, id).await?
        {
            uguisu_storage::sources::set_state(&mut tx, source.id, FetchState::NeverFetched, now)
                .await?;
        }
        if !paused {
            let at = schedule::resume_at(id, now, interval);
            podcasts::set_next_refresh_at(&mut tx, id, Some(at), now).await?;
        }
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        tracing::info!(podcast = %id, status = target.as_str(), "podcast schedule changed");
        Ok(target)
    }

    /// Sets when a podcast is next due. `None` means "as soon as the
    /// scheduler looks", which is what a person asking for a refresh
    /// *eventually* wants; `podcast refresh` is what they want when they
    /// mean now.
    pub async fn reschedule_podcast(
        &self,
        id: PodcastId,
        at: Option<OffsetDateTime>,
    ) -> Result<(), UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        if !podcasts::set_next_refresh_at(&mut tx, id, at, now).await? {
            return Err(UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: id.to_string(),
            });
        }
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.wake_scheduler();
        Ok(())
    }

    /// What the scheduler is doing.
    pub async fn scheduler_status(&self) -> Result<SchedulerStatus, UguisuError> {
        let config = self.config();
        let control = self.scheduler_control().await?;
        let now = OffsetDateTime::now_utc();
        let (due_now, next_due_at) = {
            let mut reader = self.storage().reader().await?;
            let due = podcasts::due_ids(&mut reader, now, &[], u32::MAX).await?;
            let next = podcasts::next_due_at(&mut reader).await?;
            (u32::try_from(due.len()).unwrap_or(u32::MAX), next)
        };
        Ok(SchedulerStatus {
            enabled: config.feed.scheduler,
            running: self.refresh_scheduler_running(),
            paused: control.paused,
            paused_reason: control.paused_reason,
            paused_at: control.paused_at,
            inflight: u32::try_from(lock(&self.inner.scheduler.inflight).len()).unwrap_or(u32::MAX),
            concurrency: u32::try_from(config.feed.refresh_concurrency).unwrap_or(u32::MAX),
            interval_secs: config.feed.refresh_interval.as_secs(),
            due_now,
            next_due_at,
            last_maintenance_at: control.last_maintenance_at,
        })
    }

    /// Stops the loop and waits for it and for the refreshes it started,
    /// bounded by `grace`.
    pub(crate) async fn stop_refresh_scheduler(&self, grace: Duration) {
        let task = lock(&self.inner.scheduler.task).take();
        if let Some(task) = task
            && tokio::time::timeout(grace, task).await.is_err()
        {
            tracing::warn!("refresh scheduler did not stop in time");
        }
        // The loop starting nothing new is not the same as nothing
        // running: a refresh spawned a moment ago is holding a
        // transaction, and closing the pools under it turns a clean
        // cancellation into a storage error.
        self.wait_for_refreshes(grace).await;
    }

    /// Waits for the refreshes the scheduler has started, bounded by
    /// `grace`.
    ///
    /// `uguisu serve` reaches this through [`Engine::close`]. A one-shot
    /// `scheduler run` calls it directly: `run_scheduler_pass` returns as
    /// soon as the work is *started*, and a process that exits there
    /// cancels every fetch it just asked for.
    pub async fn wait_for_refreshes(&self, grace: Duration) {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            let Some(handle) = lock(&self.inner.scheduler.refreshes).pop() else {
                return;
            };
            match tokio::time::timeout_at(deadline, handle).await {
                Ok(Ok(())) => {}
                // A refresh that panicked has already been logged by the
                // runtime; the point here is that it is no longer holding
                // a transaction, so the rest may be waited for.
                Ok(Err(e)) => tracing::warn!(error = %e, "a scheduled refresh ended badly"),
                Err(_) => {
                    tracing::warn!("a scheduled refresh did not finish in time");
                    return;
                }
            }
        }
    }
}
