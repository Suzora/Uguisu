//! Sidecars, manifests, rebuild, import, artwork
//! and tags, over the real router and a temporary data directory.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
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

async fn app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let media = MediaServer::start().await;
    let mut config = Config::from_lookup(|k| match k {
        "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS" => Some("127.0.0.1".into()),
        "UGUISU_DISCOVERY_APPLE_ENABLED" => Some("false".into()),
        "UGUISU_DOWNLOAD_PROGRESS_INTERVAL_MS" => Some("20".into()),
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

fn feed(base: &str, cover: &str, scenarios: &[&str]) -> String {
    use std::fmt::Write as _;
    let mut s = format!(
        "<?xml version=\"1.0\"?><rss version=\"2.0\" \
         xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\"><channel>\
         <title>Media Show</title><link>https://media.example/</link><description>d</description>\
         <itunes:image href=\"{cover}\"/>"
    );
    for (i, scenario) in scenarios.iter().enumerate() {
        let _ = write!(
            s,
            "<item><title>Item {i}</title><guid isPermaLink=\"false\">m-{i}</guid>\
             <pubDate>{:02} Jan 2024 10:00:00 +0000</pubDate>\
             <enclosure url=\"{base}{scenario}\" type=\"audio/mpeg\" length=\"1000\"/></item>",
            i + 1
        );
    }
    s.push_str("</channel></rss>");
    s
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn archived(t: &App, scenarios: &[&str]) -> (String, Vec<String>) {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(feed(
                    t.media.base(),
                    &format!("{}/cover.png", t.server.uri()),
                    scenarios,
                )),
        )
        .mount(&t.server)
        .await;
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": format!("{}/feed.xml", t.server.uri()) })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let podcast = json["podcast"]["id"].as_str().unwrap().to_owned();

    call(
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
        let (_, json) = call(&t.router, "GET", "/api/v1/archive/stats", None).await;
        if json["total"].as_u64() == Some(scenarios.len() as u64) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{json}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (_, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{podcast}/episodes"),
        None,
    )
    .await;
    let episodes = json["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    (podcast, episodes)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sidecars_and_manifests_are_readable_and_writable_over_the_api() {
    let t = app().await;
    let (podcast, episodes) = archived(&t, &["/normal/2048", "/normal/4096"]).await;

    // The sidecar exists because the download wrote it.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (status, _) = call(
            &t.router,
            "GET",
            &format!("/api/v1/archive/{}/sidecar", episodes[0]),
            None,
        )
        .await;
        if status == StatusCode::OK {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no sidecar appeared");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/archive/{}/sidecar", episodes[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["episode"]["id"], episodes[0]);
    assert!(json["archive"]["hash_value"].is_string());

    // Writing it again is idempotent and reports where it went.
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/archive/{}/sidecar/write", episodes[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(
        std::path::Path::new(json["path"].as_str().unwrap())
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("json")
    );

    // The manifest is stale until it is written, and then it is not.
    let (_, json) = call(&t.router, "GET", "/api/v1/archive/manifests", None).await;
    assert_eq!(json["manifests"][0]["stale"], true);
    let (status, json) = call(&t.router, "POST", "/api/v1/archive/manifests/write", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["written"][0]["entries"], 2);
    assert_eq!(json["written"][0]["cleared"], true);

    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/archive/manifests/{podcast}/verify?rehash=true"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["clean"], true, "{json}");
    assert_eq!(json["unchanged"], 2);
    assert_eq!(json["rehashed"], true);
    assert_eq!(json["schema"], 1);

    // A literal route is not swallowed by the `{episode_id}` one.
    let (status, json) = call(&t.router, "GET", "/api/v1/archive/manifests", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["manifests"].is_array());
    t.engine.close().await;
    drop(t.dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rebuild_and_import_default_to_a_dry_run() {
    let t = app().await;
    archived(&t, &["/normal/2048"]).await;

    let (status, json) = call(&t.router, "POST", "/api/v1/archive/rebuild", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["applied"], false, "a rebuild does not write unasked");
    assert_eq!(json["schema"], 1);
    assert!(json["conflicts"]["count"].is_number());

    // An import of an empty directory: a plan with nothing in it, not an
    // error, and still explicitly a dry run.
    let empty = tempfile::tempdir().unwrap();
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/archive/import",
        Some(serde_json::json!({ "path": empty.path().to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["applied"], false);
    assert_eq!(json["scanned"], 0);
    assert_eq!(json["imported"], 0);
    assert_eq!(json["items"].as_array().unwrap().len(), 0);

    // A format nobody implements is refused by name.
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/archive/import",
        Some(serde_json::json!({
            "path": empty.path().to_string_lossy(),
            "format": "gpodder"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["kind"], "invalid");

    // And the media directory cannot be imported into itself.
    let media = t.engine.config().data.media_dir().unwrap();
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/archive/import",
        Some(serde_json::json!({ "path": media.to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    t.engine.close().await;
    drop(t.dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artwork_and_tags_report_what_they_did() {
    let t = app().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tiny_png())
                .insert_header("content-type", "image/png"),
        )
        .mount(&t.server)
        .await;
    let (podcast, episodes) = archived(&t, &["/normal/2048"]).await;

    // Nothing is fetched until it is asked for.
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{podcast}/artwork"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["current"].is_null(), "artwork fetching is opt-in");

    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{podcast}/artwork/fetch"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["state"], "fetched");
    assert_eq!(json["artwork"]["format"], "png");
    assert_eq!(json["artwork"]["is_current"], true);

    let (_, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{podcast}/artwork"),
        None,
    )
    .await;
    assert_eq!(json["current"]["format"], "png");
    assert_eq!(json["history"].as_array().unwrap().len(), 0);

    // The scenario media server serves bytes that are not an audio
    // container, so tagging refuses them - and says so as a result rather
    // than as a server error.
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/archive/{}/tags/write", episodes[0]),
        Some(serde_json::json!({ "mode": "sync" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a file that is not a container Uguisu tags is a fact about the \
         file, not a server fault: {json}"
    );
    assert_eq!(json["error"]["kind"], "tags_unsupported", "{json}");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tags_unsupported"),
        "{json}"
    );

    // An unknown mode is refused before anything is opened.
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/archive/{}/tags/write", episodes[0]),
        Some(serde_json::json!({ "mode": "overwrite" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["kind"], "invalid");
    t.engine.close().await;
    drop(t.dir);
}

#[tokio::test]
async fn import_rejects_unreadable_podgrab_db() {
    let t = app().await;
    let source = tempfile::tempdir().unwrap();
    let not_a_database = source.path().join("podgrab.db");
    std::fs::write(&not_a_database, b"just text").unwrap();
    for db in [
        not_a_database.to_string_lossy().into_owned(),
        source
            .path()
            .join("missing.db")
            .to_string_lossy()
            .into_owned(),
        source.path().to_string_lossy().into_owned(),
    ] {
        let (status, json) = call(
            &t.router,
            "POST",
            "/api/v1/archive/import",
            Some(serde_json::json!({ "path": source.path().to_string_lossy(), "podgrab_db": db })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{db}: {json}");
        assert_eq!(
            json["error"]["kind"], "import_source_invalid",
            "{db}: {json}"
        );
    }
}

#[tokio::test]
async fn orphans_are_reported_not_removed() {
    let t = app().await;
    let media = t.engine.config().data.media_dir().unwrap();
    let leftover = media.join(".uguisu/tmp/01J0000000000000000000000A.import");
    let unknown = media.join("Foreign/song.mp3");
    for file in [&leftover, &unknown] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, b"left behind").unwrap();
    }
    let (status, json) = call(&t.router, "GET", "/api/v1/archive/orphans", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["clean"], false);
    assert_eq!(
        json["leftovers"]["sample"],
        serde_json::json!([".uguisu/tmp/01J0000000000000000000000A.import"])
    );
    assert_eq!(json["unknown_media"]["count"], 1);
    assert!(
        leftover.is_file() && unknown.is_file(),
        "nothing is removed"
    );
}
