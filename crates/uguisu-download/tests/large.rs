//! Large bodies: 1, 10 and 100 MiB always; 1 GiB with `UGUISU_TEST_LARGE=1`.
//! On Linux the process's peak resident set must stay within 64 MiB of
//! the baseline, which proves the body is streamed, never buffered.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss
)]

use std::time::{Duration, Instant};

use uguisu_core::download::{DownloadState, Priority};
use uguisu_download::testing::content_sha256;

mod common;
use common::{config, file_sha256, harness};

const MIB: u64 = 1024 * 1024;

/// Peak resident set size in KiB (Linux only).
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn check_memory(baseline: Option<u64>, what: &str) {
    let (Some(before), Some(after)) = (baseline, peak_rss_kib()) else {
        return;
    };
    let grew = after.saturating_sub(before) / 1024;
    eprintln!("{what}: peak RSS grew by {grew} MiB");
    assert!(grew < 64, "{what}: peak RSS grew by {grew} MiB (limit 64)");
}

async fn download_sizes(sizes: &[u64], what: &str) {
    let baseline = peak_rss_kib();
    let mut cfg = config(sizes.len().max(1), sizes.len().max(1));
    cfg.max_bytes = 2 * 1024 * MIB;
    let h = harness(cfg, 1).await;
    let svc = h.service();
    let mut jobs = Vec::new();
    for size in sizes {
        let ep = h.episode(&format!("/range/{size}")).await;
        jobs.push((
            *size,
            svc.enqueue_episode(ep, Priority::Normal)
                .await
                .unwrap()
                .job()
                .id,
        ));
    }
    let started = Instant::now();
    svc.start();
    tokio::time::timeout(Duration::from_secs(600), svc.wait_idle())
        .await
        .expect("large downloads finish")
        .unwrap();
    let elapsed = started.elapsed();
    check_memory(baseline, what);
    let total: u64 = sizes.iter().sum();
    eprintln!(
        "{what}: {} MiB in {elapsed:.1?} ({:.0} MiB/s)",
        total / MIB,
        (total / MIB) as f64 / elapsed.as_secs_f64().max(0.001)
    );
    for (size, id) in jobs {
        let job = h.job(&svc, id).await;
        assert_eq!(
            job.state,
            DownloadState::Completed,
            "{size}: {:?}",
            job.last_error_detail
        );
        assert_eq!(job.bytes_downloaded, size);
        assert_eq!(job.attempt_count, 1);
        let target = h.target(&job.target_path);
        assert_eq!(std::fs::metadata(&target).unwrap().len(), size);
        assert_eq!(
            job.hash_value.as_deref(),
            Some(content_sha256(size).as_str())
        );
        assert_eq!(file_sha256(&target), content_sha256(size), "{size}");
        assert!(!h.target(&job.part_path).exists());
    }
    svc.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_ten_and_a_hundred_mebibytes_complete_with_bounded_memory() {
    download_sizes(&[MIB, 10 * MIB, 100 * MIB], "1+10+100 MiB").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_gibibyte_when_asked() {
    if std::env::var("UGUISU_TEST_LARGE").as_deref() != Ok("1") {
        eprintln!("skipped: set UGUISU_TEST_LARGE=1 to download 1 GiB");
        return;
    }
    download_sizes(&[1024 * MIB], "1 GiB").await;
}
