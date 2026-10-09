//! Serving archived bytes: media with `Range`, artwork, and the refusals
//! that keep the route from reading anything else.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uguisu_core::config::{Config, DataConfig};
use uguisu_download::testing::MediaServer;
use uguisu_engine::{Engine, EngineConfig};
use uguisu_server::{AppState, router};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct App {
    router: Router,
    server: MockServer,
    media: MediaServer,
    engine: Engine,
    dir: tempfile::TempDir,
}

impl App {
    fn media_root(&self) -> PathBuf {
        self.dir.path().join("media")
    }
}

async fn app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let media = MediaServer::start().await;
    let mut config = Config::from_lookup(|k| match k {
        "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS" => Some("127.0.0.1".into()),
        "UGUISU_DISCOVERY_APPLE_ENABLED" => Some("false".into()),
        "UGUISU_DOWNLOAD_MIN_FREE_BYTES" => Some("0".into()),
        _ => None,
    })
    .unwrap();
    config.data = DataConfig {
        data_dir: Some(dir.path().to_path_buf()),
        media_dir: None,
    };
    let engine = Engine::open(EngineConfig::from(config)).await.unwrap();
    let state = AppState::new(engine.discovery().clone(), Some(engine.clone()));
    App {
        router: router(state),
        server,
        media,
        engine,
        dir,
    }
}

struct Raw {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Raw {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

async fn get(app: &Router, uri: &str, headers: &[(&str, &str)]) -> Raw {
    let mut req = Request::builder().method("GET").uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Raw {
        status,
        headers,
        body,
    }
}

async fn json(
    app: &Router,
    method_name: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut req = Request::builder().method(method_name).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn feed(base: &str, cover: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><rss version=\"2.0\" \
         xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\"><channel>\
         <title>Media Show</title><link>https://media.example/</link><description>d</description>\
         <itunes:image href=\"{cover}\"/>\
         <item><title>Item 0</title><guid isPermaLink=\"false\">m-0</guid>\
         <pubDate>01 Jan 2024 10:00:00 +0000</pubDate>\
         <enclosure url=\"{base}/normal/2048\" type=\"audio/mpeg\" length=\"2048\"/></item>\
         </channel></rss>"
    )
}

fn tiny_png() -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend_from_slice(&[0, 0, 0, 13]);
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]);
    out.extend_from_slice(&[0x3A, 0x7E, 0x9B, 0x55]);
    out.extend_from_slice(&[0, 0, 0, 0x0A]);
    out.extend_from_slice(b"IDAT");
    out.extend_from_slice(&[0x78, 0x9C, 0x63, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01]);
    out.extend_from_slice(&[0x0D, 0x0A, 0x2D, 0xB4]);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
    out
}

/// One archived episode, plus the podcast it belongs to.
async fn archived(t: &App) -> (String, String) {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(feed(
                    t.media.base(),
                    &format!("{}/cover.png", t.server.uri()),
                )),
        )
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(tiny_png()),
        )
        .mount(&t.server)
        .await;

    let added = json(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": format!("{}/feed.xml", t.server.uri()) })),
    )
    .await;
    let podcast = added["podcast"]["id"].as_str().unwrap().to_owned();
    json(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{podcast}/downloads"),
        Some(serde_json::json!({ "priority": "normal" })),
    )
    .await;
    t.engine.start_downloads();
    t.engine.downloads().wait_idle().await.unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let stats = json(&t.router, "GET", "/api/v1/archive/stats", None).await;
        if stats["total"].as_u64() == Some(1) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{stats}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let page = json(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{podcast}/episodes"),
        None,
    )
    .await;
    let episode = page["episodes"][0]["id"].as_str().unwrap().to_owned();
    (podcast, episode)
}

async fn record(t: &App, episode: &str) -> serde_json::Value {
    json(
        &t.router,
        "GET",
        &format!("/api/v1/archive/{episode}"),
        None,
    )
    .await
}

