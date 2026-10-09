//! The queue as a whole: idempotent enqueue, ordering and per-host limits,
//! global pause, retry persistence, shutdown/restart, reconciliation and
//! a hundred mixed downloads.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::many_single_char_names,
    clippy::too_many_lines
)]

use std::time::Duration;

use uguisu_core::UguisuError;
use uguisu_core::download::{DownloadState, PauseAllReason, Priority};
use uguisu_core::ids::{EpisodeId, JobId, PodcastId};
use uguisu_core::model::ArchiveState;
use uguisu_download::deps::{FailInjector, FailPoint};
use uguisu_download::testing::content_sha256;
use uguisu_download::{DownloadService, EnqueueOutcome, JobFilter};

use time::OffsetDateTime;

mod common;
use common::{config, file_sha256, harness, run_to_idle, wait_for};

#[tokio::test]
async fn enqueue_is_idempotent_per_episode() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    let ep = h.episode("/range/50000").await;
    let a = svc.enqueue_episode(ep, Priority::Normal).await.unwrap();
    let EnqueueOutcome::Created(job) = &a else {
        panic!("{a:?}");
    };
    assert_eq!(job.state, DownloadState::Queued);
    assert_eq!(h.archive_state(ep).await, ArchiveState::Queued);
    for _ in 0..3 {
        let again = svc.enqueue_episode(ep, Priority::High).await.unwrap();
        assert!(
            matches!(again, EnqueueOutcome::Existing(ref j) if j.id == job.id),
            "{again:?}"
        );
    }
    run_to_idle(&svc).await;
    let done = svc.enqueue_episode(ep, Priority::Normal).await.unwrap();
    assert!(
        matches!(done, EnqueueOutcome::AlreadyCompleted(_)),
        "{done:?}"
    );
    assert_eq!(h.servers[0].requests().len(), 1, "no second download");
    let page = svc
        .list(&JobFilter {
            limit: 10,
            ..JobFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(page.jobs.len(), 1);
    assert_eq!(page.next_after, None);
    let job = h.job(&svc, job.id).await;
    assert_eq!(
        file_sha256(&h.target(&job.target_path)),
        content_sha256(50_000)
    );
    assert_eq!(h.archive_state(ep).await, ArchiveState::Archived);
    assert_eq!(
        h.sink
            .names()
            .iter()
            .filter(|n| **n == "download.queued")
            .count(),
        1
    );
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn bulk_enqueue_skips_what_cannot_be_downloaded() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    let ok = h.episode("/range/1000").await;
    let no_enclosure = h.episode("/range/1000").await;
    let duplicate = h.episode("/range/1000").await;
    let skipped = h.episode("/range/1000").await;
    let removed = h.episode("/range/1000").await;
    {
        let mut w = h.storage.writer().await.unwrap();
        sqlx::query("DELETE FROM enclosures WHERE episode_id = ?1")
            .bind(no_enclosure.to_string())
            .execute(&mut *w)
            .await
            .unwrap();
        sqlx::query("UPDATE episodes SET duplicate_of_episode_id = ?2 WHERE id = ?1")
            .bind(duplicate.to_string())
            .bind(ok.to_string())
            .execute(&mut *w)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE episodes SET archive_state = 'skipped', skip_reason = 'policy' WHERE id = ?1",
        )
        .bind(skipped.to_string())
        .execute(&mut *w)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE episodes SET removed_from_feed_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
        )
        .bind(removed.to_string())
        .execute(&mut *w)
        .await
        .unwrap();
    }
    let summary = svc.enqueue_podcast(h.podcast, Priority::Low).await.unwrap();
    assert_eq!(
        (
            summary.created,
            summary.existing,
            summary.requeued,
            summary.completed
        ),
        (1, 0, 0, 0)
    );
    let mut reasons: Vec<_> = summary.skipped.iter().map(|s| s.reason.as_str()).collect();
    reasons.sort_unstable();
    assert_eq!(
        reasons,
        [
            "duplicate_candidate",
            "no_enclosure",
            "removed_from_feed",
            "skipped"
        ]
    );
    // Refusals through the single-episode path are typed errors.
    assert!(matches!(
        svc.enqueue_episode(no_enclosure, Priority::Normal).await,
        Err(UguisuError::Invalid(_))
    ));
    assert!(matches!(
        svc.enqueue_episode(duplicate, Priority::Normal).await,
        Err(UguisuError::Conflict(_))
    ));
    assert!(matches!(
        svc.enqueue_episode(EpisodeId::new(), Priority::Normal)
            .await,
        Err(UguisuError::NotFound { .. })
    ));
    assert!(matches!(
        svc.enqueue_podcast(PodcastId::new(), Priority::Normal)
            .await,
        Err(UguisuError::NotFound { .. })
    ));
    // A second bulk run sees the existing job.
    let again = svc.enqueue_podcast(h.podcast, Priority::Low).await.unwrap();
    assert_eq!((again.created, again.existing), (0, 1));
}

