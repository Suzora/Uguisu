//! Service endpoints: status, scheduler, settings, search and
//! recorded resolutions.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uguisu_core::config::{Config, DataConfig};
use uguisu_engine::{Engine, EngineConfig};
use uguisu_server::{AppState, router};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const V1: &str = include_str!("../../../tests/fixtures/feeds/parser/episodes_v1.xml");

struct App {
    router: Router,
    server: MockServer,
    engine: Engine,
    dir: tempfile::TempDir,
}

async fn app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let mut config = Config::from_lookup(|k| match k {
        "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS" => Some("127.0.0.1".into()),
        "UGUISU_DISCOVERY_APPLE_ENABLED" => Some("false".into()),
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
        engine,
        dir,
    }
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

async fn add_podcast(t: &App) -> String {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(V1),
        )
        .mount(&t.server)
        .await;
    let feed = format!("{}/feed.xml", t.server.uri());
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": feed })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    json["podcast"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn status_and_scheduler_report_the_daemon() {
    let t = app().await;
    let id = add_podcast(&t).await;

    let (status, json) = call(&t.router, "GET", "/api/v1/status", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["podcasts"], 1);
    assert_eq!(json["scheduler"]["enabled"], true);
    assert_eq!(
        json["scheduler"]["running"], false,
        "a test server starts no loop; only `serve` does"
    );
    assert_eq!(json["search"]["state"], "stale");
    assert_eq!(json["settings_problems"], 0);

    // Pause persists and says why; resume clears it.
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/scheduler/pause",
        Some(serde_json::json!({ "reason": "maintenance window" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["paused"], true);
    assert_eq!(json["paused_reason"], "maintenance window");

    let (_, json) = call(&t.router, "GET", "/api/v1/scheduler", None).await;
    assert_eq!(json["paused"], true);
    let (status, json) = call(&t.router, "POST", "/api/v1/scheduler/run", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{json}");
    assert_eq!(json["paused"], true);
    assert_eq!(json["started"], 0, "a paused scheduler starts nothing");

    let (_, json) = call(&t.router, "POST", "/api/v1/scheduler/resume", None).await;
    assert_eq!(json["paused"], false);

    // One podcast in and out of the schedule.
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/pause"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["status"], "paused");
    let (_, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/resume"),
        None,
    )
    .await;
    assert_eq!(json["status"], "active");

    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/schedule"),
        Some(serde_json::json!({ "at": "not a timestamp" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    let (status, _) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/schedule"),
        Some(serde_json::json!({ "at": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = call(&t.router, "POST", "/api/v1/scheduler/maintenance", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["events_pruned"].is_number());
}

#[tokio::test]
async fn settings_read_write_and_refuse() {
    let t = app().await;

    let (status, json) = call(&t.router, "GET", "/api/v1/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let keys = json["keys"].as_array().unwrap();
    assert!(keys.len() > 40);
    assert!(json["rejected"].as_array().unwrap().is_empty());

    let (status, json) = call(
        &t.router,
        "PUT",
        "/api/v1/settings/UGUISU_ARCHIVE_MAX_BACKLOG",
        Some(serde_json::json!({ "value": "9" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["value"], "9");
    assert_eq!(json["origin"], "settings");
    assert_eq!(t.engine.config().archive.max_backlog, 9);

    // The environment pins the allowlist in this test's configuration,
    // and it is not storable in any case: 400 before 409.
    let (status, json) = call(
        &t.router,
        "PUT",
        "/api/v1/settings/UGUISU_HTTP_ALLOW_PRIVATE_HOSTS",
        Some(serde_json::json!({ "value": "10.0.0.1" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["kind"], "invalid");

    // A key the environment sets is a conflict, not a silent no-op.
    let (status, json) = call(
        &t.router,
        "PUT",
        "/api/v1/settings/UGUISU_DISCOVERY_APPLE_ENABLED",
        Some(serde_json::json!({ "value": "true" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["kind"], "conflict");

    let (status, json) = call(
        &t.router,
        "PUT",
        "/api/v1/settings/UGUISU_ARCHIVE_MAX_BACKLOG",
        Some(serde_json::json!({ "value": "loads" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");

    let (status, json) = call(
        &t.router,
        "DELETE",
        "/api/v1/settings/UGUISU_ARCHIVE_MAX_BACKLOG",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["cleared"], true);
    assert_eq!(t.engine.config().archive.max_backlog, 3);
}

#[tokio::test]
async fn search_answers_and_reindexes() {
    let t = app().await;
    add_podcast(&t).await;

    let (status, json) = call(&t.router, "GET", "/api/v1/search?q=versioned", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["outcome"], "ok");
    assert_eq!(json["podcasts"][0]["title"], "Versioned Show");

    // Nothing to search for is an answer, not an error.
    let (status, json) = call(&t.router, "GET", "/api/v1/search?q=***", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["outcome"], "empty_query");

    // Hostile input is a search.
    let (status, json) = call(
        &t.router,
        "GET",
        "/api/v1/search?q=%22unterminated%20OR%20x*",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["outcome"].is_string());

    let (status, json) = call(&t.router, "POST", "/api/v1/search/reindex", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["episodes"].as_u64().unwrap() >= 3);
    let (_, json) = call(&t.router, "GET", "/api/v1/status", None).await;
    assert_eq!(json["search"]["state"], "ready");
}

#[tokio::test]
async fn recorded_resolutions_are_listed_and_fetched() {
    let t = app().await;
    let id = add_podcast(&t).await;

    let (status, json) = call(&t.router, "GET", "/api/v1/discovery/records", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let records = json["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["status"], "resolved");
    assert_eq!(records[0]["podcast_id"], id);

    let record_id = records[0]["id"].as_str().unwrap();
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/discovery/records/{record_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["id"], record_id);

    let (status, json) = call(
        &t.router,
        "GET",
        "/api/v1/discovery/records/not-an-id",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
}

#[tokio::test]
async fn database_maintenance_answers() {
    let t = app().await;
    let (status, json) = call(&t.router, "POST", "/api/v1/db/backup", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    let path = std::path::PathBuf::from(json["path"].as_str().unwrap());
    assert_eq!(
        path.parent().unwrap(),
        t.dir.path().join("backups"),
        "{json}"
    );
    assert_eq!(
        json["bytes"].as_u64().unwrap(),
        std::fs::metadata(&path).unwrap().len()
    );
    let (status, json) = call(&t.router, "POST", "/api/v1/db/check", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["ok"], true, "{json}");
    let (status, json) = call(&t.router, "POST", "/api/v1/db/vacuum", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["bytes_after"].as_u64().unwrap() > 0, "{json}");
}
