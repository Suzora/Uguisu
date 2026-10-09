//! Shared harness of the queue-level integration tests: a temp media
//! directory, a file-backed temporary database, one or more scenario
//! media servers and a collecting event sink.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uguisu_core::config::DownloadConfig;
use uguisu_core::download::DownloadJob;
use uguisu_core::ids::{EpisodeId, JobId, PodcastId};
use uguisu_core::model::ArchiveState;
use uguisu_download::deps::FailInjector;
use uguisu_download::testing::MediaServer;
use uguisu_download::{CollectingSink, Deps, DownloadService, FixedSpace};
use uguisu_http::{
    ClientConfig, HostThrottles, HttpClient, NetworkPolicy, Profile, RetryPolicy, ThrottleConfig,
};
use uguisu_storage::{Storage, episodes, podcasts};
use url::Url;

pub struct Harness {
    pub dir: tempfile::TempDir,
    pub media_dir: PathBuf,
    pub storage: Storage,
    pub servers: Vec<MediaServer>,
    pub sink: Arc<CollectingSink>,
    pub config: DownloadConfig,
    pub podcast: PodcastId,
}

pub fn config(global: usize, per_host: usize) -> DownloadConfig {
    DownloadConfig {
        global_concurrency: global,
        per_host_concurrency: per_host,
        max_attempts: 3,
        backoff_base: Duration::from_millis(10),
        backoff_max: Duration::from_secs(5),
        idle_timeout: Duration::from_millis(800),
        progress_interval: Duration::from_millis(20),
        min_free_bytes: 0,
        shutdown_grace: Duration::from_secs(5),
        ..DownloadConfig::default()
    }
}

pub async fn harness(cfg: DownloadConfig, servers: usize) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let media_dir = dir.path().join("media");
    let storage = Storage::open_temp().await.unwrap();
    let mut list = Vec::new();
    for _ in 0..servers {
        list.push(MediaServer::start().await);
    }
    let p = podcasts::sample("Show");
    let mut tx = storage.begin().await.unwrap();
    podcasts::insert(&mut tx, &p).await.unwrap();
    tx.commit().await.unwrap();
    Harness {
        dir,
        media_dir,
        storage,
        servers: list,
        sink: Arc::new(CollectingSink::default()),
        config: cfg,
        podcast: p.id,
    }
}

impl Harness {
    pub fn deps(&self, injector: Option<Arc<FailInjector>>) -> Deps {
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
        Deps {
            storage: self.storage.clone(),
            client,
            hosts: Arc::new(HostThrottles::new(ThrottleConfig::concurrency_only(
                self.config.per_host_concurrency,
            ))),
            sink: self.sink.clone(),
            config: self.config.clone(),
            media_dir: self.media_dir.clone(),
            space: Arc::new(FixedSpace(u64::MAX)),
            destinations: Arc::new(uguisu_download::IdentityLayout),
            shutdown: CancellationToken::new(),
            injector,
        }
    }

    /// A fresh service on the same database and media directory (a "process").
    pub fn service(&self) -> DownloadService {
        DownloadService::new(self.deps(None))
    }

    pub async fn episode_on(&self, server: usize, path: &str) -> EpisodeId {
        let mut e = episodes::sample(
            self.podcast,
            &format!("k{}", EpisodeId::new()),
            "Ep",
            OffsetDateTime::now_utc(),
        );
        e.enclosures[0].url = Url::parse(&self.servers[server].url(path)).unwrap();
        e.enclosures[0].length_bytes = None;
        let mut tx = self.storage.begin().await.unwrap();
        episodes::upsert_all(&mut tx, std::slice::from_ref(&e))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        e.id
    }

    pub async fn episode(&self, path: &str) -> EpisodeId {
        self.episode_on(0, path).await
    }

    pub fn target(&self, rel: &str) -> PathBuf {
        self.media_dir.join(rel)
    }

    pub async fn job(&self, svc: &DownloadService, id: JobId) -> DownloadJob {
        svc.job(id).await.unwrap().job
    }

    pub async fn archive_state(&self, episode: EpisodeId) -> ArchiveState {
        let mut r = self.storage.reader().await.unwrap();
        episodes::get(&mut r, episode)
            .await
            .unwrap()
            .unwrap()
            .archive_state
    }

    /// Closes the database pools and opens the same file again: what a
    /// process restart does. Services created before this call hold dead
    /// pools; create new ones with [`service`](Self::service).
    pub async fn reopen(&mut self) {
        let path = self.storage.path().to_path_buf();
        self.storage.close().await;
        self.storage = Storage::open_path(&path).await.unwrap();
    }

    /// An episode whose enclosure is an arbitrary URL (not on a media server).
    pub async fn episode_at(&self, url: &str, mime: Option<&str>) -> EpisodeId {
        let mut e = episodes::sample(
            self.podcast,
            &format!("k{}", EpisodeId::new()),
            "Ep",
            OffsetDateTime::now_utc(),
        );
        e.enclosures[0].url = Url::parse(url).unwrap();
        e.enclosures[0].length_bytes = None;
        if let Some(m) = mime {
            e.enclosures[0].mime_type = Some(m.to_owned());
        }
        let mut tx = self.storage.begin().await.unwrap();
        episodes::upsert_all(&mut tx, std::slice::from_ref(&e))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        e.id
    }

    /// Bytes on disk for a job's `.part`, if any.
    pub fn part_len(&self, job: &DownloadJob) -> Option<u64> {
        std::fs::metadata(self.target(&job.part_path))
            .ok()
            .map(|m| m.len())
    }
}

pub fn file_sha256(p: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(std::fs::read(p).unwrap()))
}

/// Polls `f` every 20 ms until it holds; fails after 20 s.
pub async fn wait_for<F: Fn() -> bool>(what: &str, f: F) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn run_to_idle(svc: &DownloadService) {
    svc.start();
    tokio::time::timeout(Duration::from_secs(60), svc.wait_idle())
        .await
        .expect("queue drains")
        .unwrap();
}
