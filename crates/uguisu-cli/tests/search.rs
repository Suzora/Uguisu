//! End-to-end tests of the `uguisu` binary against wiremock-backed providers.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

use uguisu_discovery::testing::Fixture;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FEED: &str = include_str!("../../../tests/fixtures/feeds/probe/rss_itunes_podcast.xml");

fn uguisu(server: &MockServer, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_uguisu"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        // Windows sockets cannot start without SystemRoot (os error 10106), and
        // SQLite and temp_dir() fall back to C:\Windows without TEMP/TMP.
        .envs(
            ["SystemRoot", "TEMP", "TMP"]
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
        )
        .env("UGUISU_DISCOVERY_APPLE_BASE_URL", server.uri())
        .env("UGUISU_DISCOVERY_GPODDERNET_BASE_URL", server.uri())
        .env("UGUISU_DISCOVERY_GPODDERNET_ENABLED", "true")
        .env("UGUISU_HTTP_ALLOW_PRIVATE_HOSTS", "127.0.0.1")
        .env("UGUISU_DISCOVERY_SOFT_DEADLINE_MS", "200")
        .env("UGUISU_DISCOVERY_HARD_DEADLINE_MS", "3000")
        .env("UGUISU_LOG", "error")
        .output()
        .expect("run uguisu")
}

async fn server_with_fixtures() -> MockServer {
    let server = MockServer::start().await;
    for case in ["search_darknet", "search_empty", "error_403"] {
        Fixture::load("apple", case).unwrap().mount(&server).await;
    }
    Fixture::load("gpoddernet", "search_empty")
        .unwrap()
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(FEED),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/private"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "http://10.0.0.1/feed"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn search_json_and_text() {
    let server = server_with_fixtures().await;
    let out = uguisu(&server, &["--json", "search", "podcast", "darknet diaries"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["outcome"], "results");
    assert_eq!(json["results"][0]["candidate"]["title"], "Darknet Diaries");
    assert_eq!(json["results"][0]["rank"], 1);
    assert_eq!(
        json["results"][0]["explanation"]["signals"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert!(
        json["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["provider"] == "apple" && p["status"] == "ok")
    );
    assert!(
        json["attribution"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a.as_str().unwrap().contains("Apple"))
    );

    let out = uguisu(
        &server,
        &["search", "podcast", "darknet diaries", "--explain"],
    );
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1. Darknet Diaries"), "{text}");
    assert!(
        text.contains("Feed: https://feeds.megaphone.fm/darknetdiaries"),
        "{text}"
    );
    assert!(text.contains("exact_title"), "{text}");
    assert!(text.contains("provider apple"), "{text}");
}

#[tokio::test]
async fn exit_codes_distinguish_the_outcomes() {
    let server = server_with_fixtures().await;
    let out = uguisu(&server, &["--json", "search", "podcast", "zzqqxxnothing"]);
    assert_eq!(out.status.code(), Some(3));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["outcome"], "no_results");

    let out = uguisu(
        &server,
        &[
            "--json",
            "search",
            "podcast",
            "ratelimited",
            "--provider",
            "apple",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["outcome"], "all_providers_failed");
    assert_eq!(json["providers"][0]["status"], "failed");
    assert_eq!(json["providers"][0]["error_kind"], "rate_limited");

    let out = uguisu(
        &server,
        &["search", "podcast", "x", "--provider", "spotify"],
    );
    assert_eq!(out.status.code(), Some(2));
}

#[tokio::test]
async fn resolve_direct_feed_url_and_policy_block() {
    let server = server_with_fixtures().await;
    let feed = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, &["--json", "resolve", &feed]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["title"], "Example Diaries");
    assert_eq!(json["items_with_media"], 3);
    assert!(
        json["provenance"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["kind"] == "validate" && s["ok"] == true)
    );

    // A URL given to `search podcast` is resolved, not searched.
    let out = uguisu(&server, &["search", "podcast", &feed]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Title: Example Diaries"));

    let blocked = format!("{}/private", server.uri());
    let out = uguisu(&server, &["--json", "resolve", &blocked]);
    assert_eq!(
        out.status.code(),
        Some(6),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["kind"], "blocked_by_policy");

    let out = uguisu(&server, &["resolve", "not a url"]);
    assert_eq!(out.status.code(), Some(2));
}

#[tokio::test]
async fn search_with_resolve_flag_verifies_top_result() {
    let server = server_with_fixtures().await;
    // The Apple fixture's feed URL points at megaphone (unreachable here), so rewrite the fixture to the mock feed.
    let mut fixture = Fixture::load("apple", "search_darknet").unwrap();
    let body = fixture.response.body.to_string().replace(
        "https://feeds.megaphone.fm/darknetdiaries",
        &format!("{}/feed.xml", server.uri()),
    );
    fixture.response.body = serde_json::from_str(&body).unwrap();
    fixture
        .request
        .query
        .insert("term".into(), "darknet diaries resolve".into());
    fixture.mount(&server).await;
    let out = uguisu(
        &server,
        &[
            "--json",
            "search",
            "podcast",
            "darknet diaries resolve",
            "--provider",
            "apple",
            "--resolve",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["search"]["outcome"], "results");
    assert_eq!(json["resolved"]["title"], "Example Diaries");
}

#[tokio::test]
async fn text_names_the_relaxed_query() {
    let server = MockServer::start().await;
    for case in ["search_misspelled", "search_partial"] {
        Fixture::load_live("gpoddernet", case)
            .unwrap()
            .mount(&server)
            .await;
    }
    // gpodder.net only: Apple's 20/min throttle would hold its relaxed calls past the deadline.
    let out = uguisu(
        &server,
        &[
            "search",
            "podcast",
            "darknet diariez",
            "--provider",
            "gpoddernet",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with(
            "No results for `darknet diariez`; showing results for `darknet`.
1. "
        ),
        "{text}"
    );
}
