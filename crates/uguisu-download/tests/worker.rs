//! The per-job downloader against the scenario media server: statuses,
//! the resume matrix, validation, stops, disk-full, fail points.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::case_sensitive_file_extension_comparisons
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uguisu_core::config::DownloadConfig;
use uguisu_core::download::{
    AttemptOutcome, DownloadErrorKind, DownloadJob, DownloadState, PauseAllReason, Priority,
};
use uguisu_core::ids::{EpisodeId, JobId, PodcastId};
use uguisu_core::model::ArchiveState;
use uguisu_download::deps::{FailInjector, FailPoint};
use uguisu_download::paths::{Destination, extension_for};
use uguisu_download::testing::{MediaServer, content_bytes, content_sha256};
use uguisu_download::{
    CollectingSink, Deps, FixedSpace, JobHandle, JobOutcome, SpaceProbe, StopReason, claim, run_job,
};
use uguisu_http::{
    ClientConfig, HostKey, HostThrottles, HttpClient, NetworkPolicy, Profile, RetryPolicy,
    ThrottleConfig,
};
use uguisu_storage::downloads::{self, Transition};
use uguisu_storage::{Storage, episodes, podcasts};
use url::Url;

struct Harness {
    _dir: tempfile::TempDir,
    media_dir: PathBuf,
    storage: Storage,
    server: MediaServer,
    sink: Arc<CollectingSink>,
    deps: Deps,
    podcast: PodcastId,
}

fn config() -> DownloadConfig {
    DownloadConfig {
        max_attempts: 3,
        backoff_base: Duration::from_millis(10),
        backoff_max: Duration::from_secs(5),
        idle_timeout: Duration::from_millis(400),
        progress_interval: Duration::from_millis(20),
        min_free_bytes: 0,
        ..DownloadConfig::default()
    }
}

async fn harness_with(cfg: DownloadConfig, space: Arc<dyn SpaceProbe>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let media_dir = dir.path().join("media");
    let storage = Storage::open_temp().await.unwrap();
    let server = MediaServer::start().await;
    let sink = Arc::new(CollectingSink::default());
    let client = HttpClient::new(
        Profile::Media,
        ClientConfig {
            policy: Arc::new(NetworkPolicy::strict().allow_private_hosts(["127.0.0.1"])),
            retry: RetryPolicy::none(),
            request_timeout: Duration::from_secs(5),
            ..ClientConfig::default()
        },
    )
    .unwrap();
    let deps = Deps {
        storage: storage.clone(),
        client,
        hosts: Arc::new(HostThrottles::new(ThrottleConfig::concurrency_only(1))),
        sink: sink.clone(),
        config: cfg,
        media_dir: media_dir.clone(),
        space,
        destinations: Arc::new(uguisu_download::IdentityLayout),
        shutdown: CancellationToken::new(),
        injector: None,
    };
    let p = podcasts::sample("Show");
    let mut tx = storage.begin().await.unwrap();
    podcasts::insert(&mut tx, &p).await.unwrap();
    tx.commit().await.unwrap();
    Harness {
        _dir: dir,
        media_dir,
        storage,
        server,
        sink,
        deps,
        podcast: p.id,
    }
}

async fn harness() -> Harness {
    harness_with(config(), Arc::new(FixedSpace(u64::MAX))).await
}

impl Harness {
    /// An episode whose enclosure points at `path` on the media server.
    async fn episode(&self, path: &str, length: Option<u64>) -> EpisodeId {
        let mut e = episodes::sample(
            self.podcast,
            &format!("k{}", EpisodeId::new()),
            "Ep",
            OffsetDateTime::now_utc(),
        );
        e.enclosures[0].url = Url::parse(&self.server.url(path)).unwrap();
        e.enclosures[0].length_bytes = length;
        let mut tx = self.storage.begin().await.unwrap();
        episodes::upsert_all(&mut tx, std::slice::from_ref(&e))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        e.id
    }

