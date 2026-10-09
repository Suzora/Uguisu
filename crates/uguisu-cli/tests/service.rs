//! The service commands of the `uguisu` binary: the scheduler, a
//! podcast's place in it, persisted settings and local search.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const V1: &str = include_str!("../../../tests/fixtures/feeds/parser/episodes_v1.xml");

fn uguisu(server: &MockServer, data_dir: &Path, args: &[&str]) -> std::process::Output {
    uguisu_with(server, data_dir, &[], args)
}

fn uguisu_with(
    server: &MockServer,
    data_dir: &Path,
    env: &[(&str, &str)],
    args: &[&str],
) -> std::process::Output {
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
        .env("UGUISU_DATA_DIR", data_dir)
        .env("UGUISU_DISCOVERY_APPLE_BASE_URL", server.uri())
        .env("UGUISU_DISCOVERY_APPLE_ENABLED", "false")
        .env("UGUISU_HTTP_ALLOW_PRIVATE_HOSTS", "127.0.0.1")
        .env("UGUISU_FEED_REFRESH_TIMEOUT_MS", "5000")
        .env("UGUISU_LOG", "error")
        .envs(env.iter().copied())
        .output()
        .expect("run uguisu")
}

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn add_show(server: &MockServer, dir: &Path) -> String {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/rss+xml")
                .set_body_string(V1),
        )
        .mount(server)
        .await;
    let feed = format!("{}/feed.xml", server.uri());
    let out = uguisu(server, dir, &["--json", "podcast", "add", &feed, "--yes"]);
    assert!(out.status.success(), "{}", stdout(&out));
    json(&out)["podcast"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_scheduler_is_inspected_and_driven() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let id = add_show(&server, dir.path()).await;

    let out = uguisu(&server, dir.path(), &["--json", "scheduler", "status"]);
    assert!(out.status.success());
    let status = json(&out);
    assert_eq!(status["enabled"], true);
    assert_eq!(status["paused"], false);
    assert_eq!(
        status["running"], false,
        "a one-shot command starts no loop; only `serve` does"
    );

    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "scheduler", "pause", "--reason", "testing"],
    );
    assert_eq!(json(&out)["paused"], true);
    let out = uguisu(&server, dir.path(), &["scheduler", "status"]);
    assert!(
        stdout(&out).contains("paused (testing)"),
        "{}",
        stdout(&out)
    );

    // A pass while paused starts nothing, and says so.
    let out = uguisu(&server, dir.path(), &["--json", "scheduler", "run"]);
    assert_eq!(json(&out)["started"], 0);
    assert_eq!(json(&out)["paused"], true);

    let out = uguisu(&server, dir.path(), &["--json", "scheduler", "resume"]);
    assert_eq!(json(&out)["paused"], false);

    // Housekeeping runs on demand.
    let out = uguisu(&server, dir.path(), &["--json", "scheduler", "maintenance"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(json(&out)["events_pruned"].is_number());

    // One podcast out of the schedule and back.
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "pause", &id]);
    assert_eq!(json(&out)["status"], "paused");
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "resume", &id]);
    assert_eq!(json(&out)["status"], "active");
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "schedule", &id, "--at", "yesterday"],
    );
    assert_eq!(out.status.code(), Some(2), "a bad timestamp is usage");
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "schedule", &id]);
    assert!(out.status.success(), "{}", stdout(&out));
}

#[tokio::test]
async fn settings_are_listed_stored_and_cleared() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();

    let out = uguisu(&server, dir.path(), &["config", "list"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("UGUISU_FEED_REFRESH_CONCURRENCY"));

    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "config", "set", "UGUISU_ARCHIVE_MAX_BACKLOG", "9"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(json(&out)["origin"], "settings");

    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "config", "get", "UGUISU_ARCHIVE_MAX_BACKLOG"],
    );
    assert_eq!(json(&out)["value"], "9");

    // The environment wins and says so rather than accepting a write that
    // would do nothing.
    let out = uguisu(
        &server,
        dir.path(),
        &[
            "config",
            "set",
            "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS",
            "10.0.0.1",
        ],
    );
    assert!(!out.status.success());

    // `validate` passes while nothing is being ignored, and says which
    // keys the environment owns.
    let out = uguisu(&server, dir.path(), &["config", "validate"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("(environment only)"),
        "{}",
        stdout(&out)
    );

    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "config", "unset", "UGUISU_ARCHIVE_MAX_BACKLOG"],
    );
    assert_eq!(json(&out)["cleared"], true);
    let out = uguisu(&server, dir.path(), &["config", "get", "UGUISU_NOPE"]);
    assert_eq!(out.status.code(), Some(2));
}

#[tokio::test]
async fn validate_fails_on_pinned_value() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let out = uguisu(
        &server,
        dir.path(),
        &["config", "set", "UGUISU_ARCHIVE_MAX_BACKLOG", "9"],
    );
    assert!(out.status.success(), "{}", stdout(&out));

    let out = uguisu_with(
        &server,
        dir.path(),
        &[("UGUISU_ARCHIVE_MAX_BACKLOG", "4")],
        &["config", "validate"],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("pinned by the environment"),
        "{}",
        stdout(&out)
    );
}

