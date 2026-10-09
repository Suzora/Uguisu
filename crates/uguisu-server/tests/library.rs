//! Library and feed endpoints against a temporary data directory and a
//! wiremock feed host.

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
const V2: &str = include_str!("../../../tests/fixtures/feeds/parser/episodes_v2.xml");
const GUID_CHANGED: &str = include_str!("../../../tests/fixtures/feeds/parser/guid_changed.xml");

struct App {
    router: Router,
    server: MockServer,
    engine: Engine,
    _dir: tempfile::TempDir,
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
        _dir: dir,
    }
}

async fn serve(server: &MockServer, p: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(body),
        )
        .mount(server)
        .await;
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
#[allow(clippy::too_many_lines)] // one end-to-end walk through the API
async fn add_list_show_episodes_refresh_and_status() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let feed = format!("{}/feed.xml", t.server.uri());

    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": feed })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["created"], true);
    assert_eq!(json["podcast"]["title"], "Versioned Show");
    assert_eq!(json["report"]["outcome"], "fetched");
    assert_eq!(json["report"]["episodes"]["added"], 3);
    let id = json["podcast"]["id"].as_str().unwrap().to_owned();
    let source_id = json["source"]["id"].as_str().unwrap().to_owned();

    // Idempotent add answers 200 with created=false.
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": feed })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["created"], false);
    assert_eq!(json["podcast"]["id"], id);

    let (status, json) = call(&t.router, "GET", "/api/v1/podcasts", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["podcasts"].as_array().unwrap().len(), 1);
    assert_eq!(json["podcasts"][0]["episodes_total"], 3);

    let (status, json) = call(&t.router, "GET", &format!("/api/v1/podcasts/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["podcast"]["id"], id);
    assert_eq!(json["source"]["fetch"]["state"], "fetched");

    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{id}/episodes?limit=2"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["episodes"].as_array().unwrap().len(), 2);
    let after = json["next_after"].as_str().unwrap().to_owned();
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{id}/episodes?limit=2&after={after}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["episodes"].as_array().unwrap().len(), 1);
    assert!(json["next_after"].is_null());

    // Refresh: not modified, then forced with the new version.
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/refresh"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["outcome"], "not_modified");
    assert_eq!(json["reason"], "fingerprint");
    t.server.reset().await;
    serve(&t.server, "/feed.xml", V2).await;
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/refresh?force=true"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["outcome"], "fetched");
    assert_eq!(json["episodes"]["added"], 1);
    assert_eq!(json["episodes"]["updated"], 1);

    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/feeds/{source_id}/status"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["source"]["id"], source_id);
    assert_eq!(json["source"]["fetch"]["state"], "fetched");
    assert_eq!(json["last_fetch"]["outcome"], "fetched");

    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts/refresh?force=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["entries"].as_array().unwrap().len(), 1);
    assert_eq!(json["entries"][0]["report"]["episodes"]["unchanged"], 3);

    // Stored events are listable; the live stream is SSE.
    let (_, all) = call(&t.router, "GET", "/api/v1/events?limit=1000", None).await;
    let all = all["events"].as_array().unwrap();
    assert!(all.len() > 5, "{}", all.len());
    assert_eq!(all[0]["kind"], "podcast.added");
    let (status, json) = call(&t.router, "GET", "/api/v1/events?limit=5", None).await;
    assert_eq!(status, StatusCode::OK);
    let events = json["events"].as_array().unwrap();
    assert_eq!(
        events.as_slice(),
        &all[all.len() - 5..],
        "a limit alone answers the newest events, oldest first"
    );
    let first = all[0]["id"].as_str().unwrap();
    let (_, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/events?after={first}&limit=2"),
        None,
    )
    .await;
    assert_eq!(json["events"].as_array().unwrap().as_slice(), &all[1..3]);
    let resp = t
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    t.engine.close().await;
}