    /// A queued job for the episode's enclosure.
    async fn job(&self, episode_id: EpisodeId) -> JobId {
        let mut r = self.storage.reader().await.unwrap();
        let ep = episodes::get(&mut r, episode_id).await.unwrap().unwrap();
        drop(r);
        let enc = ep.primary_enclosure().unwrap();
        let id = JobId::new();
        let ext = extension_for(enc.mime_type.as_deref(), &enc.url);
        let dest = Destination::for_job(self.podcast, episode_id, id, &ext);
        let now = OffsetDateTime::now_utc();
        let job = DownloadJob {
            id,
            episode_id,
            podcast_id: self.podcast,
            enclosure_id: Some(enc.id),
            source_url: enc.url.clone(),
            host_key: HostKey::of(&enc.url).unwrap().as_str().to_owned(),
            state: DownloadState::Queued,
            state_reason: Some("user".into()),
            priority: Priority::Normal,
            attempt_count: 0,
            max_attempts: self.deps.config.max_attempts,
            next_attempt_at: None,
            bytes_downloaded: 0,
            total_bytes: None,
            part_path: dest.part,
            target_path: dest.target,
            content_type: None,
            sniffed_type: None,
            etag: None,
            last_modified: None,
            accept_ranges: None,
            hash_algo: "sha256".into(),
            hash_value: None,
            last_http_status: None,
            last_error_kind: None,
            last_error_detail: None,
            claimed_at: None,
            progress_at: None,
            started_at: None,
            finished_at: None,
            created_at: now,
            updated_at: now,
        };
        let mut tx = self.storage.begin().await.unwrap();
        downloads::insert_job(&mut tx, &job).await.unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn run_with(&self, deps: &Deps, id: JobId) -> (JobOutcome, DownloadJob) {
        let claimed = claim(deps, id, OffsetDateTime::now_utc())
            .await
            .unwrap()
            .expect("claimable");
        let handle = JobHandle::new(&deps.shutdown);
        let outcome = run_job(deps, &handle, claimed).await;
        (outcome, self.get(id).await)
    }

    async fn run(&self, id: JobId) -> (JobOutcome, DownloadJob) {
        self.run_with(&self.deps, id).await
    }

    async fn get(&self, id: JobId) -> DownloadJob {
        let mut r = self.storage.reader().await.unwrap();
        downloads::get_job(&mut r, id).await.unwrap().unwrap()
    }

    async fn archive_state(&self, episode: EpisodeId) -> ArchiveState {
        let mut r = self.storage.reader().await.unwrap();
        episodes::get(&mut r, episode)
            .await
            .unwrap()
            .unwrap()
            .archive_state
    }

    fn target(&self, job: &DownloadJob) -> PathBuf {
        self.media_dir.join(&job.target_path)
    }

    fn part(&self, job: &DownloadJob) -> PathBuf {
        self.media_dir.join(&job.part_path)
    }

    async fn set_urls(&self, job: &DownloadJob, path: &str) {
        let url = self.server.url(path);
        let mut w = self.storage.writer().await.unwrap();
        sqlx::query("UPDATE download_jobs SET source_url = ?1 WHERE id = ?2")
            .bind(&url)
            .bind(job.id.to_string())
            .execute(&mut *w)
            .await
            .unwrap();
        sqlx::query("UPDATE enclosures SET url = ?1 WHERE episode_id = ?2")
            .bind(&url)
            .bind(job.episode_id.to_string())
            .execute(&mut *w)
            .await
            .unwrap();
    }

    async fn sql(&self, sql: &str, id: JobId) {
        let mut w = self.storage.writer().await.unwrap();
        sqlx::query(sql)
            .bind(id.to_string())
            .execute(&mut *w)
            .await
            .unwrap();
    }
}

fn file_sha256(p: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(std::fs::read(p).unwrap()))
}