#[tokio::test]
async fn priority_order_and_per_host_limits_hold() {
    let h = harness(config(4, 1), 2).await;
    let svc = h.service();
    // Host A: three jobs with different priorities; host B: two jobs.
    let a_normal = h.episode_on(0, "/slow/60000/200000").await;
    let a_high = h.episode_on(0, "/slow/60001/200000").await;
    let a_low = h.episode_on(0, "/slow/60002/200000").await;
    let b1 = h.episode_on(1, "/slow/60003/200000").await;
    let b2 = h.episode_on(1, "/slow/60004/200000").await;
    svc.enqueue_episode(a_normal, Priority::Normal)
        .await
        .unwrap();
    svc.enqueue_episode(a_high, Priority::High).await.unwrap();
    svc.enqueue_episode(a_low, Priority::Low).await.unwrap();
    svc.enqueue_episode(b1, Priority::Normal).await.unwrap();
    svc.enqueue_episode(b2, Priority::Normal).await.unwrap();
    run_to_idle(&svc).await;
    assert_eq!(
        h.servers[0].max_concurrent_total(),
        1,
        "per-host limit on A"
    );
    assert_eq!(
        h.servers[1].max_concurrent_total(),
        1,
        "per-host limit on B"
    );
    // Two hosts ran in parallel: a job of each host had started before the
    // first one completed (independent of how loaded the test machine is).
    let events = h.sink.events();
    let first_completed = events
        .iter()
        .position(|e| e.name() == "download.completed")
        .expect("something completed");
    let started_before: Vec<EpisodeId> = events[..first_completed]
        .iter()
        .filter(|e| e.name() == "download.started")
        .filter_map(|e| e.episode_id)
        .collect();
    assert!(
        started_before
            .iter()
            .any(|e| [a_normal, a_high, a_low].contains(e)),
        "host A started before the first completion: {started_before:?}"
    );
    assert!(
        started_before.iter().any(|e| [b1, b2].contains(e)),
        "host B started before the first completion: {started_before:?}"
    );
    let completed: Vec<EpisodeId> = events
        .into_iter()
        .filter(|e| e.name() == "download.completed")
        .filter_map(|e| e.episode_id)
        .collect();
    let a_order: Vec<EpisodeId> = completed
        .iter()
        .copied()
        .filter(|e| [a_normal, a_high, a_low].contains(e))
        .collect();
    assert_eq!(
        a_order,
        vec![a_high, a_normal, a_low],
        "priority DESC, then FIFO"
    );
    let stats = svc.stats().await.unwrap();
    assert_eq!(stats.by_state[&DownloadState::Completed], 5);
    assert_eq!(stats.running, 0);
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn queue_pause_persists_across_processes() {
    let h = harness(config(1, 1), 1).await;
    let svc = h.service();
    let eps = [
        h.episode("/slow/100000/100000").await,
        h.episode("/range/70000").await,
        h.episode("/range/70001").await,
    ];
    let mut ids = Vec::new();
    for e in eps {
        ids.push(
            svc.enqueue_episode(e, Priority::Normal)
                .await
                .unwrap()
                .job()
                .id,
        );
    }
    svc.start();
    wait_for("a transfer to have bytes", || {
        ids.iter().any(|id| svc.progress(*id).is_some())
    })
    .await;
    assert_eq!(svc.running_count(), 1);
    let control = svc.pause_all(PauseAllReason::User).await.unwrap();
    assert!(control.paused);
    wait_for("the paused worker to stop", || svc.running_count() == 0).await;
    let page = svc
        .list(&JobFilter {
            limit: 10,
            ..JobFilter::default()
        })
        .await
        .unwrap();
    assert!(
        page.jobs
            .iter()
            .all(|j| j.job.state == DownloadState::Paused),
        "{:?}",
        page.jobs.iter().map(|j| j.job.state).collect::<Vec<_>>()
    );
    assert!(
        page.jobs
            .iter()
            .all(|j| j.job.state_reason.as_deref() == Some("paused_all"))
    );
    let running_job = page
        .jobs
        .iter()
        .find(|j| j.job.bytes_downloaded > 0)
        .expect("the running one kept its bytes");
    assert!(h.target(&running_job.job.part_path).exists());
    let hits_before = h.servers[0].requests().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.servers[0].requests().len(),
        hits_before,
        "nothing is claimed while paused"
    );
    assert!(svc.is_idle().await.unwrap());
    svc.shutdown(Duration::from_secs(1)).await;

    // A new process sees the pause, then resumes everything.
    let svc2 = h.service();
    let stats = svc2.stats().await.unwrap();
    assert_eq!(stats.paused_all, Some(PauseAllReason::User));
    svc2.start();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(svc2.running_count(), 0, "still paused");
    svc2.resume_all().await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), svc2.wait_idle())
        .await
        .unwrap()
        .unwrap();
    let stats = svc2.stats().await.unwrap();
    assert_eq!(stats.by_state[&DownloadState::Completed], 3);
    assert_eq!(stats.paused_all, None);
    let names = h.sink.names();
    assert!(names.contains(&"download.paused_all") && names.contains(&"download.resumed_all"));
    svc2.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn the_retry_schedule_survives_a_restart() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    let ep = h.episode("/flaky/1/5000").await;
    let job_id = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    svc.start();
    // Attempt 1 answers 503 with Retry-After: 1.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let job = h.job(&svc, job_id).await;
    assert_eq!(job.state, DownloadState::Retrying);
    let due = job.next_attempt_at.unwrap();
    svc.shutdown(Duration::from_secs(1)).await;
    assert_eq!(h.servers[0].hits("/flaky/1/5000"), 1);

    // Restart: nothing happens before the scheduled time.
    let svc2 = h.service();
    svc2.reconcile(false).await.unwrap();
    svc2.start();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        h.servers[0].hits("/flaky/1/5000"),
        1,
        "no immediate retry after restart"
    );
    tokio::time::timeout(Duration::from_secs(10), svc2.wait_idle())
        .await
        .unwrap()
        .unwrap();
    let job = h.job(&svc2, job_id).await;
    assert_eq!(job.state, DownloadState::Completed);
    assert_eq!(job.attempt_count, 2);
    assert!(OffsetDateTime::now_utc() >= due);
    assert_eq!(h.servers[0].hits("/flaky/1/5000"), 2);
    svc2.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn shutdown_parks_jobs_restart_resumes() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    let ep = h.episode("/slow/400000/200000").await;
    let job_id = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    svc.start();
    tokio::time::sleep(Duration::from_millis(600)).await;
    svc.shutdown(Duration::from_secs(5)).await;
    let job = h.job(&svc, job_id).await;
    assert_eq!(
        (job.state, job.state_reason.as_deref()),
        (DownloadState::Queued, Some("shutdown"))
    );
    assert!(
        job.bytes_downloaded > 0 && job.bytes_downloaded < 400_000,
        "{}",
        job.bytes_downloaded
    );
    assert_eq!(
        std::fs::metadata(h.target(&job.part_path)).unwrap().len(),
        job.bytes_downloaded
    );

    let svc2 = h.service();
    let report = svc2.reconcile(false).await.unwrap();
    assert_eq!(report.recovered, 0, "a parked job is already queued");
    run_to_idle(&svc2).await;
    let job = h.job(&svc2, job_id).await;
    assert_eq!(job.state, DownloadState::Completed);
    assert_eq!(
        file_sha256(&h.target(&job.target_path)),
        content_sha256(400_000)
    );
    let reqs = h.servers[0].requests_for("/slow/400000/200000");
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[1].range_start().is_some(),
        "resumed with a Range request"
    );
    assert!(!h.target(&job.part_path).exists());
    svc2.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn reconcile_recovers_every_crash_boundary() {
    let cases: Vec<(FailPoint, u32, u32)> = vec![
        // fail point, expected recovered, expected finalized
        (FailPoint::AfterBytes(150_000), 1, 0),
        (FailPoint::BeforeFinalizingWrite, 1, 0),
        (FailPoint::AfterFinalizingWrite, 0, 1),
        (FailPoint::AfterRename, 0, 1),
    ];
    for (point, recovered, finalized) in cases {
        let h = harness(config(1, 1), 1).await;
        let injector = FailInjector::armed(point);
        let dying = DownloadService::new(h.deps(Some(injector.clone())));
        let ep = h.episode("/range/300000").await;
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
        let job = h.job(&dying, job_id).await;
        assert!(job.state.is_active(), "{point:?}: {}", job.state);

        // The next process repairs it.
        let svc = h.service();
        let report = svc.reconcile(false).await.unwrap();
        assert_eq!(
            (report.recovered, report.finalized),
            (recovered, finalized),
            "{point:?}: {report:?}"
        );
        run_to_idle(&svc).await;
        let job = h.job(&svc, job_id).await;
        assert_eq!(job.state, DownloadState::Completed, "{point:?}");
        assert_eq!(
            file_sha256(&h.target(&job.target_path)),
            content_sha256(300_000),
            "{point:?}"
        );
        assert!(!h.target(&job.part_path).exists(), "{point:?}");
        assert_eq!(h.archive_state(ep).await, ArchiveState::Archived);
        let detail = svc.job(job_id).await.unwrap();
        assert!(
            detail.attempts.iter().all(|a| a.finished_at.is_some()),
            "{point:?}: every attempt is closed"
        );
        svc.shutdown(Duration::from_secs(1)).await;
    }

    // finalizing with neither file: finalization_lost.
    let h = harness(config(1, 1), 1).await;
    let svc = h.service();
    let ep = h.episode("/range/1000").await;
    let job_id = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    let mut w = h.storage.writer().await.unwrap();
    sqlx::query("UPDATE download_jobs SET state = 'finalizing' WHERE id = ?1")
        .bind(job_id.to_string())
        .execute(&mut *w)
        .await
        .unwrap();
    drop(w);
    let report = svc.reconcile(false).await.unwrap();
    assert_eq!(report.finalization_lost, 1);
    let job = h.job(&svc, job_id).await;
    assert_eq!(
        (job.state, job.state_reason.as_deref()),
        (DownloadState::Failed, Some("finalization_lost"))
    );
    // A user retry brings it back.
    svc.retry(job_id).await.unwrap();
    run_to_idle(&svc).await;
    assert_eq!(h.job(&svc, job_id).await.state, DownloadState::Completed);
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn failed_repeat_rename_keeps_part() {
    let h = harness(config(1, 1), 1).await;
    let svc = h.service();
    let ep = h.episode("/range/1000").await;
    let job = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .clone();
    // A file where the target's folder belongs: the rename cannot happen.
    let mut w = h.storage.writer().await.unwrap();
    sqlx::query(
        "UPDATE download_jobs SET state = 'finalizing', target_path = 'blocked/a.mp3' WHERE id = ?1",
    )
    .bind(job.id.to_string())
    .execute(&mut *w)
    .await
    .unwrap();
    drop(w);
    let part = h.target(&job.part_path);
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, b"audio").unwrap();
    std::fs::write(h.target("blocked"), b"in the way").unwrap();

    let report = svc.reconcile(false).await.unwrap();
    assert_eq!(
        (report.finalization_failed, report.finalization_lost),
        (1, 0),
        "{report:?}"
    );
    let failed = h.job(&svc, job.id).await;
    assert_eq!(
        (failed.state, failed.state_reason.as_deref()),
        (DownloadState::Failed, Some("finalization"))
    );
    assert!(part.exists(), "the .part stays");
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn orphans_and_missing_targets_are_reported() {
    let h = harness(config(1, 1), 1).await;
    let svc = h.service();
    let ep = h.episode("/range/2000").await;
    let job_id = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    run_to_idle(&svc).await;
    let job = h.job(&svc, job_id).await;
    let tmp = h.media_dir.join(h.podcast.to_string()).join(".uguisu-tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let orphan = tmp.join(format!("{}.part", JobId::new()));
    std::fs::write(&orphan, b"stale").unwrap();
    std::fs::write(tmp.join("notes.txt"), b"ignored").unwrap();
    let report = svc.reconcile(false).await.unwrap();
    assert_eq!(report.orphan_parts.len(), 1);
    assert_eq!(
        std::path::Path::new(&report.orphan_parts[0])
            .extension()
            .and_then(|e| e.to_str()),
        Some("part")
    );
    assert!(report.orphan_parts[0].contains(".uguisu-tmp/"));
    assert!(orphan.exists(), "never deleted");
    assert_eq!(svc.stats().await.unwrap().orphan_parts, 1);
    assert_eq!(report.missing_targets, 0);

    std::fs::remove_file(h.target(&job.target_path)).unwrap();
    let shallow = svc.reconcile(false).await.unwrap();
    assert_eq!(shallow.missing_targets, 0, "shallow does not stat targets");
    let deep = svc.reconcile(true).await.unwrap();
    assert_eq!(deep.missing_targets, 1);
    assert_eq!(h.archive_state(ep).await, ArchiveState::Missing);
    assert_eq!(
        h.job(&svc, job_id).await.state,
        DownloadState::Completed,
        "the job stays completed"
    );
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn corrupt_host_key_fails_cleanly() {
    let h = harness(config(1, 1), 1).await;
    let svc = h.service();
    let ep = h.episode("/range/3000").await;
    let job = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .clone();
    {
        let mut w = h.storage.writer().await.unwrap();
        uguisu_storage::downloads::set_source(
            &mut w,
            job.id,
            &job.source_url,
            "not a host key",
            job.enclosure_id,
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    }
    run_to_idle(&svc).await;

    let detail = svc.job(job.id).await.unwrap();
    assert_eq!(
        (detail.job.state, detail.job.state_reason.as_deref()),
        (DownloadState::Failed, Some("validation"))
    );
    assert_eq!(detail.attempts.len(), 1, "{:?}", detail.attempts);
    assert!(detail.attempts[0].finished_at.is_some());
    assert_eq!(h.archive_state(ep).await, ArchiveState::Failed);
    let mut r = h.storage.reader().await.unwrap();
    let failed = uguisu_storage::events::list_after(&mut r, None, 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.name() == "download.failed")
        .count();
    assert_eq!(failed, 1);
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn user_commands_follow_the_table() {
    let h = harness(config(1, 1), 1).await;
    let svc = h.service();
    let ep = h.episode("/range/3000").await;
    let id = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    let j = svc.pause(id).await.unwrap();
    assert_eq!(
        (j.state, j.state_reason.as_deref()),
        (DownloadState::Paused, Some("user"))
    );
    assert!(matches!(svc.pause(id).await, Err(UguisuError::Conflict(_))));
    let j = svc.resume(id).await.unwrap();
    assert_eq!(
        (j.state, j.state_reason.as_deref()),
        (DownloadState::Queued, Some("resumed"))
    );
    let j = svc.cancel(id).await.unwrap();
    assert_eq!(j.state, DownloadState::Cancelled);
    assert!(j.finished_at.is_some());
    assert_eq!(h.archive_state(ep).await, ArchiveState::Expected);
    assert!(matches!(
        svc.resume(id).await,
        Err(UguisuError::Conflict(_))
    ));
    let j = svc.retry(id).await.unwrap();
    assert_eq!(
        (j.state, j.state_reason.as_deref(), j.attempt_count),
        (DownloadState::Queued, Some("requeued"), 0)
    );
    assert!(matches!(
        svc.cancel(JobId::new()).await,
        Err(UguisuError::NotFound { .. })
    ));
    // Enqueue on a cancelled job re-queues it.
    svc.cancel(id).await.unwrap();
    let again = svc.enqueue_episode(ep, Priority::High).await.unwrap();
    assert!(
        matches!(again, EnqueueOutcome::Requeued(ref j) if j.priority == Priority::High),
        "{again:?}"
    );
    run_to_idle(&svc).await;
    assert_eq!(h.job(&svc, id).await.state, DownloadState::Completed);
    for r in [
        svc.retry(id).await,
        svc.pause(id).await,
        svc.cancel(id).await,
    ] {
        assert!(matches!(r, Err(UguisuError::Conflict(_))));
    }
    let names = h.sink.names();
    for n in [
        "download.paused",
        "download.resumed",
        "download.cancelled",
        "download.queued",
        "download.completed",
    ] {
        assert!(names.contains(&n), "{n}");
    }

    // Cancel a running job: the worker stops and the .part stays.
    let ep = h.episode("/slow/400000/200000").await;
    let id = svc
        .enqueue_episode(ep, Priority::Normal)
        .await
        .unwrap()
        .job()
        .id;
    wait_for("the transfer to have bytes", || svc.progress(id).is_some()).await;
    assert_eq!(svc.running_count(), 1);
    let j = svc.cancel(id).await.unwrap();
    assert_eq!(j.state, DownloadState::Cancelled);
    wait_for("the cancelled worker to stop", || svc.running_count() == 0).await;
    let j = h.job(&svc, id).await;
    assert_eq!(j.state, DownloadState::Cancelled);
    assert!(j.bytes_downloaded > 0);
    assert!(h.target(&j.part_path).exists());
    let detail = svc.job(id).await.unwrap();
    assert!(detail.attempts[0].finished_at.is_some());
    // retry-failed touches only failed jobs.
    assert_eq!(svc.retry_failed().await.unwrap(), 0);
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn a_hundred_mixed_downloads_complete_cleanly() {
    let h = harness(config(8, 8), 1).await;
    let svc = h.service();
    let mut expected: Vec<(EpisodeId, u64, u32)> = Vec::new();
    for i in 0..100u64 {
        let size = 10_000 + i * 731;
        let (path, attempts) = match i % 7 {
            0 => (format!("/flaky/1/{size}"), 2),
            3 => (format!("/rate-limit/{size}"), 2),
            _ => (format!("/range/{size}"), 1),
        };
        let ep = h.episode(&path).await;
        expected.push((ep, size, attempts));
    }
    let summary = svc
        .enqueue_podcast(h.podcast, Priority::Normal)
        .await
        .unwrap();
    assert_eq!(summary.created, 100);
    run_to_idle(&svc).await;
    let stats = svc.stats().await.unwrap();
    assert_eq!(
        stats.by_state.get(&DownloadState::Completed),
        Some(&100),
        "{stats:?}"
    );
    assert!(h.servers[0].max_concurrent_total() <= 8);
    let mut checked = 0;
    for (ep, size, attempts) in expected {
        let job = svc.job_for_episode(ep).await.unwrap().unwrap();
        assert_eq!(job.state, DownloadState::Completed);
        assert_eq!(job.attempt_count, attempts, "{}", job.source_url);
        assert_eq!(
            job.hash_value.as_deref(),
            Some(content_sha256(size).as_str())
        );
        assert_eq!(
            file_sha256(&h.target(&job.target_path)),
            content_sha256(size)
        );
        let detail = svc.job(job.id).await.unwrap();
        assert_eq!(detail.attempts.len(), attempts as usize);
        assert!(detail.attempts.iter().all(|a| a.finished_at.is_some()));
        checked += 1;
    }
    assert_eq!(checked, 100);
    // Nothing but targets under the podcast directory; no .part left.
    let podcast_dir = h.media_dir.join(h.podcast.to_string());
    let mut parts = 0;
    let mut targets = 0;
    for entry in std::fs::read_dir(&podcast_dir).unwrap().flatten() {
        if entry.file_type().unwrap().is_dir() {
            assert_eq!(entry.file_name(), ".uguisu-tmp");
            parts += std::fs::read_dir(entry.path()).unwrap().count();
        } else {
            targets += 1;
        }
    }
    assert_eq!((targets, parts), (100, 0));
    svc.shutdown(Duration::from_secs(1)).await;
}

/// The host names the destination; the queue checks the answer.
#[derive(Debug)]
struct FixedTarget(&'static str);

impl uguisu_download::DestinationResolver for FixedTarget {
    fn target(&self, _request: &uguisu_download::DestinationRequest<'_>) -> Option<String> {
        Some(self.0.to_owned())
    }
}

#[tokio::test]
async fn a_safe_free_destination_is_used() {
    let h = harness(config(2, 2), 1).await;
    let svc = DownloadService::new(
        h.deps(None)
            .with_destinations(std::sync::Arc::new(FixedTarget("Show/2024/Readable.mp3"))),
    );
    let episode = h.episode("/normal/1024").await;
    let out = svc
        .enqueue_episode(episode, Priority::Normal)
        .await
        .unwrap();
    let job = match out {
        EnqueueOutcome::Created(job) => job,
        other => panic!("{other:?}"),
    };
    assert_eq!(job.target_path, "Show/2024/Readable.mp3");
    // The `.part` path is unchanged: finalization stays an intra-directory
    // rename regardless of what the destination resolver said.
    assert!(job.part_path.ends_with(".part"), "{}", job.part_path);
    assert!(job.part_path.contains(".uguisu-tmp"), "{}", job.part_path);
}

#[tokio::test]
async fn an_unusable_destination_falls_back() {
    for proposed in [
        "../escape.mp3",
        "/etc/passwd",
        "C:/Windows/x.mp3",
        "",
        "a/../../b.mp3",
    ] {
        let h = harness(config(2, 2), 1).await;
        let leaked: &'static str = Box::leak(proposed.to_owned().into_boxed_str());
        let svc = DownloadService::new(
            h.deps(None)
                .with_destinations(std::sync::Arc::new(FixedTarget(leaked))),
        );
        let episode = h.episode("/normal/1024").await;
        let job = match svc
            .enqueue_episode(episode, Priority::Normal)
            .await
            .unwrap()
        {
            EnqueueOutcome::Created(job) => job,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            job.target_path,
            format!("{}/{episode}.mp3", h.podcast),
            "`{proposed}` must not become a destination"
        );
    }

    // Two episodes cannot be aimed at one path: the second keeps the
    // identifier layout, so the first finalization is never overwritten.
    let h = harness(config(2, 2), 1).await;
    let svc = DownloadService::new(
        h.deps(None)
            .with_destinations(std::sync::Arc::new(FixedTarget("Show/Same.mp3"))),
    );
    let first = h.episode("/normal/1024").await;
    let second = h.episode("/normal/2048").await;
    let job_a = match svc.enqueue_episode(first, Priority::Normal).await.unwrap() {
        EnqueueOutcome::Created(job) => job,
        other => panic!("{other:?}"),
    };
    let job_b = match svc.enqueue_episode(second, Priority::Normal).await.unwrap() {
        EnqueueOutcome::Created(job) => job,
        other => panic!("{other:?}"),
    };
    assert_eq!(job_a.target_path, "Show/Same.mp3");
    assert_eq!(job_b.target_path, format!("{}/{second}.mp3", h.podcast));

    // Nor at a name something on the disk already has.
    let h = harness(config(2, 2), 1).await;
    std::fs::create_dir_all(h.target("Show")).unwrap();
    std::fs::write(h.target("Show/Taken.mp3"), b"somebody's").unwrap();
    let svc = DownloadService::new(
        h.deps(None)
            .with_destinations(std::sync::Arc::new(FixedTarget("Show/Taken.mp3"))),
    );
    let episode = h.episode("/normal/1024").await;
    let job = match svc
        .enqueue_episode(episode, Priority::Normal)
        .await
        .unwrap()
    {
        EnqueueOutcome::Created(job) => job,
        other => panic!("{other:?}"),
    };
    assert_eq!(job.target_path, format!("{}/{episode}.mp3", h.podcast));
}

#[tokio::test]
async fn a_claim_in_flight_is_busy() {
    // Regression: the scheduler commits `downloading` and *then* records
    // the worker. Both happen under the claim lock, but `is_idle` did not
    // take it, so it could look in between, find an empty map and nothing
    // claimable, and say the queue was idle with a transfer about to
    // start. `download run --wait` returned there, and the
    // shutdown/restart test failed on it under load.
    let h = harness(config(1, 1), 1).await;
    let svc = h.service();
    let ep = h.episode_on(0, "/ok/1000").await;
    svc.enqueue_episode(ep, Priority::Normal).await.unwrap();

    let guard = svc.hold_claims().await;
    let asking = tokio::spawn({
        let svc = svc.clone();
        async move { svc.is_idle().await }
    });
    // While a claim is in flight, the question has no answer yet.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut { asking })
            .await
            .is_err(),
        "is_idle answered while a claim was in progress"
    );
    drop(guard);
}
