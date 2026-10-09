//! Shared helpers for engine integration tests: a temporary data
//! directory, a wiremock server standing in for feed hosts, and the
//! parser fixture corpus. Tests never touch the network.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use uguisu_core::config::{DataConfig, DiscoveryConfig, DownloadConfig, FeedConfig};
use uguisu_core::feed::RefreshReport;
use uguisu_core::ids::PodcastId;
use uguisu_download::testing::MediaServer;
use uguisu_engine::{Engine, EngineConfig, RefreshOptions};
use uguisu_http::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A running engine on a temporary data directory plus a mock feed host
/// and a scenario media server for enclosures.
pub struct Harness {
    pub dir: tempfile::TempDir,
    pub server: MockServer,
    pub media: MediaServer,
    pub engine: Engine,
}

impl Harness {
    pub async fn new() -> Self {
        Self::with_feed_config(FeedConfig::default()).await
    }

    pub async fn with_feed_config(feed: FeedConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let media = MediaServer::start().await;
        let engine = Engine::open(config(dir.path().to_path_buf(), feed))
            .await
            .unwrap();
        Self {
            dir,
            server,
            media,
            engine,
        }
    }

    /// Where downloaded media lands (`<data_dir>/media`).
    pub fn media_dir(&self) -> PathBuf {
        self.engine.config().data.media_dir().unwrap()
    }

    /// Absolute URL of a path on the mock server.
    pub fn url(&self, p: &str) -> String {
        format!("{}{p}", self.server.uri())
    }

    /// Serves `body` at `p` with optional validators.
    pub async fn serve(
        &self,
        p: &str,
        body: &[u8],
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) {
        let mut template = ResponseTemplate::new(200)
            .set_body_bytes(body.to_vec())
            .insert_header("content-type", "application/rss+xml; charset=utf-8");
        if let Some(e) = etag {
            template = template.insert_header("etag", e);
        }
        if let Some(lm) = last_modified {
            template = template.insert_header("last-modified", lm);
        }
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(template)
            .mount(&self.server)
            .await;
    }

    /// Forgets every mounted response.
    pub async fn reset(&self) {
        self.server.reset().await;
    }

    /// Serves `name` at `/feed.xml` and adds it as a podcast.
    pub async fn add_fixture(&self, name: &str) -> uguisu_engine::library::AddOutcome {
        self.serve_fixture("/feed.xml", name).await;
        self.engine
            .add_podcast(&self.url("/feed.xml"), CancellationToken::new())
            .await
            .unwrap()
    }

    /// Refreshes a podcast with or without `force`.
    pub async fn refresh(&self, id: PodcastId, force: bool) -> RefreshReport {
        self.engine
            .refresh_podcast(
                id,
                RefreshOptions {
                    force,
                    cancel: None,
                },
            )
            .await
            .unwrap()
    }

    /// Serves a corpus fixture at `p`.
    pub async fn serve_fixture(&self, p: &str, name: &str) {
        self.serve(p, &fixture(name), None, None).await;
    }

    /// Re-opens the engine on the same data directory (the current handle
    /// must have been dropped first).
    pub async fn reopen(&mut self) {
        self.engine = Engine::open(config(self.dir.path().to_path_buf(), FeedConfig::default()))
            .await
            .unwrap();
    }

    /// Closes the engine, drops its handle (releasing the lock) and opens
    /// a fresh one on the same data directory, optionally with a download
    /// fail point armed: a "process restart".
    pub async fn restart(
        &mut self,
        injector: Option<std::sync::Arc<uguisu_download::deps::FailInjector>>,
    ) {
        self.engine.close().await;
        let placeholder = Engine::open(config(
            tempfile::tempdir().unwrap().keep(),
            FeedConfig::default(),
        ))
        .await
        .unwrap();
        drop(std::mem::replace(&mut self.engine, placeholder));
        self.engine = Engine::open_with_injector(
            config(self.dir.path().to_path_buf(), FeedConfig::default()),
            injector,
        )
        .await
        .unwrap();
    }
}

