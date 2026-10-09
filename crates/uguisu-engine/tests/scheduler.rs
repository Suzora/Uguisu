//! The feed-refresh scheduler (ADR 0027): what a pass starts, what it
//! refuses to start, and what a pause means.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use time::{Duration, OffsetDateTime};
use uguisu_core::model::PodcastStatus;
use uguisu_http::CancellationToken;
use uguisu_storage::podcasts;

/// Makes a podcast due (or not) without going through a refresh.
async fn set_due(h: &Harness, id: uguisu_core::PodcastId, at: Option<OffsetDateTime>) {
    let mut tx = h.engine.storage().begin().await.unwrap();
    podcasts::set_next_refresh_at(&mut tx, id, at, OffsetDateTime::now_utc())
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn podcast_status(h: &Harness, id: uguisu_core::PodcastId) -> PodcastStatus {
    let mut reader = h.engine.storage().reader().await.unwrap();
    podcasts::get(&mut reader, id)
        .await
        .unwrap()
        .unwrap()
        .status
}

async fn next_due(h: &Harness, id: uguisu_core::PodcastId) -> Option<OffsetDateTime> {
    let mut reader = h.engine.storage().reader().await.unwrap();
    podcasts::get(&mut reader, id)
        .await
        .unwrap()
        .unwrap()
        .next_refresh_at
}

#[tokio::test]
async fn a_pass_refreshes_and_replans() {
    let h = Harness::new().await;
    let added = h.add_fixture("minimal_rss.xml").await;
    let id = added.podcast.id;
    // `podcast add` refreshed it, so it is not due again yet.
    let planned = next_due(&h, id).await.unwrap();
    assert!(planned > OffsetDateTime::now_utc());
    assert_eq!(h.engine.run_scheduler_pass(8).await.unwrap().started, 0);

    set_due(
        &h,
        id,
        Some(OffsetDateTime::now_utc() - Duration::minutes(1)),
    )
    .await;
    let pass = h.engine.run_scheduler_pass(8).await.unwrap();
    assert_eq!((pass.due, pass.started, pass.paused), (1, 1, false));
    // The spawned refresh finishes and plans the next one for itself.
    let replanned = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(at) = next_due(&h, id).await
                && at > OffsetDateTime::now_utc()
            {
                return at;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the scheduled refresh never replanned the podcast");
    let interval = h.engine.config().feed.refresh_interval;
    assert!(replanned >= OffsetDateTime::now_utc() + Duration::seconds(1));
    assert!(
        replanned
            <= OffsetDateTime::now_utc() + Duration::seconds(2 * interval.as_secs().cast_signed()),
        "the spread must stay inside two intervals"
    );
}

#[tokio::test]
async fn a_pass_fills_capacity_without_repeats() {
    let h = Harness::new().await;
    let mut ids = Vec::new();
    for n in 0..4 {
        h.serve_fixture(&format!("/feed{n}.xml"), "minimal_rss.xml")
            .await;
        let added = h
            .engine
            .add_podcast(
                &h.url(&format!("/feed{n}.xml")),
                CancellationToken::default(),
            )
            .await
            .unwrap();
        ids.push(added.podcast.id);
        set_due(&h, added.podcast.id, None).await;
    }
    // Capacity of two means two refreshes, not four.
    let pass = h.engine.run_scheduler_pass(2).await.unwrap();
    assert_eq!(pass.started, 2);
    // A second pass while those two are in flight must not pick them up
    // again — and may start the rest if slots are free.
    let second = h.engine.run_scheduler_pass(2).await.unwrap();
    assert_eq!(
        second.started, 0,
        "the capacity is spent; a pass must not oversubscribe it"
    );
    let bigger = h.engine.run_scheduler_pass(4).await.unwrap();
    assert!(
        bigger.started <= 2,
        "only the podcasts that are not already in flight"
    );
}

#[tokio::test]
async fn a_paused_scheduler_starts_nothing() {
    let h = Harness::new().await;
    let added = h.add_fixture("minimal_rss.xml").await;
    set_due(&h, added.podcast.id, None).await;

    let control = h.engine.pause_scheduler(Some("maintenance")).await.unwrap();
    assert!(control.paused);
    let pass = h.engine.run_scheduler_pass(8).await.unwrap();
    assert!(pass.paused);
    assert_eq!(pass.started, 0);

    let status = h.engine.scheduler_status().await.unwrap();
    assert!(status.paused && status.enabled && !status.running);
    assert_eq!(status.paused_reason.as_deref(), Some("maintenance"));
    assert_eq!(status.due_now, 1, "the work is waiting, not lost");

    // A manual refresh is still a manual refresh — and it plans the next
    // one, so the podcast stops being due.
    h.refresh(added.podcast.id, true).await;
    assert!(next_due(&h, added.podcast.id).await.unwrap() > OffsetDateTime::now_utc());
    set_due(&h, added.podcast.id, None).await;

    assert!(!h.engine.resume_scheduler().await.unwrap().paused);
    assert_eq!(h.engine.run_scheduler_pass(8).await.unwrap().started, 1);
}

#[tokio::test]
async fn a_paused_podcast_returns_spread_out() {
    let h = Harness::new().await;
    let added = h.add_fixture("minimal_rss.xml").await;
    let id = added.podcast.id;
    set_due(&h, id, None).await;

    assert_eq!(
        h.engine.pause_podcast(id).await.unwrap(),
        PodcastStatus::Paused
    );
    assert_eq!(podcast_status(&h, id).await, PodcastStatus::Paused);
    let pass = h.engine.run_scheduler_pass(8).await.unwrap();
    assert_eq!((pass.due, pass.started), (0, 0), "paused is not due");

    // Still refreshable by hand, and still paused afterwards.
    h.refresh(id, true).await;
    assert_eq!(podcast_status(&h, id).await, PodcastStatus::Paused);

    let before = OffsetDateTime::now_utc();
    assert_eq!(
        h.engine.resume_podcast(id).await.unwrap(),
        PodcastStatus::Active
    );
    let planned = next_due(&h, id).await.unwrap();
    let interval = h
        .engine
        .config()
        .feed
        .refresh_interval
        .as_secs()
        .cast_signed();
    assert!(planned >= before, "{planned} is in the past");
    assert!(
        planned <= before + Duration::seconds(interval),
        "a resume must land inside the interval, not after it"
    );

    // Resuming what is already running is a no-op, not an error.
    assert_eq!(
        h.engine.resume_podcast(id).await.unwrap(),
        PodcastStatus::Active
    );
    assert_eq!(
        next_due(&h, id).await.unwrap(),
        planned,
        "and changes nothing"
    );

    // Resuming is also how an archived podcast comes back (ADR 0055).
    let mut tx = h.engine.storage().begin().await.unwrap();
    podcasts::set_status(
        &mut tx,
        id,
        PodcastStatus::Archived,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        h.engine.resume_podcast(id).await.unwrap(),
        PodcastStatus::Active
    );
}

#[tokio::test]
async fn the_loop_starts_once_obeys_switch() {
    let mut h = Harness::new().await;
    assert!(!h.engine.refresh_scheduler_running());
    h.engine.start_refresh_scheduler();
    h.engine.start_refresh_scheduler();
    assert!(h.engine.refresh_scheduler_running());
    let added = h.add_fixture("minimal_rss.xml").await;
    set_due(&h, added.podcast.id, None).await;
    h.engine.wake_scheduler();
    // The loop picks it up on its own, without anybody calling a pass.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if next_due(&h, added.podcast.id)
                .await
                .is_some_and(|at| at > OffsetDateTime::now_utc())
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the running scheduler never refreshed a due podcast");

    // A restart with the scheduler switched off starts the loop but the
    // loop does nothing: the key is live, so this is a setting, not a
    // build-time decision.
    h.restart(None).await;
    h.engine
        .set_setting("UGUISU_FEED_SCHEDULER", "false", Some("test"))
        .await
        .unwrap();
    set_due(&h, added.podcast.id, None).await;
    h.engine.start_refresh_scheduler();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        next_due(&h, added.podcast.id).await.is_none(),
        "a switched-off scheduler must refresh nothing"
    );
    h.engine.close().await;
    assert!(
        !h.engine.refresh_scheduler_running(),
        "close joins the loop"
    );
}

/// Rows in the event log.
async fn count_events(h: &Harness) -> usize {
    let mut reader = h.engine.storage().reader().await.unwrap();
    uguisu_storage::events::list_after(&mut reader, None, 10_000)
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn housekeeping_prunes_and_remembers() {
    let mut h = Harness::new().await;
    h.add_fixture("minimal_rss.xml").await;
    assert!(count_events(&h).await > 0);

    // Keep nothing older than today and at most one row: the default
    // configuration keeps a month and a hundred thousand, so this is what
    // a busy installation looks like from the inside.
    h.engine
        .set_setting("UGUISU_EVENTS_RETAIN_MAX_ROWS", "1", Some("test"))
        .await
        .unwrap();
    let report = h.engine.run_maintenance().await.unwrap();
    assert!(report.events_pruned > 0);
    assert_eq!(count_events(&h).await, 1);

    // It recorded when it ran, so a restart does not run it again.
    let control = h.engine.scheduler_control().await.unwrap();
    let ran_at = control.last_maintenance_at.expect("not recorded");
    h.restart(None).await;
    assert_eq!(
        h.engine
            .scheduler_control()
            .await
            .unwrap()
            .last_maintenance_at,
        Some(ran_at)
    );
}

#[tokio::test]
async fn zero_retention_means_no_limit() {
    let h = Harness::new().await;
    h.add_fixture("minimal_rss.xml").await;
    let before = count_events(&h).await;
    assert!(before > 0);
    for (key, value) in [
        ("UGUISU_EVENTS_RETAIN_DAYS", "0"),
        ("UGUISU_EVENTS_RETAIN_MAX_ROWS", "0"),
    ] {
        h.engine
            .set_setting(key, value, Some("test"))
            .await
            .unwrap();
    }
    let report = h.engine.run_maintenance().await.unwrap();
    assert_eq!(report.events_pruned, 0);
    let after = count_events(&h).await;
    assert!(
        after >= before,
        "an operator who asked for no limit must not lose the log"
    );
}

/// A one-shot `scheduler run` must not exit out from under the refreshes
/// it just started.
///
/// `run_scheduler_pass` returns when the work is *started*; the process
/// that called it then closes the engine, which cancels the shutdown token
/// and closes the pools. Against a live database that turned a clean
/// cancellation into `attempted to acquire a connection on a closed pool`
/// and left `next_refresh_at` untouched, so the command reported a refresh
/// it had in fact undone. `wait_for_refreshes` is what the CLI waits on.
#[tokio::test]
async fn waiting_leaves_refreshes_committed() {
    let h = Harness::new().await;
    let added = h.add_fixture("minimal_rss.xml").await;
    let id = added.podcast.id;
    let before = next_due(&h, id).await.unwrap();
    set_due(
        &h,
        id,
        Some(OffsetDateTime::now_utc() - Duration::minutes(1)),
    )
    .await;

    let pass = h.engine.run_scheduler_pass(8).await.unwrap();
    assert_eq!(pass.started, 1);
    h.engine
        .wait_for_refreshes(std::time::Duration::from_secs(20))
        .await;

    // No polling: after the wait there is nothing left to wait for.
    let after = next_due(&h, id).await.expect("the refresh replanned it");
    assert!(
        after > OffsetDateTime::now_utc(),
        "the refresh committed a future due time, not the past one it was given: {after}"
    );
    assert_ne!(after, before);
    assert_eq!(
        h.engine.scheduler_status().await.unwrap().inflight,
        0,
        "the slot is released by the time the wait returns"
    );
    // And waiting again is free rather than a second pass.
    h.engine
        .wait_for_refreshes(std::time::Duration::from_secs(1))
        .await;
}