#[tokio::test]
async fn downloads_validates_and_finalizes() {
    let h = harness().await;
    let ep = h.episode("/range/300000", Some(300_000)).await;
    let id = h.job(ep).await;
    let (outcome, job) = h.run(id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(job.state, DownloadState::Completed);
    assert_eq!(job.bytes_downloaded, 300_000);
    assert_eq!(job.total_bytes, Some(300_000));
    assert_eq!(
        job.hash_value.as_deref(),
        Some(content_sha256(300_000).as_str())
    );
    assert_eq!(job.content_type.as_deref(), Some("audio/mpeg"));
    assert_eq!(job.accept_ranges, Some(true));
    assert!(job.finished_at.is_some());
    assert_eq!(job.attempt_count, 1);
    let target = h.target(&job);
    assert!(target.exists() && !h.part(&job).exists());
    assert_eq!(file_sha256(&target), content_sha256(300_000));
    assert!(job.target_path.ends_with(".mp3"));
    assert_eq!(h.archive_state(ep).await, ArchiveState::Archived);
    let mut r = h.storage.reader().await.unwrap();
    let attempts = downloads::list_attempts(&mut r, id).await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].outcome, Some(AttemptOutcome::Completed));
    assert_eq!(attempts[0].bytes_received, 300_000);
    assert_eq!(attempts[0].http_status, Some(200));
    let names = h.sink.names();
    assert_eq!(names.first(), Some(&"download.started"));
    assert_eq!(names.last(), Some(&"download.completed"));
    let stored = uguisu_storage::events::list_after(&mut r, None, 100)
        .await
        .unwrap();
    assert!(
        stored.iter().all(|e| e.name() != "download.progress"),
        "progress is never stored"
    );
    assert_eq!(h.server.requests().len(), 1);
}

#[tokio::test]
async fn status_matrix_and_attempt_budget() {
    let h = harness().await;
    for code in [400u16, 401, 403, 404, 410, 418] {
        let ep = h.episode(&format!("/status/{code}"), None).await;
        let id = h.job(ep).await;
        let (outcome, job) = h.run(id).await;
        assert_eq!(
            outcome,
            JobOutcome::Failed {
                reason: "not_retryable".into()
            },
            "{code}"
        );
        assert_eq!(job.state, DownloadState::Failed);
        assert_eq!(job.last_http_status, Some(code));
        assert_eq!(h.archive_state(ep).await, ArchiveState::Failed);
        let expected = match code {
            401 => DownloadErrorKind::Unauthorized,
            403 => DownloadErrorKind::Forbidden,
            404 | 410 => DownloadErrorKind::NotFound,
            _ => DownloadErrorKind::Http,
        };
        assert_eq!(job.last_error_kind, Some(expected));
        assert!(
            !job.last_error_detail
                .as_deref()
                .unwrap()
                .contains("retry-after")
        );
    }
    for code in [408u16, 425, 429, 500, 502, 503, 504] {
        let ep = h.episode(&format!("/status/{code}"), None).await;
        let id = h.job(ep).await;
        let before = OffsetDateTime::now_utc();
        let (outcome, job) = h.run(id).await;
        let JobOutcome::RetryScheduled { next_attempt_at } = outcome else {
            panic!("{code}: {outcome:?}");
        };
        assert_eq!(job.state, DownloadState::Retrying);
        assert_eq!(
            job.next_attempt_at,
            Some(next_attempt_at.replace_nanosecond(0).unwrap()),
            "stored at second precision"
        );
        assert!(next_attempt_at >= before);
        if code == 429 || code == 503 {
            assert!(
                next_attempt_at >= before + time::Duration::milliseconds(900),
                "Retry-After wins"
            );
        }
        assert_eq!(h.archive_state(ep).await, ArchiveState::Queued);
        assert_eq!(
            job.last_error_kind,
            Some(if code == 429 {
                DownloadErrorKind::RateLimited
            } else {
                DownloadErrorKind::Http
            })
        );
        // Spend the budget: attempts 2 and 3.
        h.sql(
            "UPDATE download_jobs SET next_attempt_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
            id,
        )
        .await;
        let (o2, _) = h.run(id).await;
        assert!(
            matches!(o2, JobOutcome::RetryScheduled { .. }),
            "{code}: {o2:?}"
        );
        h.sql(
            "UPDATE download_jobs SET next_attempt_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
            id,
        )
        .await;
        let (o3, job) = h.run(id).await;
        assert_eq!(
            o3,
            JobOutcome::Failed {
                reason: "max_attempts".into()
            },
            "{code}"
        );
        assert_eq!(job.attempt_count, 3);
        let mut r = h.storage.reader().await.unwrap();
        let attempts = downloads::list_attempts(&mut r, id).await.unwrap();
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[0].outcome, Some(AttemptOutcome::RetryScheduled));
        assert_eq!(attempts[2].outcome, Some(AttemptOutcome::Failed));
    }
    assert!(h.sink.names().contains(&"download.retry_scheduled"));
    assert!(h.sink.names().contains(&"download.failed"));
}

