//! Crash boundaries across real restarts: a worker dies at every fail
//! point, the database pools are closed and the file reopened (what a new
//! process sees), and the next service reconciles and finishes the job
//! with the right bytes. Also: injected write errors and idempotency of
//! enqueue and reconcile across restarts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::time::Duration;

use uguisu_core::download::{DownloadErrorKind, DownloadState, PauseAllReason, Priority};
use uguisu_core::events::EventKind;
use uguisu_core::ids::{EpisodeId, JobId};
use uguisu_core::model::ArchiveState;
use uguisu_download::deps::{FailInjector, FailPoint};
use uguisu_download::testing::content_sha256;
use uguisu_download::{DownloadService, JobFilter};

mod common;
use common::{Harness, config, file_sha256, harness, run_to_idle};

const SIZE: u64 = 1_000_000;
/// Paced (2 MB/s, so half a second) and large enough that several progress
/// records are written before a fail point fires. `/slow` supports ranges
/// and strong validators exactly like `/range`.
const ROUTE: &str = "/slow/1000000/2000000";

/// Runs a service that dies at `point` while downloading `ep`; returns the job.
async fn die_at(h: &Harness, point: FailPoint, ep: EpisodeId) -> JobId {
    let injector = FailInjector::armed(point);
    let dying = DownloadService::new(h.deps(Some(injector.clone())));
    let job_id = dying
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    dying.start();
    tokio::time::timeout(Duration::from_secs(20), dying.wait_idle())
        .await
        .unwrap()
        .unwrap();
    assert!(injector.was_hit(), "{point:?}");
    dying.shutdown(Duration::from_millis(100)).await;
    job_id
}

/// Stored `download.completed` events for a job.
async fn completed_events(h: &Harness, job_id: JobId) -> usize {
    let mut r = h.storage.reader().await.unwrap();
    uguisu_storage::events::list_after(&mut r, None, 1000)
        .await
        .unwrap()
        .iter()
        .filter(
            |e| matches!(&e.kind, EventKind::DownloadCompleted { job_id: j, .. } if *j == job_id),
        )
        .count()
}