/// Swaps `at` for a symlink to `target`, keeping the bytes elsewhere.
#[cfg(unix)]
fn relink(at: &std::path::Path, target: &std::path::Path) {
    let stash = at.with_extension("stashed");
    std::fs::rename(at, &stash).unwrap();
    std::os::unix::fs::symlink(target, at).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn media_serves_the_file_and_validator() {
    let t = app().await;
    let (_, episode) = archived(&t).await;
    let file = record(&t, &episode).await;
    let size = file["size_bytes"].as_u64().unwrap();
    let hash = file["hash_value"].as_str().unwrap();
    let on_disk =
        std::fs::read(t.media_root().join(file["relative_path"].as_str().unwrap())).unwrap();

    let r = get(&t.router, &format!("/api/v1/archive/{episode}/media"), &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body.len() as u64, size);
    assert_eq!(r.body, on_disk);
    assert_eq!(r.header("content-type"), Some("audio/mpeg"));
    assert_eq!(r.header("accept-ranges"), Some("bytes"));
    assert_eq!(r.header("content-length"), Some(size.to_string().as_str()));
    assert_eq!(r.header("etag"), Some(format!("\"{hash}\"").as_str()));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_range_returns_exactly_those_bytes() {
    let t = app().await;
    let (_, episode) = archived(&t).await;
    let file = record(&t, &episode).await;
    let size = file["size_bytes"].as_u64().unwrap();
    let on_disk =
        std::fs::read(t.media_root().join(file["relative_path"].as_str().unwrap())).unwrap();
    let uri = format!("/api/v1/archive/{episode}/media");

    let r = get(&t.router, &uri, &[("range", "bytes=100-199")]).await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.body, on_disk[100..200]);
    assert_eq!(
        r.header("content-range"),
        Some(format!("bytes 100-199/{size}").as_str())
    );
    assert_eq!(r.header("content-length"), Some("100"));

    let tail = get(&t.router, &uri, &[("range", "bytes=-64")]).await;
    assert_eq!(tail.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(tail.body, on_disk[on_disk.len() - 64..]);

    let rest = get(&t.router, &uri, &[("range", "bytes=2000-")]).await;
    assert_eq!(rest.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(rest.body, on_disk[2000..]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_range_past_the_end_is_refused() {
    let t = app().await;
    let (_, episode) = archived(&t).await;
    let size = record(&t, &episode).await["size_bytes"].as_u64().unwrap();

    let r = get(
        &t.router,
        &format!("/api/v1/archive/{episode}/media"),
        &[("range", "bytes=99999-")],
    )
    .await;
    assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        r.header("content-range"),
        Some(format!("bytes */{size}").as_str())
    );
    assert!(r.body.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_matching_validator_saves_the_body() {
    let t = app().await;
    let (_, episode) = archived(&t).await;
    let hash = record(&t, &episode).await["hash_value"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = format!("/api/v1/archive/{episode}/media");

    let fresh = get(
        &t.router,
        &uri,
        &[("if-none-match", &format!("\"{hash}\""))],
    )
    .await;
    assert_eq!(fresh.status, StatusCode::NOT_MODIFIED);
    assert!(fresh.body.is_empty());

    // A stale `If-Range` must produce the whole file, never a splice of two
    // versions.
    let stale = get(
        &t.router,
        &uri,
        &[("range", "bytes=0-9"), ("if-range", "\"gone\"")],
    )
    .await;
    assert_eq!(stale.status, StatusCode::OK);
    assert_eq!(stale.body.len(), 2048);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_symlink_replaces_no_artifact() {
    let t = app().await;
    let (_, episode) = archived(&t).await;
    let relative = record(&t, &episode).await["relative_path"]
        .as_str()
        .unwrap()
        .to_owned();
    // Inside the media root, so only the refusal of a non-regular file can
    // stop it: a symlink is not the artifact, whatever it points at.
    let decoy = t.media_root().join("decoy.mp3");
    std::fs::write(&decoy, b"not yours").unwrap();
    relink(&t.media_root().join(&relative), &decoy);

    let r = get(&t.router, &format!("/api/v1/archive/{episode}/media"), &[]).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let body = String::from_utf8_lossy(&r.body);
    assert!(!body.contains("not yours"), "{body}");
    let parsed: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(parsed["error"]["kind"], "archive_invalid");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_symlink_leaving_the_root_fails() {
    let t = app().await;
    let (_, episode) = archived(&t).await;
    let relative = record(&t, &episode).await["relative_path"]
        .as_str()
        .unwrap()
        .to_owned();
    let directory = relative.split('/').next().unwrap().to_owned();
    let escape = t.dir.path().join("escape");
    std::fs::create_dir_all(&escape).unwrap();
    relink(&t.media_root().join(&directory), &escape);

    let r = get(&t.router, &format!("/api/v1/archive/{episode}/media"), &[]).await;
    let body = String::from_utf8_lossy(&r.body);
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        !body.contains(&escape.display().to_string()),
        "the response named a path outside the archive: {body}"
    );
    let parsed: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(parsed["error"]["kind"], "path_invalid");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bytes_gone_but_record_present() {
    let t = app().await;
    let (_, episode) = archived(&t).await;
    let relative = record(&t, &episode).await["relative_path"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::rename(
        t.media_root().join(&relative),
        t.dir.path().join("moved-away"),
    )
    .unwrap();

    let r = get(&t.router, &format!("/api/v1/archive/{episode}/media"), &[]).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let parsed: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(parsed["error"]["kind"], "archive_missing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_episode_has_no_media() {
    let t = app().await;
    let r = get(
        &t.router,
        "/api/v1/archive/01ARZ3NDEKTSV4RRFFQ69G5FAV/media",
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let parsed: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(parsed["error"]["kind"], "archive_not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artwork_bytes_come_from_the_store() {
    let t = app().await;
    let (podcast, _) = archived(&t).await;
    let fetched = json(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{podcast}/artwork/fetch"),
        None,
    )
    .await;
    assert_eq!(fetched["state"], "fetched", "{fetched}");
    let hash = fetched["artwork"]["hash_value"]
        .as_str()
        .unwrap()
        .to_owned();

    let r = get(
        &t.router,
        &format!("/api/v1/podcasts/{podcast}/artwork/image"),
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, tiny_png());
    assert_eq!(r.header("content-type"), Some("image/png"));
    assert_eq!(r.header("etag"), Some(format!("\"{hash}\"").as_str()));

    let cached = get(
        &t.router,
        &format!("/api/v1/podcasts/{podcast}/artwork/image"),
        &[("if-none-match", &format!("\"{hash}\""))],
    )
    .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artwork_that_was_never_fetched_is_absent() {
    let t = app().await;
    let (podcast, _) = archived(&t).await;
    let r = get(
        &t.router,
        &format!("/api/v1/podcasts/{podcast}/artwork/image"),
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(body["error"]["kind"], "artwork_not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bad_id_never_reaches_the_disk() {
    let t = app().await;
    for uri in [
        "/api/v1/archive/..%2F..%2Fetc%2Fpasswd/media",
        "/api/v1/archive/nope/media",
        "/api/v1/podcasts/nope/artwork/image",
    ] {
        let r = get(&t.router, uri, &[]).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{uri}");
        let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(body["error"]["kind"], "invalid", "{uri}");
    }
}
