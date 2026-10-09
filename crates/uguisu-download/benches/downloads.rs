//! Download engine benchmarks on loopback (`docs/benchmarks/`). No public
//! network: bodies come from the scenario media server in this process, so
//! the numbers measure the engine (streaming, hashing, progress writes,
//! finalization, queue admission), not an internet connection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use time::OffsetDateTime;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use uguisu_core::Event;
use uguisu_core::config::DownloadConfig;
use uguisu_core::download::{DownloadJob, DownloadState, Priority};
use uguisu_core::events::EventKind;
use uguisu_core::ids::{EpisodeId, JobId, PodcastId};
use uguisu_download::paths::{Destination, extension_for};
use uguisu_download::testing::{MediaServer, content_bytes};
use uguisu_download::{
    Deps, DownloadService, EventSink, FixedSpace, JobHandle, NoopSink, claim, run_job,
};
use uguisu_http::{
    ClientConfig, HostKey, HostThrottles, HttpClient, NetworkPolicy, Profile, RetryPolicy,
    ThrottleConfig,
};
use uguisu_storage::{Storage, downloads, episodes, podcasts};
use url::Url;

const MIB: u64 = 1024 * 1024;

/// Records when the first `download.started` of a run was published.
#[derive(Debug, Default)]
struct FirstStarted {
    at: std::sync::Mutex<Option<Instant>>,
    count: AtomicU64,
}

impl FirstStarted {
    fn reset(&self) {
        *self.at.lock().unwrap() = None;
        self.count.store(0, Ordering::SeqCst);
    }

    fn elapsed_since(&self, from: Instant) -> Option<Duration> {
        self.at.lock().unwrap().map(|at| at.duration_since(from))
    }
}

impl EventSink for FirstStarted {
    fn publish(&self, events: &[Event]) {
        for e in events {
            if matches!(e.kind, EventKind::DownloadStarted { .. }) {
                self.count.fetch_add(1, Ordering::SeqCst);
                let mut at = self.at.lock().unwrap();
                if at.is_none() {
                    *at = Some(Instant::now());
                }
            }
        }
    }
}

struct Bench {
    _dir: tempfile::TempDir,
    media_dir: PathBuf,
    storage: Storage,
    server: MediaServer,
    podcast: PodcastId,
    config: DownloadConfig,
    sink: Arc<dyn EventSink>,
}

fn config(global: usize) -> DownloadConfig {
    DownloadConfig {
        global_concurrency: global,
        per_host_concurrency: global,
        max_attempts: 1,
        idle_timeout: Duration::from_secs(10),
        progress_interval: Duration::from_secs(1),
        min_free_bytes: 0,
        max_bytes: 4 * 1024 * MIB,
        shutdown_grace: Duration::from_secs(5),
        ..DownloadConfig::default()
    }
}

async fn setup(cfg: DownloadConfig, sink: Arc<dyn EventSink>) -> Bench {
    let dir = tempfile::tempdir().unwrap();
    let media_dir = dir.path().join("media");
    let storage = Storage::open_temp().await.unwrap();
    let server = MediaServer::start().await;
    let p = podcasts::sample("Bench Show");
    let mut tx = storage.begin().await.unwrap();
    podcasts::insert(&mut tx, &p).await.unwrap();
    tx.commit().await.unwrap();
    Bench {
        _dir: dir,
        media_dir,
        storage,
        server,
        podcast: p.id,
        config: cfg,
        sink,
    }
}

impl Bench {
    fn deps(&self) -> Deps {
        let client = HttpClient::new(
            Profile::Media,
            ClientConfig {
                policy: Arc::new(NetworkPolicy::strict().allow_private_hosts(["127.0.0.1"])),
                retry: RetryPolicy::none(),
                request_timeout: Duration::from_secs(30),
                ..ClientConfig::default()
            },
        )
        .unwrap();
        Deps::new(
            self.storage.clone(),
            client,
            Arc::new(HostThrottles::new(ThrottleConfig::concurrency_only(
                self.config.per_host_concurrency,
            ))),
            Arc::clone(&self.sink),
            self.config.clone(),
            self.media_dir.clone(),
            Arc::new(FixedSpace(u64::MAX)),
            CancellationToken::new(),
        )
    }

