//! Log output of the `uguisu` binary: the JSON format, and URL redaction.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::{Command, Output};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FEED: &str = include_str!("../../../tests/fixtures/feeds/parser/episodes_v1.xml");

/// Runs `uguisu` with JSON logs at `info` and nothing from the environment
/// but what a process needs to start.
fn uguisu(dir: &Path, args: &[&str]) -> Output {
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
        .env("UGUISU_DATA_DIR", dir)
        .env("UGUISU_DISCOVERY_APPLE_ENABLED", "false")
        .env("UGUISU_HTTP_ALLOW_PRIVATE_HOSTS", "127.0.0.1")
        .env("UGUISU_LOG", "info,uguisu_http=trace,uguisu_discovery=debug")
        .env("UGUISU_LOG_FORMAT", "json")
        .output()
        .expect("run uguisu")
}

fn json_lines(stderr: &str) -> Vec<serde_json::Value> {
    stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

async fn serve_feed(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(FEED),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn json_logs_without_url_secrets() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve_feed(&server).await;
    let url = format!("{}/feed.xml?token=SEKRIT", server.uri());

    let out = uguisu(dir.path(), &["--json", "podcast", "add", &url]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");

    let lines = json_lines(&stderr);
    assert!(!stderr.contains("SEKRIT"), "{stderr}");
    let redacted = serde_json::Value::from(format!("{}/feed.xml?[redacted]", server.uri()));
    assert!(
        lines
            .iter()
            .filter_map(|l| l["fields"].as_object())
            .any(|fields| fields.values().any(|v| *v == redacted)),
        "no log line names the feed: {stderr}"
    );
}

#[test]
fn search_query_url_redacted() {
    let dir = tempfile::tempdir().unwrap();
    // A term query (it has a space) that quotes a private feed.
    let out = uguisu(
        dir.path(),
        &[
            "--json",
            "search",
            "podcast",
            "darknet https://h.example/f?token=SEKRIT",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);

    let lines = json_lines(&stderr);
    assert!(
        lines
            .iter()
            .any(|l| l["fields"]["message"] == "search started"),
        "the search is logged: {stderr}"
    );
    assert!(!stderr.contains("SEKRIT"), "{stderr}");
}

#[tokio::test]
async fn failed_refresh_logs_no_secret() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve_feed(&server).await;
    let url = format!("{}/feed.xml?token=SEKRIT", server.uri());
    let out = uguisu(dir.path(), &["--json", "podcast", "add", &url]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let added: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = added["podcast"]["id"].as_str().unwrap().to_owned();

    // The failure's detail quotes the error body, and this one echoes the URL.
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(503).set_body_string(format!("upstream {url} failed")))
        .mount(&server)
        .await;
    let out = uguisu(dir.path(), &["--json", "podcast", "refresh", &id]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    let lines = json_lines(&stderr);
    assert!(
        lines
            .iter()
            .any(|l| l["fields"]["message"] == "refresh failed"),
        "the failure is logged: {stderr}"
    );
    assert!(!stderr.contains("SEKRIT"), "{stderr}");
}