/// Attempt 1 is cut off after 100 000 of 300 000 bytes; returns the job
/// (retrying, 100 000 bytes on disk, strong ETag recorded).
async fn interrupted(h: &Harness) -> DownloadJob {
    let ep = h.episode("/disconnect/300000/100000", Some(300_000)).await;
    let id = h.job(ep).await;
    let (outcome, job) = h.run(id).await;
    assert!(
        matches!(outcome, JobOutcome::RetryScheduled { .. }),
        "{outcome:?}"
    );
    assert_eq!(job.state, DownloadState::Retrying);
    assert!(
        matches!(
            job.last_error_kind,
            Some(DownloadErrorKind::ContentLengthMismatch | DownloadErrorKind::Network)
        ),
        "a cut connection is a network error or a short body: {:?}",
        job.last_error_kind
    );
    assert_eq!(job.bytes_downloaded, 100_000);
    assert_eq!(job.total_bytes, Some(300_000));
    assert_eq!(job.etag.as_deref(), Some("\"gen-300000\""));
    assert_eq!(job.accept_ranges, Some(true));
    assert_eq!(std::fs::metadata(h.part(&job)).unwrap().len(), 100_000);
    h.sql(
        "UPDATE download_jobs SET next_attempt_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
        id,
    )
    .await;
    job
}

#[tokio::test]
async fn resume_when_db_and_file_agree() {
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));
    let reqs = h.server.requests_for("/range/300000");
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].range_start(), Some(100_000));
    assert_eq!(reqs[0].if_range.as_deref(), Some("\"gen-300000\""));
    assert_eq!(job.attempt_count, 2);
    let names = h.sink.names();
    let started: Vec<_> = h
        .sink
        .events()
        .into_iter()
        .filter(|e| e.name() == "download.started")
        .collect();
    assert_eq!(started.len(), 2);
    assert_eq!(
        names.iter().filter(|n| **n == "download.completed").count(),
        1
    );
    let mut r = h.storage.reader().await.unwrap();
    let attempts = downloads::list_attempts(&mut r, job.id).await.unwrap();
    assert_eq!(attempts[1].range_start, 100_000);
    assert_eq!(attempts[1].bytes_received, 200_000);
    assert_eq!(attempts[1].http_status, Some(206));
}

#[tokio::test]
async fn resume_reconciles_any_part_length() {
    // Longer: an unacknowledged tail is discarded.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(h.part(&job))
            .unwrap();
        f.write_all(&vec![0xEE; 50_000]).unwrap();
    }
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));
    assert_eq!(
        h.server.requests_for("/range/300000")[0].range_start(),
        Some(100_000)
    );

    // Shorter: resume from the file's length.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    std::fs::OpenOptions::new()
        .write(true)
        .open(h.part(&job))
        .unwrap()
        .set_len(60_000)
        .unwrap();
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));
    assert_eq!(
        h.server.requests_for("/range/300000")[0].range_start(),
        Some(60_000)
    );

    // Missing: start over.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    std::fs::remove_file(h.part(&job)).unwrap();
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));
    assert_eq!(h.server.requests_for("/range/300000")[0].range, None);
}