    async fn episode(&self, path: &str) -> EpisodeId {
        let mut e = episodes::sample(
            self.podcast,
            &format!("k{}", EpisodeId::new()),
            "Ep",
            OffsetDateTime::now_utc(),
        );
        e.enclosures[0].url = Url::parse(&self.server.url(path)).unwrap();
        e.enclosures[0].length_bytes = None;
        let mut tx = self.storage.begin().await.unwrap();
        episodes::upsert_all(&mut tx, std::slice::from_ref(&e))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        e.id
    }

    /// A queued job row for the episode, optionally pre-seeded with a
    /// `.part` prefix and the validators a resume needs.
    async fn job(&self, episode_id: EpisodeId, size: u64, prefill: u64) -> JobId {
        let mut r = self.storage.reader().await.unwrap();
        let ep = episodes::get(&mut r, episode_id).await.unwrap().unwrap();
        drop(r);
        let enc = ep.primary_enclosure().unwrap();
        let id = JobId::new();
        let ext = extension_for(enc.mime_type.as_deref(), &enc.url);
        let dest = Destination::for_job(self.podcast, episode_id, id, &ext);
        let now = OffsetDateTime::now_utc();
        if prefill > 0 {
            let part = self.media_dir.join(&dest.part);
            std::fs::create_dir_all(part.parent().unwrap()).unwrap();
            std::fs::write(&part, content_bytes(size, 0, prefill)).unwrap();
        }
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
            max_attempts: 1,
            next_attempt_at: None,
            bytes_downloaded: prefill,
            total_bytes: (prefill > 0).then_some(size),
            part_path: dest.part,
            target_path: dest.target,
            content_type: None,
            sniffed_type: None,
            etag: (prefill > 0).then(|| format!("\"gen-{size}\"")),
            last_modified: None,
            accept_ranges: (prefill > 0).then_some(true),
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

    /// Runs one job to its end through the real worker (no scheduler).
    async fn run_one(&self, deps: &Deps, id: JobId) {
        let claimed = claim(deps, id, OffsetDateTime::now_utc())
            .await
            .unwrap()
            .expect("claimable");
        let handle = JobHandle::new(&deps.shutdown);
        let outcome = run_job(deps, &handle, claimed).await;
        assert!(
            format!("{outcome:?}").contains("Completed"),
            "{outcome:?} for {id}"
        );
    }

    /// Deletes everything downloaded so far so long runs do not fill the disk.
    fn clear_media(&self) {
        let _ = std::fs::remove_dir_all(&self.media_dir);
    }
}

/// Total payload per iteration; split across the workers under test.
const PAYLOAD: u64 = 32 * MIB;

fn bench_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut g = c.benchmark_group("download_throughput");
    g.sample_size(10);
    g.throughput(Throughput::Bytes(PAYLOAD));
    for &workers in &[1usize, 4, 8] {
        let each = PAYLOAD / workers as u64;
        let b = rt.block_on(setup(config(workers), Arc::new(NoopSink)));
        g.bench_function(BenchmarkId::new("workers", workers), |bench| {
            let b = &b;
            bench.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let svc = DownloadService::new(b.deps());
                    for _ in 0..workers {
                        let ep = b.episode(&format!("/range/{each}")).await;
                        svc.enqueue_episode(ep, Priority::Normal).await.unwrap();
                    }
                    let started = Instant::now();
                    svc.start();
                    svc.wait_idle().await.unwrap();
                    total += started.elapsed();
                    svc.shutdown(Duration::from_secs(5)).await;
                    b.clear_media();
                }
                total
            });
        });
        rt.block_on(async { b.storage.close().await });
        drop(b);
    }
    g.finish();
}