/// One request for everything a page about an episode needs: the episode with
/// its enclosures, its podcast's title, and whether it has been downloaded.
#[tokio::test]
async fn one_episode_answers_with_its_context() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let (_, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": format!("{}/feed.xml", t.server.uri()) })),
    )
    .await;
    let podcast = json["podcast"]["id"].as_str().unwrap().to_owned();
    let (_, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{podcast}/episodes"),
        None,
    )
    .await;
    let episode = json["episodes"][0]["id"].as_str().unwrap().to_owned();

    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/episodes/{episode}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["episode"]["id"], episode);
    assert_eq!(json["podcast_title"], "Versioned Show");
    assert!(
        json["episode"]["enclosures"]
            .as_array()
            .is_some_and(|e| !e.is_empty()),
        "the enclosures come with it, or a player has nothing to open: {json}"
    );
    // Nothing downloaded yet, and absent is `null` rather than missing.
    assert!(json["archive"].is_null(), "{json}");
    assert!(json["job"].is_null(), "{json}");

    let missing = uguisu_core::ids::EpisodeId::new();
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/episodes/{missing}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["kind"], "not_found");
    let (status, json) = call(&t.router, "GET", "/api/v1/episodes/nope", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["kind"], "invalid");
    t.engine.close().await;
}

#[tokio::test]
async fn duplicates_list_then_resolve() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let feed = format!("{}/feed.xml", t.server.uri());
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": feed })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let id = json["podcast"]["id"].as_str().unwrap().to_owned();
    t.server.reset().await;
    serve(&t.server, "/feed.xml", GUID_CHANGED).await;
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/refresh"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    let listing = format!("/api/v1/episodes/duplicates?podcast={id}");
    let (status, json) = call(&t.router, "GET", &listing, None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    let pairs = json["duplicates"].as_array().unwrap();
    assert_eq!(pairs.len(), 1, "{json}");
    let candidate = pairs[0]["candidate"]["id"].as_str().unwrap().to_owned();
    let original = pairs[0]["original"]["id"].as_str().unwrap().to_owned();
    assert!(
        pairs[0]["candidate"]["duplicate_reasons"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("same_enclosure_url")),
        "{json}"
    );

    let resolve = format!("/api/v1/episodes/{candidate}/resolve");
    let (status, json) = call(
        &t.router,
        "POST",
        &resolve,
        Some(serde_json::json!({ "resolution": "maybe" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{json}");
    assert_eq!(json["error"]["kind"], "invalid");
    let same = Some(serde_json::json!({ "resolution": "same" }));
    let (status, json) = call(&t.router, "POST", &resolve, same.clone()).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["resolution"], "same");
    assert_eq!(json["episode"]["id"], original);

    let (_, json) = call(&t.router, "GET", &listing, None).await;
    assert_eq!(json["duplicates"], serde_json::json!([]), "{json}");
    let (status, json) = call(&t.router, "POST", &resolve, same.clone()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/episodes/{original}/resolve"),
        same,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["kind"], "conflict");
}

#[tokio::test]
async fn errors_map_to_http_statuses() {
    let t = app().await;
    let (status, json) = call(&t.router, "GET", "/api/v1/podcasts/nope", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["kind"], "invalid");
    // An error is versioned like every other body (ADR 0038).
    assert_eq!(json["schema"], 1, "{json}");
    let missing = uguisu_core::ids::PodcastId::new();
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{missing}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["kind"], "not_found");
    let (status, _) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{missing}/refresh"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": "http://10.0.0.1/feed.xml" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["error"]["kind"], "blocked_by_policy");
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": format!("{}/missing.xml", t.server.uri()) })),
    )
    .await;
    assert!(
        status == StatusCode::BAD_GATEWAY || status == StatusCode::UNPROCESSABLE_ENTITY,
        "{status}: {json}"
    );
    // Conflict: a second feed with the same podcast:guid.
    let p20 = include_str!("../../../tests/fixtures/feeds/parser/podcasting20.xml");
    serve(&t.server, "/a.xml", p20).await;
    serve(&t.server, "/b.xml", p20).await;
    let (status, _) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": format!("{}/a.xml", t.server.uri()) })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": format!("{}/b.xml", t.server.uri()) })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["kind"], "conflict");
    t.engine.close().await;
}