#[tokio::test]
async fn resume_needs_a_strong_validator() {
    // Weak ETag and no Last-Modified: no resume.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    h.sql(
        "UPDATE download_jobs SET etag = 'W/\"x\"', last_modified = NULL WHERE id = ?1",
        job.id,
    )
    .await;
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(h.server.requests_for("/range/300000")[0].range, None);
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));

    // The resource changed (If-Range mismatch → 200): restart from zero.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/changed-etag/300000").await;
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    let req = &h.server.requests_for("/changed-etag/300000")[0];
    assert_eq!(req.range_start(), Some(100_000));
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));
    let mut r = h.storage.reader().await.unwrap();
    let attempts = downloads::list_attempts(&mut r, job.id).await.unwrap();
    assert!(
        attempts[1]
            .error_detail
            .as_deref()
            .unwrap()
            .contains("range_ignored")
    );
    assert_eq!(attempts[1].bytes_received, 300_000);

    // No range support: restart from zero.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/no-range/300000").await;
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));
    assert_eq!(job.accept_ranges, Some(false));
}

#[tokio::test]
async fn complete_part_files_finalize_without_a_body() {
    // Total known and equal to the bytes on disk: no request at all.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    std::fs::write(h.part(&job), content_bytes(300_000, 0, 300_000)).unwrap();
    h.sql(
        "UPDATE download_jobs SET bytes_downloaded = 300000, total_bytes = 300000 WHERE id = ?1",
        job.id,
    )
    .await;
    h.server.reset();
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert!(h.server.requests().is_empty());
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));

    // Total unknown: the Range request answers 416 with the matching total.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    std::fs::write(h.part(&job), content_bytes(300_000, 0, 300_000)).unwrap();
    h.sql(
        "UPDATE download_jobs SET bytes_downloaded = 300000, total_bytes = NULL WHERE id = ?1",
        job.id,
    )
    .await;
    h.server.reset();
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(h.server.requests().len(), 1);
    assert_eq!(job.last_http_status, Some(416));
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));

    // 416 whose total disagrees: restart from zero.
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/200000").await;
    std::fs::write(h.part(&job), vec![1u8; 300_000]).unwrap();
    h.sql(
        "UPDATE download_jobs SET bytes_downloaded = 300000, total_bytes = NULL WHERE id = ?1",
        job.id,
    )
    .await;
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(job.bytes_downloaded, 200_000);
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(200_000));
}

#[tokio::test]
async fn bad_range_answers_fail_without_retry() {
    for path in ["/content-range-bad/300000", "/range-wrong-start/300000"] {
        let h = harness().await;
        let job = interrupted(&h).await;
        h.set_urls(&job, path).await;
        let (outcome, job) = h.run(job.id).await;
        assert_eq!(
            outcome,
            JobOutcome::Failed {
                reason: "not_retryable".into()
            },
            "{path}"
        );
        assert_eq!(job.last_error_kind, Some(DownloadErrorKind::RangeInvalid));
        assert!(!h.target(&job).exists());
    }
}

#[tokio::test]
async fn slow_bodies_finish_but_stalls_time_out() {
    let h = harness().await;
    let ep = h.episode("/slow/20000/40000", None).await;
    let (outcome, job) = h.run(h.job(ep).await).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(job.bytes_downloaded, 20_000);

    let ep = h.episode("/stall/100000/1000", None).await;
    let (outcome, job) = h.run(h.job(ep).await).await;
    assert!(
        matches!(outcome, JobOutcome::RetryScheduled { .. }),
        "{outcome:?}"
    );
    assert_eq!(job.last_error_kind, Some(DownloadErrorKind::Timeout));
    assert_eq!(job.bytes_downloaded, 1000);
    assert_eq!(std::fs::metadata(h.part(&job)).unwrap().len(), 1000);
}