#[tokio::test]
async fn a_restart_repairs_every_crash_boundary() {
    let points = [
        FailPoint::AfterBytes(500_000),
        FailPoint::BeforeFinalizingWrite,
        FailPoint::AfterFinalizingWrite,
        FailPoint::AfterRename,
    ];
    for point in points {
        let mut h = harness(config(1, 1), 1).await;
        let ep = h.episode(ROUTE).await;
        let job_id = die_at(&h, point, ep).await;
        let probe = h.service();
        let dead = h.job(&probe, job_id).await;
        assert!(dead.state.is_active(), "{point:?}: {}", dead.state);
        let file_before = h.part_len(&dead);
        let db_before = dead.bytes_downloaded;
        if let Some(len) = file_before {
            assert!(
                db_before <= len,
                "{point:?}: the database never claims more than the file holds"
            );
        }
        drop(probe);

        // The "next process": fresh pools on the same file.
        h.reopen().await;
        let svc = h.service();
        let report = svc.reconcile(false).await.unwrap();
        assert_eq!(
            report.recovered + report.finalized,
            1,
            "{point:?}: exactly one job repaired: {report:?}"
        );
        run_to_idle(&svc).await;
        let job = h.job(&svc, job_id).await;
        assert_eq!(job.state, DownloadState::Completed, "{point:?}");
        assert_eq!(job.bytes_downloaded, SIZE);
        assert_eq!(job.total_bytes, Some(SIZE));
        assert_eq!(
            file_sha256(&h.target(&job.target_path)),
            content_sha256(SIZE),
            "{point:?}"
        );
        assert!(
            !h.target(&job.part_path).exists(),
            "{point:?}: no .part left"
        );
        assert_eq!(h.archive_state(ep).await, ArchiveState::Archived);
        let detail = svc.job(job_id).await.unwrap();
        assert!(
            detail.attempts.iter().all(|a| a.finished_at.is_some()),
            "{point:?}: every attempt is closed"
        );
        if point == FailPoint::AfterBytes(500_000) {
            // The retry resumed from a validated offset, never past what the
            // database had acknowledged, and asked the server for a range.
            assert_eq!(detail.attempts.len(), 2);
            let resumed = detail.attempts[1].range_start;
            assert!(
                resumed > 0 && resumed <= db_before,
                "{resumed} vs {db_before}"
            );
            let reqs = h.servers[0].requests_for(ROUTE);
            assert_eq!(reqs.len(), 2);
            assert_eq!(reqs[1].range_start(), Some(resumed));
        }
        assert_eq!(completed_events(&h, job_id).await, 1, "{point:?}");

        // Reconciling again changes nothing.
        let again = svc.reconcile(true).await.unwrap();
        assert_eq!(
            (
                again.recovered,
                again.finalized,
                again.finalization_lost,
                again.missing_targets
            ),
            (0, 0, 0, 0),
            "{point:?}: {again:?}"
        );
        assert_eq!(
            h.job(&svc, job_id).await,
            job,
            "{point:?}: reconcile is a no-op"
        );
        svc.shutdown(Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn a_wrong_target_is_not_adopted() {
    // After the hash is written something else is put at the target; after
    // the rename the file is swapped for a shorter one. Neither is the download.
    for point in [FailPoint::AfterFinalizingWrite, FailPoint::AfterRename] {
        let mut h = harness(config(1, 1), 1).await;
        let ep = h.episode(ROUTE).await;
        let job_id = die_at(&h, point, ep).await;
        let probe = h.service();
        let dead = h.job(&probe, job_id).await;
        drop(probe);
        assert_eq!(dead.state, DownloadState::Finalizing, "{point:?}");
        let target = h.target(&dead.target_path);
        let part = h.target(&dead.part_path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let foreign = b"not the episode".to_vec();
        std::fs::write(&target, &foreign).unwrap();

        h.reopen().await;
        let svc = h.service();
        let report = svc.reconcile(false).await.unwrap();
        assert_eq!(
            (report.finalized, report.finalization_conflicts),
            (0, 1),
            "{point:?}: {report:?}"
        );
        let job = h.job(&svc, job_id).await;
        assert_eq!(job.state, DownloadState::Failed, "{point:?}");
        assert_eq!(job.state_reason.as_deref(), Some("target_exists"));
        assert_eq!(std::fs::read(&target).unwrap(), foreign, "{point:?}");
        assert_eq!(
            part.exists(),
            point == FailPoint::AfterFinalizingWrite,
            "{point:?}: a .part that was there stays"
        );
        assert_eq!(h.archive_state(ep).await, ArchiveState::Failed);
        assert_eq!(completed_events(&h, job_id).await, 0, "{point:?}");
        svc.shutdown(Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn two_deaths_end_in_one_file() {
    let mut h = harness(config(1, 1), 1).await;
    let ep = h.episode(ROUTE).await;
    let job_id = die_at(&h, FailPoint::AfterBytes(300_000), ep).await;
    h.reopen().await;
    let mid = h.service();
    let r = mid.reconcile(false).await.unwrap();
    assert_eq!(r.recovered, 1);
    drop(mid);
    let again = die_at(&h, FailPoint::AfterBytes(600_000), ep).await;
    assert_eq!(again, job_id, "one job per episode, whatever happens");
    h.reopen().await;
    let svc = h.service();
    assert_eq!(svc.reconcile(false).await.unwrap().recovered, 1);
    run_to_idle(&svc).await;
    let detail = svc.job(job_id).await.unwrap();
    assert_eq!(detail.job.state, DownloadState::Completed);
    assert_eq!(detail.attempts.len(), 3);
    assert!(detail.attempts[1].range_start > 0);
    assert!(detail.attempts[2].range_start > detail.attempts[1].range_start);
    assert_eq!(
        file_sha256(&h.target(&detail.job.target_path)),
        content_sha256(SIZE)
    );
    let reqs = h.servers[0].requests_for(ROUTE);
    assert_eq!(reqs.len(), 3);
    assert!(reqs[1].range_start().is_some() && reqs[2].range_start().is_some());
    assert_eq!(completed_events(&h, job_id).await, 1);
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn a_disk_full_pause_survives_restart() {
    let mut h = harness(config(1, 1), 1).await;
    let injector = FailInjector::armed(FailPoint::WriteError(
        100_000,
        std::io::ErrorKind::StorageFull,
    ));
    let svc = DownloadService::new(h.deps(Some(injector.clone())));
    let big = h.episode(ROUTE).await;
    let small = h.episode("/range/5000").await;
    let big_job = svc
        .enqueue_episode(big, Priority::High)
        .await
        .unwrap()
        .job()
        .id;
    let small_job = svc
        .enqueue_episode(small, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    svc.start();
    tokio::time::timeout(Duration::from_secs(20), svc.wait_idle())
        .await
        .unwrap()
        .unwrap();
    assert!(injector.was_hit());
    let job = h.job(&svc, big_job).await;
    assert_eq!(
        (job.state, job.state_reason.as_deref()),
        (DownloadState::Paused, Some("disk_full"))
    );
    assert_eq!(job.last_error_kind, Some(DownloadErrorKind::DiskFull));
    assert_eq!(job.attempt_count, 0, "a full disk refunds the attempt");
    assert!(h.part_len(&job).is_some(), "the partial file is kept");
    let stats = svc.stats().await.unwrap();
    assert_eq!(stats.paused_all, Some(PauseAllReason::DiskFull));
    assert_ne!(h.job(&svc, small_job).await.state, DownloadState::Completed);
    assert_eq!(
        h.servers[0].hits("/range/5000"),
        0,
        "nothing else was claimed"
    );
    svc.shutdown(Duration::from_secs(1)).await;

    // The pause survives the restart; `resume-all` lifts it and the big
    // download resumes with a range request.
    h.reopen().await;
    let svc = h.service();
    let report = svc.reconcile(false).await.unwrap();
    assert_eq!(report.recovered, 0);
    assert_eq!(
        svc.stats().await.unwrap().paused_all,
        Some(PauseAllReason::DiskFull)
    );
    svc.start();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        h.servers[0].hits(ROUTE),
        1,
        "paused: no request while paused"
    );
    svc.resume_all().await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), svc.wait_idle())
        .await
        .unwrap()
        .unwrap();
    for id in [big_job, small_job] {
        assert_eq!(h.job(&svc, id).await.state, DownloadState::Completed);
    }
    let detail = svc.job(big_job).await.unwrap();
    assert_eq!(
        file_sha256(&h.target(&detail.job.target_path)),
        content_sha256(SIZE)
    );
    let reqs = h.servers[0].requests_for(ROUTE);
    assert_eq!(reqs.len(), 2);
    let resumed = reqs[1].range_start().unwrap();
    assert!(resumed > 0 && resumed < SIZE, "{resumed}");
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn a_permission_error_needs_retry() {
    let mut h = harness(config(1, 1), 1).await;
    let injector = FailInjector::armed(FailPoint::WriteError(
        50_000,
        std::io::ErrorKind::PermissionDenied,
    ));
    let svc = DownloadService::new(h.deps(Some(injector.clone())));
    let ep = h.episode(ROUTE).await;
    let job_id = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    run_to_idle(&svc).await;
    let job = h.job(&svc, job_id).await;
    assert_eq!(
        (job.state, job.state_reason.as_deref()),
        (DownloadState::Failed, Some("not_retryable"))
    );
    assert_eq!(
        job.last_error_kind,
        Some(DownloadErrorKind::PermissionDenied)
    );
    assert_eq!(job.attempt_count, 1);
    assert_eq!(h.archive_state(ep).await, ArchiveState::Failed);
    assert_eq!(
        svc.stats().await.unwrap().paused_all,
        None,
        "not a disk-full pause"
    );
    svc.shutdown(Duration::from_secs(1)).await;

    h.reopen().await;
    let svc = h.service();
    let job = svc.retry(job_id).await.unwrap();
    assert_eq!(job.state, DownloadState::Queued);
    run_to_idle(&svc).await;
    let job = h.job(&svc, job_id).await;
    assert_eq!(job.state, DownloadState::Completed);
    assert_eq!(
        file_sha256(&h.target(&job.target_path)),
        content_sha256(SIZE)
    );
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn enqueue_and_reconcile_are_idempotent_across_restarts() {
    let mut h = harness(config(1, 1), 1).await;
    let ep = h.episode("/range/2000").await;
    let mut ids = Vec::new();
    for round in 0..3 {
        let svc = h.service();
        for _ in 0..3 {
            let outcome = svc.enqueue_episode(ep, Priority::Normal).await.unwrap();
            ids.push(outcome.job().id);
        }
        let report = svc.reconcile(true).await.unwrap();
        assert_eq!(
            (
                report.recovered,
                report.finalized,
                report.finalization_lost,
                report.missing_targets
            ),
            (0, 0, 0, 0),
            "round {round}: {report:?}"
        );
        let page = svc
            .list(&JobFilter {
                state: None,
                podcast_id: None,
                after: None,
                limit: 100,
            })
            .await
            .unwrap();
        assert_eq!(page.jobs.len(), 1, "round {round}");
        h.reopen().await;
    }
    assert!(
        ids.iter().all(|i| *i == ids[0]),
        "one job id, ever: {ids:?}"
    );
    let svc = h.service();
    run_to_idle(&svc).await;
    let before = h.job(&svc, ids[0]).await;
    assert_eq!(before.state, DownloadState::Completed);
    // Completed: enqueue is a no-op, reconcile too.
    let outcome = svc.enqueue_episode(ep, Priority::High).await.unwrap();
    assert!(matches!(
        outcome,
        uguisu_download::EnqueueOutcome::AlreadyCompleted(_)
    ));
    svc.reconcile(true).await.unwrap();
    assert_eq!(h.job(&svc, ids[0]).await, before);
    assert_eq!(h.servers[0].hits("/range/2000"), 1);
    svc.shutdown(Duration::from_secs(1)).await;
}
