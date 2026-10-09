//! Security properties of the queue: private and non-HTTP enclosure URLs
//! and redirects to private targets fail closed without a request or a
//! file, error details carry no secrets, paths come from ids only and the
//! byte cap holds.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use uguisu_core::download::{DownloadErrorKind, DownloadState, Priority};
use uguisu_core::events::EventKind;
use uguisu_core::ids::JobId;
use uguisu_download::DownloadService;
use uguisu_download::testing::content_sha256;

mod common;
use common::{Harness, config, file_sha256, harness, run_to_idle};

/// `.part` files anywhere under the media directory.
fn part_files(h: &Harness) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(podcasts) = std::fs::read_dir(&h.media_dir) else {
        return out;
    };
    for p in podcasts.flatten() {
        if let Ok(tmp) = std::fs::read_dir(p.path().join(".uguisu-tmp")) {
            out.extend(tmp.flatten().map(|e| e.path()));
        }
    }
    out
}

#[tokio::test]
async fn private_enclosure_urls_fail_closed() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    // A URL that is not an addressable http(s) resource is refused at
    // enqueue time: no job, no file, no request.
    for url in [
        "file:///etc/passwd",
        "data:audio/mpeg;base64,AAAA",
        "mailto:x@example.com",
    ] {
        let ep = h.episode_at(url, Some("audio/mpeg")).await;
        let err = svc.enqueue_episode(ep, Priority::Normal).await.unwrap_err();
        assert!(
            matches!(err, uguisu_core::UguisuError::Invalid(_)),
            "{url}: {err:?}"
        );
    }
    // A private or loopback address is a queued job that fails closed on
    // its first attempt: the network policy refuses before any connection.
    let private = [
        "http://169.254.169.254/latest/meta-data/x.mp3",
        "http://10.0.0.1/x.mp3",
        "http://192.168.1.10/x.mp3",
        "http://[fd00::1]/x.mp3",
        "http://[::1]:1/x.mp3",
        "http://localhost:1/x.mp3",
        "ftp://example.com/x.mp3",
    ];
    let mut jobs: Vec<(JobId, &str)> = Vec::new();
    for url in private {
        let ep = h.episode_at(url, Some("audio/mpeg")).await;
        let id = svc
            .enqueue_episode(ep, Priority::Normal)
            .await
            .unwrap()
            .job()
            .id;
        jobs.push((id, url));
    }
    run_to_idle(&svc).await;
    for (id, url) in jobs {
        let job = h.job(&svc, id).await;
        assert_eq!(
            (job.state, job.state_reason.as_deref()),
            (DownloadState::Failed, Some("not_retryable")),
            "{url}"
        );
        assert_eq!(job.attempt_count, 1, "{url}: no retry");
        assert!(
            matches!(
                job.last_error_kind,
                Some(
                    DownloadErrorKind::PolicyBlocked
                        | DownloadErrorKind::Http
                        | DownloadErrorKind::Validation
                )
            ),
            "{url}: {:?}",
            job.last_error_kind
        );
        assert!(!h.target(&job.target_path).exists(), "{url}");
        assert!(!h.target(&job.part_path).exists(), "{url}");
    }
    assert!(
        part_files(&h).is_empty(),
        "no partial file for any refused URL"
    );
    assert!(
        h.servers[0].requests().is_empty(),
        "the policy refuses before any connection"
    );
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn redirects_to_private_targets_fail_closed() {
    let h = harness(config(3, 3), 1).await;
    let svc = h.service();
    let mut jobs = Vec::new();
    for target in ["metadata", "lan", "loopback"] {
        let ep = h.episode(&format!("/redirect-private/{target}")).await;
        jobs.push((
            target,
            svc.enqueue_episode(ep, Priority::Normal)
                .await
                .unwrap()
                .job()
                .id,
        ));
    }
    run_to_idle(&svc).await;
    for (target, id) in jobs {
        let job = h.job(&svc, id).await;
        assert_eq!(job.state, DownloadState::Failed, "{target}");
        assert_eq!(
            job.last_error_kind,
            Some(DownloadErrorKind::PolicyBlocked),
            "{target}"
        );
        assert_eq!(job.attempt_count, 1, "{target}");
        assert!(!h.target(&job.part_path).exists(), "{target}");
        assert_eq!(
            h.servers[0].hits(&format!("/redirect-private/{target}")),
            1,
            "{target}: the redirect itself is the only request"
        );
    }
    assert!(part_files(&h).is_empty());
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn errors_and_events_carry_no_secrets() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    let mut jobs = Vec::new();
    for code in [401u16, 403, 404, 500] {
        let url = format!(
            "{}?token=SECRET123&sig=HIDDEN",
            h.servers[0].url(&format!("/status/{code}"))
        );
        let ep = h.episode_at(&url, Some("audio/mpeg")).await;
        jobs.push((
            code,
            svc.enqueue_episode(ep, Priority::Normal)
                .await
                .unwrap()
                .job()
                .id,
        ));
    }
    run_to_idle(&svc).await;
    let leaks = |text: &str| text.contains("SECRET123") || text.contains("HIDDEN");
    for (code, id) in jobs {
        let detail = svc.job(id).await.unwrap();
        assert_eq!(detail.job.last_http_status, Some(code));
        let d = detail.job.last_error_detail.clone().unwrap_or_default();
        assert!(!leaks(&d), "{code}: job detail leaks the query: {d}");
        for a in &detail.attempts {
            let d = a.error_detail.clone().unwrap_or_default();
            assert!(!leaks(&d), "{code}: attempt detail leaks the query: {d}");
        }
    }
    // `download.queued` and `download.started` carry the enclosure URL on
    // purpose (it is what the user asked for). Everything derived from the
    // response - failure details, retry details - must not echo it.
    for e in h.sink.events() {
        match &e.kind {
            EventKind::DownloadQueued { .. } | EventKind::DownloadStarted { .. } => {}
            EventKind::DownloadFailed { detail, .. }
            | EventKind::DownloadRetryScheduled { detail, .. } => {
                assert!(!leaks(detail), "{} leaks the query: {detail}", e.name());
            }
            _ => {
                let text = serde_json::to_string(&e).unwrap();
                assert!(!leaks(&text), "{} leaks the query: {text}", e.name());
            }
        }
    }
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn paths_are_derived_from_ids_only() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    // Hostile URLs and content types: the destination is built from ids and
    // a whitelisted extension, so none of this can reach the path.
    let cases: Vec<(String, Option<&str>)> = vec![
        (
            format!(
                "{}?name=..%2F..%2Fetc%2Fpasswd.mp3",
                h.servers[0].url("/range/1000")
            ),
            Some("audio/mpeg"),
        ),
        (h.servers[0].url("/range/1001"), Some("audio/../../x")),
        (
            h.servers[0].url("/range/1002"),
            Some("video/mp4; charset=\"../..\""),
        ),
        (
            format!("{}/..%2F..%2Fevil.exe", h.servers[0].url("/range/1003")),
            None,
        ),
        (
            h.servers[0].url("/range/1004/%2e%2e%2f%2e%2e%2fshadow"),
            None,
        ),
    ];
    let mut jobs = Vec::new();
    for (url, mime) in &cases {
        let ep = h.episode_at(url, *mime).await;
        jobs.push((
            ep,
            url.clone(),
            svc.enqueue_episode(ep, Priority::Normal)
                .await
                .unwrap()
                .job()
                .id,
        ));
    }
    run_to_idle(&svc).await;
    for (ep, url, id) in jobs {
        let job = h.job(&svc, id).await;
        let parts: Vec<&str> = job.target_path.split('/').collect();
        assert_eq!(parts.len(), 2, "{url}: {}", job.target_path);
        assert_eq!(parts[0], h.podcast.to_string(), "{url}");
        let (stem, ext) = parts[1].rsplit_once('.').expect("an extension");
        assert_eq!(stem, ep.to_string(), "{url}");
        assert!(
            (1..=5).contains(&ext.len())
                && ext
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "{url}: unsafe extension {ext}"
        );
        let tmp: Vec<&str> = job.part_path.split('/').collect();
        assert_eq!(
            tmp,
            vec![parts[0], ".uguisu-tmp", &format!("{id}.part")],
            "{url}"
        );
        let target = h.target(&job.target_path);
        assert!(target.starts_with(&h.media_dir), "{url}");
        assert!(
            !target
                .components()
                .any(|c| c == std::path::Component::ParentDir),
            "{url}: {} walks up",
            target.display()
        );
        if target.exists() {
            let real = target.canonicalize().unwrap();
            assert!(
                real.starts_with(h.media_dir.canonicalize().unwrap()),
                "{url}: {} resolves outside the media directory",
                real.display()
            );
        }
    }
    assert!(part_files(&h).is_empty());
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn the_byte_cap_stops_oversized_bodies() {
    let mut cfg = config(2, 2);
    cfg.max_bytes = 100_000;
    let h = harness(cfg, 1).await;
    let svc = h.service();
    let declared = h.episode("/range/200000").await;
    let chunked = h.episode("/no-length/200000").await;
    let fine = h.episode("/range/90000").await;
    let ids = [
        svc.enqueue_episode(declared, Priority::Normal)
            .await
            .unwrap()
            .job()
            .id,
        svc.enqueue_episode(chunked, Priority::Normal)
            .await
            .unwrap()
            .job()
            .id,
        svc.enqueue_episode(fine, Priority::Normal)
            .await
            .unwrap()
            .job()
            .id,
    ];
    run_to_idle(&svc).await;
    for id in &ids[..2] {
        let job = h.job(&svc, *id).await;
        assert_eq!(
            job.state,
            DownloadState::Failed,
            "{:?}",
            job.last_error_detail
        );
        assert_eq!(job.last_error_kind, Some(DownloadErrorKind::Validation));
        assert_eq!(job.attempt_count, 1, "no retry for an oversized body");
        assert!(!h.target(&job.target_path).exists());
    }
    let ok = h.job(&svc, ids[2]).await;
    assert_eq!(ok.state, DownloadState::Completed);
    assert_eq!(
        file_sha256(&h.target(&ok.target_path)),
        content_sha256(90_000)
    );
    svc.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn a_service_without_workers_stays_idle() {
    let h = harness(config(2, 2), 1).await;
    let svc = h.service();
    let ep = h.episode("/range/1000").await;
    svc.enqueue_episode(ep, Priority::Normal).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.servers[0].requests().is_empty());
    assert!(
        !h.media_dir.exists(),
        "the media directory is created lazily"
    );
    assert!(!svc.is_started());
    let _ = DownloadService::clone(&svc);
}