#[tokio::test]
async fn no_length_and_gzip_bodies_complete() {
    let h = harness().await;
    let ep = h.episode("/no-length/70000", None).await;
    let (outcome, job) = h.run(h.job(ep).await).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(job.total_bytes, Some(70_000));
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(70_000));

    let ep = h.episode("/gzip/1000", None).await;
    let (outcome, job) = h.run(h.job(ep).await).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(
        file_sha256(&h.target(&job)),
        content_sha256(1000),
        "stored as served"
    );
    let mut r = h.storage.reader().await.unwrap();
    let attempts = downloads::list_attempts(&mut r, job.id).await.unwrap();
    assert!(
        attempts[0]
            .error_detail
            .as_deref()
            .unwrap()
            .contains("content_encoding")
    );
}

#[tokio::test]
async fn policy_and_source_problems_fail_for_good() {
    let h = harness().await;
    for target in ["metadata", "lan", "loopback"] {
        let ep = h
            .episode(&format!("/redirect-private/{target}"), None)
            .await;
        let (outcome, job) = h.run(h.job(ep).await).await;
        assert_eq!(
            outcome,
            JobOutcome::Failed {
                reason: "not_retryable".into()
            }
        );
        assert_eq!(job.last_error_kind, Some(DownloadErrorKind::PolicyBlocked));
        assert!(!h.part(&job).exists(), "nothing was written");
    }
    // Episode gone.
    let ep = h.episode("/range/1000", None).await;
    let id = h.job(ep).await;
    h.sql(
        "UPDATE download_jobs SET episode_id = episode_id WHERE id = ?1",
        id,
    )
    .await;
    let mut w = h.storage.writer().await.unwrap();
    sqlx::query("DELETE FROM enclosures WHERE episode_id = ?1")
        .bind(ep.to_string())
        .execute(&mut *w)
        .await
        .unwrap();
    drop(w);
    let (outcome, job) = h.run(id).await;
    assert_eq!(
        outcome,
        JobOutcome::Failed {
            reason: "source_missing".into()
        }
    );
    assert_eq!(job.state, DownloadState::Failed);
    // Target exists.
    let ep = h.episode("/range/1000", None).await;
    let id = h.job(ep).await;
    let job = h.get(id).await;
    std::fs::create_dir_all(h.target(&job).parent().unwrap()).unwrap();
    std::fs::write(h.target(&job), b"keep me").unwrap();
    let (outcome, job) = h.run(id).await;
    assert_eq!(
        outcome,
        JobOutcome::Failed {
            reason: "target_exists".into()
        }
    );
    assert_eq!(std::fs::read(h.target(&job)).unwrap(), b"keep me");
    assert!(h.server.requests_for("/range/1000").is_empty());
}

#[tokio::test]
async fn a_changed_enclosure_url_restarts_the_transfer() {
    let h = harness().await;
    let job = interrupted(&h).await;
    // Only the episode changes; the job still points at the old URL.
    let url = h.server.url("/range/300000");
    let mut w = h.storage.writer().await.unwrap();
    sqlx::query("UPDATE enclosures SET url = ?1 WHERE episode_id = ?2")
        .bind(&url)
        .bind(job.episode_id.to_string())
        .execute(&mut *w)
        .await
        .unwrap();
    drop(w);
    let (outcome, job) = h.run(job.id).await;
    assert_eq!(outcome, JobOutcome::Completed);
    assert_eq!(job.source_url.as_str(), url);
    let reqs = h.server.requests_for("/range/300000");
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].range, None, "restarted from zero");
    assert_eq!(file_sha256(&h.target(&job)), content_sha256(300_000));
}

async fn running(
    h: &Harness,
    path: &str,
) -> (JobId, Arc<JobHandle>, tokio::task::JoinHandle<JobOutcome>) {
    let ep = h.episode(path, None).await;
    let id = h.job(ep).await;
    let claimed = claim(&h.deps, id, OffsetDateTime::now_utc())
        .await
        .unwrap()
        .unwrap();
    let handle = JobHandle::new(&h.deps.shutdown);
    let deps = h.deps.clone();
    let hd = handle.clone();
    let task = tokio::spawn(async move { run_job(&deps, &hd, claimed).await });
    tokio::time::sleep(Duration::from_millis(350)).await;
    (id, handle, task)
}