/// Engine configuration for tests: temp data dir, mock host allowed,
/// providers off, short timeouts.
pub fn config(data_dir: PathBuf, feed: FeedConfig) -> EngineConfig {
    let mut discovery = DiscoveryConfig::default();
    discovery.apple.enabled = false;
    discovery.gpoddernet.enabled = false;
    discovery.network.allow_private_hosts = vec!["127.0.0.1".to_owned()];
    discovery.network.connect_timeout = Duration::from_secs(2);
    discovery.network.request_timeout = Duration::from_secs(5);
    EngineConfig::default()
        .with_data(DataConfig {
            data_dir: Some(data_dir),
            media_dir: None,
        })
        .with_feed(feed)
        .with_discovery(discovery)
        .with_download(DownloadConfig {
            global_concurrency: 3,
            per_host_concurrency: 2,
            max_attempts: 3,
            backoff_base: Duration::from_millis(20),
            backoff_max: Duration::from_secs(2),
            idle_timeout: Duration::from_secs(2),
            progress_interval: Duration::from_millis(20),
            min_free_bytes: 0,
            shutdown_grace: Duration::from_secs(5),
            ..DownloadConfig::default()
        })
}

/// Root of the shared parser corpus.
pub fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/feeds/parser")
}

/// A corpus fixture by file name.
pub fn fixture(name: &str) -> Vec<u8> {
    let p = corpus_root().join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A probe fixture by file name.
pub fn probe_fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/feeds/probe")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The body of a recorded live feed (`tests/fixtures/discovery/live/web`).
pub fn live_feed(host: &str, case: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/discovery/live/web")
        .join(host)
        .join(format!("{case}.json"));
    let json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
    )
    .unwrap();
    json["response"]["body"]
        .as_str()
        .unwrap()
        .as_bytes()
        .to_vec()
}

/// A synthetic RSS feed with `count` items, all with GUIDs, dates and
/// enclosures; item `i` gets guid `ep-{i}` so subsets are easy to build.
pub fn synthetic_feed(count: usize) -> Vec<u8> {
    synthetic_feed_range(0..count)
}

/// A synthetic feed whose enclosures point at the media server: item `i`
/// gets `<base>/<scenarios[i % len]>` (a scenario path such as
/// `/range/100000`).
pub fn synthetic_feed_with_media(count: usize, base: &str, scenarios: &[&str]) -> Vec<u8> {
    let body = String::from_utf8(synthetic_feed(count)).unwrap();
    let mut out = body;
    for i in 0..count {
        let from = format!("https://cdn.synthetic.example/media/{i}.mp3");
        let to = format!("{base}{}", scenarios[i % scenarios.len()]);
        out = out.replace(&from, &to);
    }
    out.into_bytes()
}

/// A synthetic feed whose items are the given indices.
pub fn synthetic_feed_range(indices: impl IntoIterator<Item = usize>) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\" xmlns:podcast=\"https://podcastindex.org/namespace/1.0\">\n<channel>\n<title>Synthetic Show</title>\n<link>https://synthetic.example/</link>\n<description>Generated for tests</description>\n<itunes:author>Generator</itunes:author>\n<podcast:guid>7d1c4b8e-0000-4000-8000-00000000abcd</podcast:guid>\n",
    );
    for i in indices {
        let day = 1 + (i % 28);
        let month = 1 + (i / 28) % 12;
        let year = 2020 + (i / 336) % 6;
        let _ = writeln!(
            s,
            "<item><title>Episode {i}: the one about &amp; things</title><guid isPermaLink=\"false\">ep-{i}</guid>\
             <pubDate>{day:02} {mon} {year} 10:00:00 +0000</pubDate>\
             <description><![CDATA[<p>Show notes for episode <b>{i}</b> with a <a href=\"https://synthetic.example/{i}\">link</a>.</p>]]></description>\
             <itunes:duration>{}:{:02}:00</itunes:duration><itunes:episode>{}</itunes:episode>\
             <enclosure url=\"https://cdn.synthetic.example/media/{i}.mp3\" type=\"audio/mpeg\" length=\"{}\"/>\
             </item>",
            (i % 3) + 1,
            i % 60,
            i + 1,
            10_000_000 + i * 1000,
            mon = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"
            ][month - 1],
        );
    }
    s.push_str("</channel>\n</rss>\n");
    s.into_bytes()
}

/// A digest of every file in a tree, so "untouched" can be asserted.
pub fn tree_digest(root: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                entries.push((relative, std::fs::read(&path).unwrap()));
            }
        }
    }
    entries.sort();
    let mut hasher = Sha256::new();
    for (path, body) in entries {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(&body);
    }
    hex::encode(hasher.finalize())
}
