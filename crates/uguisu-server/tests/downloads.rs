//! Download queue endpoints against a temporary data directory, a
//! wiremock feed host and the scenario media server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uguisu_core::config::{Config, DataConfig};
use uguisu_download::testing::{MediaServer, content_sha256};
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

/// A three-item feed whose enclosures live on the media server.
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

async fn add_podcast(t: &App, scenarios: &[&str]) -> (String, Vec<String>) {
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
    let id = json["podcast"]["id"].as_str().unwrap().to_owned();
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{id}/episodes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let episodes = json["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    (id, episodes)
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

#[tokio::test]
async fn enqueue_list_show_commands_control_and_reconcile() {
    let t = app().await;
    let (podcast, episodes) = add_podcast(&t, &["/range/5000", "/range/6000", "/range/7000"]).await;

    // Single enqueue: 201, then 200 existing; bad ids and unknown episodes.
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/downloads",
        Some(serde_json::json!({ "episode_id": episodes[0], "priority": "high" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["outcome"], "created");
    assert_eq!(json["job"]["state"], "queued");
    assert_eq!(json["job"]["priority"], "high");
    let job = json["job"]["id"].as_str().unwrap().to_owned();
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/downloads",
        Some(serde_json::json!({ "episode_id": episodes[0] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["outcome"], "existing");
    assert_eq!(json["job"]["id"], job);
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/downloads",
        Some(serde_json::json!({ "episode_id": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["kind"], "invalid");
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/downloads",
        Some(serde_json::json!({ "episode_id": uguisu_core::ids::EpisodeId::new().to_string() })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["kind"], "not_found");

    // Bulk enqueue skips the one that already has a job.
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{podcast}/downloads"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["created"], 2);
    assert_eq!(json["existing"], 1);
    assert_eq!(json["skipped"].as_array().unwrap().len(), 0);

    // Listing, filters and paging.
    let (status, json) = call(&t.router, "GET", "/api/v1/downloads", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 3);
    assert!(json["next_after"].is_null());
    // A queue row names what it is downloading, so a reader does not have to
    // fetch the episode list to render one.
    let row = &json["jobs"][0];
    assert!(
        row["episode_title"].as_str().is_some_and(|t| !t.is_empty()),
        "{row}"
    );
    assert!(
        row["podcast_title"].as_str().is_some_and(|t| !t.is_empty()),
        "{row}"
    );
    // And the job's own fields are still where they were: the summary is
    // flattened, so nothing that read `DownloadJob` has to change.
    assert!(row["id"].as_str().is_some(), "{row}");
    assert!(row["state"].as_str().is_some(), "{row}");
    assert!(row["target_path"].as_str().is_some(), "{row}");
    let (status, json) = call(&t.router, "GET", "/api/v1/downloads?limit=2", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 2);
    let after = json["next_after"].as_str().unwrap().to_owned();
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/downloads?limit=2&after={after}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 1);
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/downloads?state=queued&podcast={podcast}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 3);
    let (status, json) = call(&t.router, "GET", "/api/v1/downloads?state=completed", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 0);
    let (status, json) = call(&t.router, "GET", "/api/v1/downloads?state=bogus", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("queued")
    );

    // Stats: nothing runs because nobody started workers.
    let (status, json) = call(&t.router, "GET", "/api/v1/downloads/stats", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["by_state"]["queued"], 3);
    assert_eq!(json["running"], 0);
    assert_eq!(json["workers_started"], false);
    assert!(json["paused_all"].is_null());

    // Show and job commands.
    let (status, json) = call(&t.router, "GET", &format!("/api/v1/downloads/{job}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["job"]["id"], job);
    assert_eq!(json["attempts"].as_array().unwrap().len(), 0);
    assert!(json["progress"].is_null());
    let (status, _) = call(&t.router, "GET", "/api/v1/downloads/nope", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &t.router,
        "GET",
        &format!("/api/v1/downloads/{}", uguisu_core::ids::JobId::new()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/downloads/{job}/pause"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["job"]["state"], "paused");
    assert_eq!(json["job"]["state_reason"], "user");
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/downloads/{job}/pause"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["kind"], "conflict");
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/downloads/{job}/resume"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["job"]["state"], "queued");
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/downloads/{job}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["job"]["state"], "cancelled");
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/downloads/{job}/retry"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["job"]["state"], "queued");
    assert_eq!(json["job"]["state_reason"], "requeued");
    let (status, json) = call(&t.router, "POST", "/api/v1/downloads/retry-failed", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["requeued"], 0);

    // Global pause persists and shows in stats.
    let (status, json) = call(&t.router, "POST", "/api/v1/downloads/pause", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["control"]["paused"], true);
    assert_eq!(json["control"]["paused_reason"], "user");
    let (_, json) = call(&t.router, "GET", "/api/v1/downloads/stats", None).await;
    assert_eq!(json["paused_all"], "user");
    let (status, json) = call(&t.router, "POST", "/api/v1/downloads/resume", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["control"]["paused"], false);

    // Reconcile reports and never deletes.
    let tmp = t
        .dir
        .path()
        .join("media")
        .join(&podcast)
        .join(".uguisu-tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let orphan = tmp.join(format!("{}.part", uguisu_core::ids::JobId::new()));
    std::fs::write(&orphan, b"stale").unwrap();
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/downloads/reconcile?deep=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["recovered"], 0);
    assert_eq!(json["orphan_parts"].as_array().unwrap().len(), 1);
    assert_eq!(json["missing_targets"], 0);
    assert!(orphan.exists());
    t.engine.close().await;
}

/// Reads SSE frames until `until` shows up (or the deadline passes) and
/// returns everything read so far.
async fn read_sse_until(body: &mut Body, until: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut text = String::new();
    while !text.contains(until) {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(left, body.frame())
            .await
            .expect("sse deadline")
            .expect("stream open")
            .unwrap();
        if let Some(data) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    text
}

#[tokio::test]
async fn workers_download_and_events_exclude() {
    let t = app().await;
    let (_podcast, episodes) = add_podcast(&t, &["/slow/300000/600000"]).await;
    let mut plain = t
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .into_body();
    let mut filtered = t
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/events?exclude=download.progress,podcast.added")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .into_body();

    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/downloads",
        Some(serde_json::json!({ "episode_id": episodes[0] })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let job = json["job"]["id"].as_str().unwrap().to_owned();
    t.engine.start_downloads();
    t.engine.downloads().wait_idle().await.unwrap();

    let (status, json) = call(&t.router, "GET", &format!("/api/v1/downloads/{job}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["job"]["state"], "completed", "{json}");
    assert_eq!(json["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(json["attempts"][0]["outcome"], "completed");
    let target = t
        .dir
        .path()
        .join("media")
        .join(json["job"]["target_path"].as_str().unwrap());
    assert_eq!(
        hex::encode(Sha256::digest(std::fs::read(&target).unwrap())),
        content_sha256(300_000)
    );
    let (_, json) = call(&t.router, "GET", "/api/v1/downloads/stats", None).await;
    assert_eq!(json["by_state"]["completed"], 1);
    assert_eq!(json["workers_started"], true);

    let all = read_sse_until(&mut plain, "event: download.completed").await;
    assert!(all.contains("event: download.queued"), "{all}");
    assert!(all.contains("event: download.started"), "{all}");
    assert!(all.contains("event: download.progress"), "{all}");
    let some = read_sse_until(&mut filtered, "event: download.completed").await;
    assert!(some.contains("event: download.started"), "{some}");
    assert!(!some.contains("event: download.progress"), "{some}");
    assert!(!some.contains("event: podcast.added"), "{some}");
    t.engine.close().await;
}