#[tokio::test]
async fn cancel_pause_and_shutdown_keep_the_part() {
    let h = harness().await;
    // User cancel: the command writes the state first, then stops the worker.
    let (id, handle, task) = running(&h, "/slow/400000/200000").await;
    let mut w = h.storage.writer().await.unwrap();
    let t = Transition {
        to: DownloadState::Cancelled,
        reason: Some("user"),
        finished: true,
        ..Transition::default()
    };
    assert!(
        downloads::transition(
            &mut w,
            id,
            &[DownloadState::Downloading],
            &t,
            OffsetDateTime::now_utc()
        )
        .await
        .unwrap()
    );
    drop(w);
    handle.stop(StopReason::User);
    assert_eq!(task.await.unwrap(), JobOutcome::Cancelled);
    let job = h.get(id).await;
    assert_eq!(job.state, DownloadState::Cancelled);
    assert!(job.bytes_downloaded > 0);
    assert_eq!(
        std::fs::metadata(h.part(&job)).unwrap().len(),
        job.bytes_downloaded
    );
    let mut r = h.storage.reader().await.unwrap();
    let a = downloads::list_attempts(&mut r, id).await.unwrap();
    assert_eq!(a[0].outcome, Some(AttemptOutcome::Cancelled));
    drop(r);

    // pause-all: the worker persists paused(paused_all).
    let (id, handle, task) = running(&h, "/slow/400000/200000").await;
    handle.stop(StopReason::PausedAll);
    assert_eq!(
        task.await.unwrap(),
        JobOutcome::Paused {
            reason: "paused_all".into()
        }
    );
    let job = h.get(id).await;
    assert_eq!(
        (job.state, job.state_reason.as_deref()),
        (DownloadState::Paused, Some("paused_all"))
    );
    assert!(h.part(&job).exists());
    assert!(h.sink.names().contains(&"download.paused"));
    assert_eq!(h.archive_state(job.episode_id).await, ArchiveState::Queued);

    // shutdown: queued(shutdown), attempt interrupted.
    let (id, handle, task) = running(&h, "/slow/400000/200000").await;
    handle.stop(StopReason::Shutdown);
    assert_eq!(task.await.unwrap(), JobOutcome::Requeued);
    let job = h.get(id).await;
    assert_eq!(
        (job.state, job.state_reason.as_deref()),
        (DownloadState::Queued, Some("shutdown"))
    );
    let mut r = h.storage.reader().await.unwrap();
    let a = downloads::list_attempts(&mut r, id).await.unwrap();
    assert_eq!(a[0].outcome, Some(AttemptOutcome::Interrupted));
    assert_eq!(a[0].bytes_received, job.bytes_downloaded);
}

#[tokio::test]
async fn a_full_disk_spends_no_attempt() {
    // Pre-check.
    let h = harness_with(config(), Arc::new(FixedSpace(10))).await;
    let ep = h.episode("/range/300000", Some(300_000)).await;
    let (outcome, job) = h.run(h.job(ep).await).await;
    assert_eq!(
        outcome,
        JobOutcome::Paused {
            reason: "disk_full".into()
        }
    );
    assert_eq!(job.attempt_count, 0, "refunded");
    assert_eq!(job.last_error_kind, Some(DownloadErrorKind::DiskFull));
    let mut r = h.storage.reader().await.unwrap();
    let control = downloads::control_get(&mut r).await.unwrap();
    assert!(control.paused);
    assert_eq!(control.paused_reason, Some(PauseAllReason::DiskFull));
    let names = h.sink.names();
    assert!(names.contains(&"download.paused") && names.contains(&"download.paused_all"));
    assert!(h.server.requests().is_empty());
    drop(r);

    // ENOSPC while writing.
    let h = harness().await;
    let deps = Deps {
        injector: Some(FailInjector::armed(FailPoint::WriteError(
            50_000,
            std::io::ErrorKind::StorageFull,
        ))),
        ..h.deps.clone()
    };
    let ep = h.episode("/range/300000", Some(300_000)).await;
    let id = h.job(ep).await;
    let (outcome, job) = h.run_with(&deps, id).await;
    assert_eq!(
        outcome,
        JobOutcome::Paused {
            reason: "disk_full".into()
        }
    );
    assert_eq!(job.attempt_count, 0);
    assert!(h.part(&job).exists());
    let mut r = h.storage.reader().await.unwrap();
    assert!(downloads::control_get(&mut r).await.unwrap().paused);
}