fn bench_resume(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut g = c.benchmark_group("download_resume");
    g.sample_size(10);
    let size = 32 * MIB;
    let b = rt.block_on(setup(config(1), Arc::new(NoopSink)));
    let deps = b.deps();

    g.throughput(Throughput::Bytes(size));
    g.bench_function("fresh_32MiB", |bench| {
        let (b, deps) = (&b, &deps);
        bench.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let ep = b.episode(&format!("/range/{size}")).await;
                let id = b.job(ep, size, 0).await;
                let started = Instant::now();
                b.run_one(deps, id).await;
                total += started.elapsed();
                b.clear_media();
            }
            total
        });
    });
    // Half the file is already on disk: the attempt re-hashes 16 MiB and
    // transfers the other 16 MiB with a Range request.
    g.throughput(Throughput::Bytes(size / 2));
    g.bench_function("resume_from_half", |bench| {
        let (b, deps) = (&b, &deps);
        bench.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let ep = b.episode(&format!("/range/{size}")).await;
                let id = b.job(ep, size, size / 2).await;
                let started = Instant::now();
                b.run_one(deps, id).await;
                total += started.elapsed();
                b.clear_media();
            }
            total
        });
    });
    // The whole file is on disk: no transfer at all, just re-hash, fsync
    // and rename - the cost of finalizing.
    for &size in &[32 * MIB, 100 * MIB] {
        g.throughput(Throughput::Bytes(size));
        g.bench_function(BenchmarkId::new("finalize_only", size / MIB), |bench| {
            let (b, deps) = (&b, &deps);
            bench.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let ep = b.episode(&format!("/range/{size}")).await;
                    let id = b.job(ep, size, size).await;
                    let started = Instant::now();
                    b.run_one(deps, id).await;
                    total += started.elapsed();
                    b.clear_media();
                }
                total
            });
        });
    }
    rt.block_on(async { b.storage.close().await });
    g.finish();
}

fn bench_queue(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut g = c.benchmark_group("download_queue");
    g.sample_size(10);
    // Enqueue cost per episode, one row each, no workers running.
    for &n in &[100usize, 1_000] {
        let b = rt.block_on(setup(config(1), Arc::new(NoopSink)));
        let mut episodes = Vec::new();
        rt.block_on(async {
            for _ in 0..n {
                episodes.push(b.episode("/range/64").await);
            }
        });
        g.throughput(Throughput::Elements(n as u64));
        g.bench_function(BenchmarkId::new("enqueue", n), |bench| {
            let (b, episodes) = (&b, &episodes);
            bench.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let svc = DownloadService::new(b.deps());
                    let started = Instant::now();
                    for ep in episodes {
                        svc.enqueue_episode(*ep, Priority::Normal).await.unwrap();
                    }
                    total += started.elapsed();
                    // Enqueue is idempotent, so the rows have to go before
                    // the next iteration or it would measure the "already
                    // queued" path instead.
                    let mut w = b.storage.writer().await.unwrap();
                    sqlx::query("DELETE FROM download_jobs")
                        .execute(&mut *w)
                        .await
                        .unwrap();
                }
                total
            });
        });
        rt.block_on(async { b.storage.close().await });
    }

    // Time from `start()` to the first `download.started` with a full queue.
    {
        let n = 1_000usize;
        let sink = Arc::new(FirstStarted::default());
        let b = rt.block_on(setup(config(3), sink.clone()));
        rt.block_on(async {
            let svc = DownloadService::new(b.deps());
            for _ in 0..n {
                let ep = b.episode("/range/64").await;
                svc.enqueue_episode(ep, Priority::Normal).await.unwrap();
            }
        });
        g.throughput(Throughput::Elements(1));
        g.bench_function(BenchmarkId::new("first_claim_of", n), |bench| {
            let (b, sink) = (&b, &sink);
            bench.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    // Put every job back in the queue without touching files.
                    let mut w = b.storage.writer().await.unwrap();
                    sqlx::query(
                        "UPDATE download_jobs SET state = 'queued', state_reason = 'user', \
                         attempt_count = 0, bytes_downloaded = 0, finished_at = NULL",
                    )
                    .execute(&mut *w)
                    .await
                    .unwrap();
                    sqlx::query("DELETE FROM download_attempts")
                        .execute(&mut *w)
                        .await
                        .unwrap();
                    drop(w);
                    b.clear_media();
                    sink.reset();
                    let svc = DownloadService::new(b.deps());
                    let started = Instant::now();
                    svc.start();
                    while sink.elapsed_since(started).is_none() {
                        tokio::time::sleep(Duration::from_micros(200)).await;
                    }
                    total += sink.elapsed_since(started).unwrap();
                    svc.shutdown(Duration::from_secs(5)).await;
                }
                total
            });
        });
        rt.block_on(async { b.storage.close().await });
        drop(b);
    }
    g.finish();
}

criterion_group!(benches, bench_throughput, bench_resume, bench_queue);
criterion_main!(benches);