#[tokio::test]
async fn inspect_needs_no_engine() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let feed = format!("{}/feed.xml", t.server.uri());
    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/feeds/inspect?url={}", urlencode(&feed)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["title"], "Versioned Show");
    assert_eq!(json["items"], 3);
    assert_eq!(json["looks_like_podcast"], true);
    assert_eq!(json["preview"].as_array().unwrap().len(), 3);
    let (status, json) = call(&t.router, "GET", "/api/v1/podcasts", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        json["podcasts"].as_array().unwrap().is_empty(),
        "nothing stored"
    );
    let (status, json) = call(&t.router, "GET", "/api/v1/feeds/inspect", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["kind"], "invalid");

    // A discovery-only server inspects too, but has no library.
    let bare = router(AppState::new(t.engine.discovery().clone(), None));
    let (status, json) = call(
        &bare,
        "GET",
        &format!("/api/v1/feeds/inspect?url={}", urlencode(&feed)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let (status, json) = call(&bare, "GET", "/api/v1/podcasts", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["error"]["kind"], "engine_unavailable");
    t.engine.close().await;
}

fn urlencode(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

fn opml(urls: &[String]) -> String {
    let outlines: String = urls
        .iter()
        .map(|u| format!(r#"<outline text="Feed" xmlUrl="{u}"/>"#))
        .collect::<Vec<_>>()
        .concat();
    format!(r#"<opml version="2.0"><body>{outlines}</body></opml>"#)
}

#[tokio::test]
async fn opml_export_is_an_attachment() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let feed = format!("{}/feed.xml", t.server.uri());
    let (status, _) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": feed })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let req = Request::builder()
        .uri("/api/v1/podcasts/opml")
        .body(Body::empty())
        .unwrap();
    let resp = t.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let headers = resp.headers().clone();
    assert_eq!(
        headers["content-type"].to_str().unwrap(),
        "text/x-opml; charset=utf-8"
    );
    assert_eq!(
        headers["content-disposition"].to_str().unwrap(),
        "attachment; filename=\"uguisu.opml\""
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.starts_with("<?xml"), "{text}");
    assert!(text.contains(&format!("xmlUrl=\"{feed}\"")), "{text}");
    assert!(text.contains("text=\"Versioned Show\""), "{text}");
}

#[tokio::test]
async fn opml_dry_run_adds_nothing() {
    let t = app().await;
    let feed = format!("{}/feed.xml", t.server.uri());
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts/opml",
        Some(serde_json::json!({ "opml": opml(&[feed.clone(), "feed://x/y".to_owned()]) })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["applied"], false);
    assert_eq!(json["counts"]["add"], 1);
    assert_eq!(json["counts"]["invalid"], 1);
    assert_eq!(json["items"][0]["xml_url"], feed);
    assert_eq!(json["items"][0]["action"], "add");
    assert_eq!(json["items"][1]["action"], "invalid");
    assert!(t.server.received_requests().await.unwrap().is_empty());
    assert!(t.engine.list_podcasts().await.unwrap().is_empty());
}

#[tokio::test]
async fn opml_apply_adds_with_policy() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let feed = format!("{}/feed.xml", t.server.uri());
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts/opml",
        Some(serde_json::json!({
            "opml": opml(&[feed]),
            "apply": true,
            "policy": { "mode": "auto", "max_backlog": 1 },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["applied"], true);
    assert_eq!(json["counts"]["add"], 1);
    let id = json["items"][0]["podcast_id"].as_str().unwrap().to_owned();

    let (status, json) = call(
        &t.router,
        "GET",
        &format!("/api/v1/podcasts/{id}/policy"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["stored"]["mode"], "auto", "{json}");
    assert_eq!(json["stored"]["max_backlog"], 1, "{json}");
}

#[tokio::test]
async fn unusable_opml_is_400_invalid() {
    let t = app().await;
    for document in ["<rss version=\"2.0\"/>", "", "plain text"] {
        let (status, json) = call(
            &t.router,
            "POST",
            "/api/v1/podcasts/opml",
            Some(serde_json::json!({ "opml": document, "apply": true })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{document:?}: {json}");
        assert_eq!(json["error"]["kind"], "invalid");
    }
}

#[tokio::test]
async fn unknown_policy_mode_is_refused() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts/opml",
        Some(serde_json::json!({
            "opml": opml(&[format!("{}/feed.xml", t.server.uri())]),
            "apply": true,
            "policy": { "mode": "sometimes" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["kind"], "policy_invalid");
    assert!(t.server.received_requests().await.unwrap().is_empty());
    assert!(t.engine.list_podcasts().await.unwrap().is_empty());
}

#[tokio::test]
async fn announced_feed_moves_with_force() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let feed = format!("{}/feed.xml", t.server.uri());
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": feed })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let id = json["podcast"]["id"].as_str().unwrap().to_owned();
    let renamed = format!("{}/renamed.xml", t.server.uri());
    t.server.reset().await;
    serve(
        &t.server,
        "/feed.xml",
        &V1.replacen(
            "<itunes:author>V Host</itunes:author>",
            &format!("<itunes:new-feed-url>{renamed}</itunes:new-feed-url>"),
            1,
        ),
    )
    .await;
    serve(
        &t.server,
        "/renamed.xml",
        &V1.replacen("<title>Versioned Show</title>", "<title>Renamed</title>", 1),
    )
    .await;
    let refresh = format!("/api/v1/podcasts/{id}/refresh?force=true");
    let (status, json) = call(&t.router, "POST", &refresh, None).await;
    assert_eq!(status, StatusCode::OK, "{json}");

    let show = format!("/api/v1/podcasts/{id}");
    let (_, json) = call(&t.router, "GET", &show, None).await;
    assert_eq!(json["announced"]["feed_url"], renamed, "{json}");
    assert_eq!(
        json["announced"]["fetch"]["last_error_kind"],
        "invalid_podcast_feed"
    );

    let mv = format!("/api/v1/podcasts/{id}/move-feed");
    let (status, json) = call(
        &t.router,
        "POST",
        &mv,
        Some(serde_json::json!({ "url": renamed })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["moved"], false);
    assert_eq!(json["verified"], false);
    let (status, json) = call(
        &t.router,
        "POST",
        &mv,
        Some(serde_json::json!({ "url": renamed, "force": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["moved"], true, "{json}");
    assert_eq!(json["report"]["outcome"], "fetched", "{json}");

    let (_, json) = call(&t.router, "GET", &show, None).await;
    assert_eq!(json["source"]["feed_url"], renamed, "{json}");
    assert_eq!(json["announced"], serde_json::Value::Null);

    for (url, want) in [
        ("file:///etc/passwd".to_owned(), StatusCode::BAD_REQUEST),
        (
            format!("{}/missing.xml", t.server.uri()),
            StatusCode::BAD_GATEWAY,
        ),
    ] {
        let (status, json) = call(
            &t.router,
            "POST",
            &mv,
            Some(serde_json::json!({ "url": url, "force": true })),
        )
        .await;
        assert_eq!(status, want, "{url}: {json}");
    }
}

#[tokio::test]
async fn archive_then_remove_a_podcast() {
    let t = app().await;
    serve(&t.server, "/feed.xml", V1).await;
    let (status, json) = call(
        &t.router,
        "POST",
        "/api/v1/podcasts",
        Some(serde_json::json!({ "input": format!("{}/feed.xml", t.server.uri()) })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let id = json["podcast"]["id"].as_str().unwrap().to_owned();
    let show = format!("/api/v1/podcasts/{id}");

    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/archive"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["status"], "archived");
    let (status, json) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/refresh"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    let (status, _) = call(
        &t.router,
        "POST",
        &format!("/api/v1/podcasts/{id}/pause"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // The CLI's client removes through the POST spelling.
    let (status, json) = call(&t.router, "POST", &format!("{show}/remove"), None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["podcast_id"], id.as_str());
    assert_eq!(json["episodes"], 3, "{json}");
    assert_eq!(json["files"], 0, "{json}");
    let (status, _) = call(&t.router, "GET", &show, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, json) = call(&t.router, "DELETE", &show, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert!(
        json["error"]["message"].as_str().unwrap().contains(&id),
        "{json}"
    );
}
