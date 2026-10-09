//! Archive and policy endpoints against a temporary data directory, a
//! wiremock feed host and the scenario media server.

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

fn feed(base: &str, scenarios: &[&str]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Media Show</title>\
         <link>https://media.example/</link><description>d</description>",
    );
    for (i, scenario) in scenarios.iter().enumerate() {
        use std::fmt::Write as _;
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

/// Adds a podcast, downloads everything and waits for the archive records.
async fn archived(t: &App, scenarios: &[&str]) -> (String, Vec<String>) {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(feed(t.media.base(), scenarios)),
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

    let (status, _) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{podcast}/downloads"),
        Some(serde_json::json!({ "priority": "normal" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
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

#[tokio::test]
async fn list_show_verify_and_report_over_http() {
    let t = app().await;
    let (podcast, episodes) = archived(&t, &["/range/5000", "/range/6000", "/range/7000"]).await;

    let (status, json) = call(&t.router, "GET", "/api/v1/archive", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["schema"], 1);
    assert_eq!(json["files"].as_array().unwrap().len(), 3);

    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/archive?podcast={podcast}&limit=2"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["files"].as_array().unwrap().len(), 2);

    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/archive/{}", episodes[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["hash_algo"], "sha256");
    assert_eq!(json["verification_state"], "verified");
    let path = json["relative_path"].as_str().unwrap().to_owned();
    assert!(path.starts_with("Media Show/"), "{path}");

    // Bad input and an episode without an artifact are told apart.
    let (status, json) = call(&t.router, "GET", "/api/v1/archive/not-an-id", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    let (status, json) = call(
        &t.router,
        "GET",
        "/api/v1/archive/01ARZ3NDEKTSV4RRFFQ69G5FAV",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["kind"], "archive_not_found");

    // A bulk verification reports, and a full one hashes.
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/archive/verify",
        Some(serde_json::json!({ "depth": "full" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["checked"], 3);
    assert_eq!(json["verified"], 3);
    assert_eq!(json["depth"], "full");

    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/archive/verify",
        Some(serde_json::json!({ "depth": "sideways" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");

    // Tampering is reported, not repaired.
    let media_dir = t.engine.config().data.media_dir().unwrap();
    let full = media_dir.join(&path);
    let original = std::fs::read(&full).unwrap();
    std::fs::write(&full, b"short").unwrap();
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/archive/{}/verify", episodes[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["state"], "invalid");
    assert_eq!(json["reason"], "size_mismatch");
    assert_eq!(std::fs::read(&full).unwrap(), b"short", "nothing rewritten");

    let (status, json) = call(&t.router, "GET", "/api/v1/archive/invalid", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["files"].as_array().unwrap().len(), 1);
    let (_, json) = call(&t.router, "GET", "/api/v1/archive/missing", None).await;
    assert!(json["files"].as_array().unwrap().is_empty());

    let (_, json) = call(&t.router, "GET", "/api/v1/archive/stats", None).await;
    assert_eq!(json["total"], 3);
    assert_eq!(json["by_state"]["invalid"], 1);
    assert_eq!(json["by_state"]["verified"], 2);

    // Putting the bytes back makes it verify again; the record never moved.
    std::fs::write(&full, &original).unwrap();
    let (_, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/archive/{}/verify", episodes[0]),
        Some(serde_json::json!({ "depth": "full" })),
    )
    .await;
    assert_eq!(json["state"], "verified");
    assert_eq!(json["file"]["relative_path"], path);
    t.engine.close().await;
    drop(t.dir);
}

#[tokio::test]
async fn preview_and_relocate_over_http() {
    let t = app().await;
    let (_, episodes) = archived(&t, &["/range/5000"]).await;
    let media_dir = t.engine.config().data.media_dir().unwrap();

    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/archive/{}/path-preview", episodes[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let resolved = json["resolved"].as_str().unwrap().to_owned();
    assert!(resolved.starts_with("Media Show/"), "{resolved}");
    assert_eq!(json["would_move"], false, "already on the template path");
    assert_eq!(json["current"], resolved);

    // A dry run answers without touching anything.
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/archive/{}/relocate", episodes[0]),
        Some(serde_json::json!({ "dry_run": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["moved"], false);
    assert!(media_dir.join(&resolved).is_file());

    // Reconciliation is idempotent and reports.
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/archive/reconcile?deep=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["registered"], 0, "nothing was missing a record");
    assert_eq!(json["checked"], 1);
    assert_eq!(json["missing"], 0);
    t.engine.close().await;
    drop(t.dir);
}

#[tokio::test]
async fn policies_round_trip_over_http() {
    let t = app().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(feed(t.media.base(), &["/range/5000", "/range/6000"])),
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

    // The default installation archives nothing on its own.
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{podcast}/policy"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["stored"].is_null());
    assert_eq!(json["effective"]["mode"], "manual");
    let (_, json) = call(&t.router, "GET", "/api/v1/downloads/stats", None).await;
    assert_eq!(json["by_state"].as_object().unwrap().len(), 0, "{json}");

    // Opting this podcast in, overriding only the backlog.
    let (status, json) = call(
        &t.router,
        "PUT",
        &format!("/api/v1/podcasts/{podcast}/policy"),
        Some(serde_json::json!({ "mode": "auto", "max_backlog": 1, "priority": "high" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["stored"]["mode"], "auto");
    assert_eq!(json["effective"]["mode"], "auto");
    assert_eq!(json["effective"]["max_backlog"], 1);
    assert_eq!(json["effective"]["priority"], "high");
    assert!(
        json["stored"]["max_age_days"].is_null(),
        "an unset field stays unset: {json}"
    );
    assert_eq!(
        json["effective"]["max_age_days"], 0,
        "and falls back to the global default"
    );

    let (status, json) = call(&t.router, "GET", "/api/v1/archive/policies", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["policies"].as_array().unwrap().len(), 1);

    let (status, json) = call(
        &t.router,
        "PUT",
        &format!("/api/v1/podcasts/{podcast}/policy"),
        Some(serde_json::json!({ "mode": "sideways" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["kind"], "policy_invalid");

    let (status, json) = call(
        &t.router,
        "DELETE",
        &format!("/api/v1/podcasts/{podcast}/policy"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["cleared"], true);
    let (_, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{podcast}/policy"),
        None,
    )
    .await;
    assert_eq!(json["effective"]["mode"], "manual", "back to the default");
    t.engine.close().await;
    drop(t.dir);
}