#[tokio::test]
async fn fail_points_leave_the_expected_traces() {
    let h = harness().await;
    let cases: Vec<(FailPoint, DownloadState, bool, bool)> = vec![
        // point, state left in the DB, .part present, target present
        (
            FailPoint::AfterBytes(150_000),
            DownloadState::Downloading,
            true,
            false,
        ),
        (
            FailPoint::BeforeFinalizingWrite,
            DownloadState::Downloading,
            true,
            false,
        ),
        (
            FailPoint::AfterFinalizingWrite,
            DownloadState::Finalizing,
            true,
            false,
        ),
        (
            FailPoint::AfterRename,
            DownloadState::Finalizing,
            false,
            true,
        ),
    ];
    for (point, state, part, target) in cases {
        let injector = FailInjector::armed(point);
        let deps = Deps {
            injector: Some(injector.clone()),
            ..h.deps.clone()
        };
        let ep = h.episode("/range/300000", Some(300_000)).await;
        let id = h.job(ep).await;
        let (outcome, job) = h.run_with(&deps, id).await;
        assert_eq!(outcome, JobOutcome::Died, "{point:?}");
        assert!(injector.was_hit());
        assert_eq!(job.state, state, "{point:?}");
        assert_eq!(h.part(&job).exists(), part, "{point:?}");
        assert_eq!(h.target(&job).exists(), target, "{point:?}");
        if point == FailPoint::AfterBytes(150_000) {
            let len = std::fs::metadata(h.part(&job)).unwrap().len();
            assert!(len <= 150_000 + 64 * 1024, "{len}");
            assert!(
                job.bytes_downloaded <= len,
                "the database never claims more than the file holds"
            );
        }
        if state == DownloadState::Finalizing {
            assert_eq!(
                job.hash_value.as_deref(),
                Some(content_sha256(300_000).as_str())
            );
        }
        let mut r = h.storage.reader().await.unwrap();
        let a = downloads::list_attempts(&mut r, id).await.unwrap();
        assert_eq!(a[0].finished_at, None, "{point:?}: the attempt stays open");
    }
}

#[tokio::test]
async fn ignored_range_restart_uses_whole_cap() {
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/no-range/300000").await;
    // 100 000 on disk: the 200 that answers the Range restarts from zero, and
    // the whole file fits the cap although it is more than what was left.
    let mut capped = h.deps.clone();
    capped.config.max_bytes = 350_000;
    let (outcome, after) = h.run_with(&capped, job.id).await;
    assert_eq!(outcome, JobOutcome::Completed, "{after:?}");
    assert_eq!(file_sha256(&h.target(&after)), content_sha256(300_000));
}

#[tokio::test]
async fn resumed_file_respects_the_cap() {
    let h = harness().await;
    let job = interrupted(&h).await;
    h.set_urls(&job, "/range/300000").await;
    // 300 000 bytes in all, 100 000 on disk: a resumed response of 200 000
    // fits a cap of 250 000 on its own, the whole file does not.
    let mut capped = h.deps.clone();
    capped.config.max_bytes = 250_000;
    let (_, after) = h.run_with(&capped, job.id).await;
    assert_eq!(after.state, DownloadState::Failed, "{after:?}");
    assert_eq!(after.last_error_kind, Some(DownloadErrorKind::Validation));
    assert!(
        !h.target(&after).exists(),
        "nothing over the cap was finished"
    );
}