#[tokio::test]
async fn the_library_is_searched_and_reindexed() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    add_show(&server, dir.path()).await;

    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "search", "library", "versioned"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
    let results = json(&out);
    assert_eq!(results["outcome"], "ok");
    assert_eq!(results["podcasts"][0]["title"], "Versioned Show");

    // Nothing found is exit 3, the discovery convention, whatever the
    // reason — and the reason is printed.
    let out = uguisu(
        &server,
        dir.path(),
        &["search", "library", "supercalifragilistic"],
    );
    assert_eq!(out.status.code(), Some(3));

    let out = uguisu(&server, dir.path(), &["search", "library", "***"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stdout(&out).contains("nothing to search for"));

    // Hostile input is a search, not a crash and not a usage error.
    // `--` because a leading dash is clap's business, not the query
    // parser's; everything after it is the query as typed.
    for hostile in ["\"unterminated", "a OR b", "NEAR(x y)", "-x", "--x"] {
        let out = uguisu(&server, dir.path(), &["search", "library", "--", hostile]);
        assert!(
            matches!(out.status.code(), Some(0 | 3)),
            "{hostile}: {:?} {}",
            out.status.code(),
            stdout(&out)
        );
    }

    let out = uguisu(&server, dir.path(), &["--json", "search", "reindex"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(json(&out)["episodes"].as_u64().unwrap() >= 3);

    let out = uguisu(
        &server,
        dir.path(),
        &["search", "library", "versioned", "--explain"],
    );
    assert!(stdout(&out).contains("text"), "{}", stdout(&out));
}

#[tokio::test]
async fn database_is_migrated_copied_checked() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let out = uguisu(&server, dir.path(), &["--json", "db", "migrate"]);
    assert!(out.status.success(), "{out:?}");
    let first = json(&out);
    assert_eq!(first["found"], serde_json::Value::Null, "{first}");
    assert!(!first["applied"].as_array().unwrap().is_empty(), "{first}");
    let out = uguisu(&server, dir.path(), &["db", "migrate"]);
    assert_eq!(
        stdout(&out),
        format!("Schema {}; nothing to apply\n", first["now"])
    );

    add_show(&server, dir.path()).await;
    let copy = dir.path().join("copy.db");
    let copy_arg = copy.to_str().unwrap();
    let out = uguisu(&server, dir.path(), &["--json", "db", "backup", copy_arg]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(json(&out)["path"], copy_arg);
    let out = uguisu(&server, dir.path(), &["db", "backup", copy_arg]);
    assert_eq!(out.status.code(), Some(1), "an existing file is refused");
    let out = uguisu(&server, dir.path(), &["--json", "db", "backup"]);
    assert!(out.status.success(), "{out:?}");
    let default = json(&out)["path"].as_str().unwrap().to_owned();
    assert!(
        Path::new(&default).starts_with(dir.path().join("backups")),
        "{default}"
    );

    // The copy is a data directory's database like any other.
    let restored = tempfile::tempdir().unwrap();
    std::fs::copy(&copy, restored.path().join("uguisu.db")).unwrap();
    let out = uguisu(&server, restored.path(), &["--json", "podcast", "list"]);
    assert_eq!(json(&out)["podcasts"].as_array().unwrap().len(), 1);

    let out = uguisu(&server, dir.path(), &["db", "check"]);
    assert_eq!(stdout(&out), "The database is sound\n");
    assert!(out.status.success());
    let out = uguisu(&server, dir.path(), &["--json", "db", "vacuum"]);
    assert!(out.status.success(), "{out:?}");
    assert!(json(&out)["bytes_after"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn remote_db_refuses_local_paths() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/db/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false,
            "integrity": [],
            "foreign_keys": ["archive_policies row 1 references a missing podcasts (constraint 0)"],
            "schema": 1,
        })))
        .mount(&api)
        .await;
    let uri = api.uri();
    let out = uguisu(&server, dir.path(), &["--server", &uri, "db", "check"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "foreign key: archive_policies row 1 references a missing podcasts (constraint 0)\n"
    );
    for args in [["db", "migrate"].as_slice(), &["db", "backup", "copy.db"]] {
        let out = uguisu(&server, dir.path(), &[&["--server", &uri], args].concat());
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    assert!(
        api.received_requests().await.unwrap().len() == 1,
        "only the check reached the server"
    );
}

/// `uguisu` with only what `health` needs: no data directory, no token.
fn health(env: &[(&str, &str)]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_uguisu"))
        .args(["--json", "health"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(
            ["SystemRoot", "TEMP", "TMP"]
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
        )
        .envs(env.iter().copied())
        .output()
        .expect("run uguisu health")
}

#[test]
fn health_answers_a_running_server() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    // What a container sets: the unspecified address, asked on loopback.
    let bind = format!("0.0.0.0:{port}");
    let down = health(&[("UGUISU_BIND", &bind)]);
    assert_eq!(down.status.code(), Some(1), "{}", stdout(&down));

    let dir = tempfile::tempdir().unwrap();
    let mut serve = Command::new(env!("CARGO_BIN_EXE_uguisu"))
        .args(["serve", "--bind", &format!("127.0.0.1:{port}")])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(
            ["SystemRoot", "TEMP", "TMP"]
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
        )
        .env("UGUISU_DATA_DIR", dir.path())
        .env("UGUISU_LOG", "error")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    let up = loop {
        let out = health(&[("UGUISU_BIND", &bind)]);
        if out.status.code() == Some(0) || start.elapsed() > std::time::Duration::from_secs(30) {
            break out;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    serve.kill().unwrap();
    serve.wait().unwrap();
    assert_eq!(up.status.code(), Some(0), "{}", stdout(&up));
    let body = json(&up);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["schema"], 1);
    assert!(body["version"].is_string(), "{body}");
}
