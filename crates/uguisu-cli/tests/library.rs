//! End-to-end tests of the library commands of the `uguisu` binary with a
//! temporary data directory and a wiremock feed host.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const V1: &str = include_str!("../../../tests/fixtures/feeds/parser/episodes_v1.xml");
const V2: &str = include_str!("../../../tests/fixtures/feeds/parser/episodes_v2.xml");
const MALFORMED: &str = include_str!("../../../tests/fixtures/feeds/parser/malformed_item.xml");
const GUID_CHANGED: &str = include_str!("../../../tests/fixtures/feeds/parser/guid_changed.xml");

fn uguisu(server: &MockServer, data_dir: &Path, args: &[&str]) -> std::process::Output {
    command(server, data_dir, args)
        .output()
        .expect("run uguisu")
}

fn command(server: &MockServer, data_dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_uguisu"));
    command
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
        .env("UGUISU_LOG", "error");
    command
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

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn serve(server: &MockServer, p: &str, template: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(template)
        .mount(server)
        .await;
}

fn feed(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/rss+xml")
        .set_body_string(body)
}

#[tokio::test]
async fn podcast_and_feed_commands() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());

    let out = uguisu(&server, dir.path(), &["--json", "podcast", "add", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let added = json(&out);
    assert_eq!(added["schema"], 1);
    assert_eq!(added["created"], true);
    assert_eq!(added["report"]["outcome"], "fetched");
    assert_eq!(added["report"]["episodes"]["added"], 3);
    let id = added["podcast"]["id"].as_str().unwrap().to_owned();
    let source_id = added["source"]["id"].as_str().unwrap().to_owned();
    assert!(dir.path().join("uguisu.db").exists());

    // Text output and idempotency.
    let out = uguisu(&server, dir.path(), &["podcast", "add", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("Already in the library"),
        "{}",
        stdout(&out)
    );

    let out = uguisu(&server, dir.path(), &["podcast", "list"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("Versioned Show"), "{text}");
    assert!(text.contains(&id), "{text}");
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "list"]);
    assert_eq!(json(&out)["podcasts"][0]["episodes_total"], 3);

    let out = uguisu(&server, dir.path(), &["podcast", "show", &id]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("Episodes:    3"), "{text}");
    assert!(text.contains(&source_id), "{text}");

    // Not modified (fingerprint) → exit 0.
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "refresh", &id]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["outcome"], "not_modified");
    let out = uguisu(&server, dir.path(), &["podcast", "refresh", &id]);
    assert!(stdout(&out).contains("not modified"), "{}", stdout(&out));

    // Forced with a new version → fetched.
    server.reset().await;
    serve(&server, "/feed.xml", feed(V2)).await;
    let out = uguisu(&server, dir.path(), &["podcast", "refresh", &id, "--force"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("1 added"), "{text}");
    assert!(text.contains("1 updated"), "{text}");

    let out = uguisu(&server, dir.path(), &["feed", "status", &source_id]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("state fetched"), "{text}");
    assert!(text.contains("Last fetch:"), "{text}");
    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "feed", "status", &source_id],
    );
    assert_eq!(json(&out)["source"]["fetch"]["state"], "fetched");

    let out = uguisu(
        &server,
        dir.path(),
        &["feed", "refresh", &source_id, "--force"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    // --all aggregates; one podcast, fetched.
    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "podcast", "refresh", "--all", "--force"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["entries"].as_array().unwrap().len(), 1);
    let out = uguisu(&server, dir.path(), &["podcast", "refresh", "--all"]);
    assert!(
        stdout(&out).contains("1 of 1 podcast(s) refreshed"),
        "{}",
        stdout(&out)
    );
}

#[tokio::test]
async fn exit_codes_reflect_feed_and_network_failures() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "add", &url]);
    let id = json(&out)["podcast"]["id"].as_str().unwrap().to_owned();

    server.reset().await;
    serve(&server, "/feed.xml", ResponseTemplate::new(404)).await;
    let out = uguisu(&server, dir.path(), &["podcast", "refresh", &id]);
    assert_eq!(out.status.code(), Some(8), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("failed (not_found)"),
        "{}",
        stdout(&out)
    );

    server.reset().await;
    serve(&server, "/feed.xml", feed("<?xml version=\"1.0\"?><rss><channel><title>x</title><item><title>a</title><enclosure url=\"https://cdn.example/a.mp3\" type=\"audio/mpeg\"/></item><item><ti")).await;
    let out = uguisu(&server, dir.path(), &["podcast", "refresh", &id, "--force"]);
    assert_eq!(
        out.status.code(),
        Some(9),
        "truncated is partial: {}",
        stdout(&out)
    );

    server.reset().await;
    serve(&server, "/feed.xml", feed(MALFORMED)).await;
    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "podcast", "refresh", &id, "--force"],
    );
    assert_eq!(
        out.status.code(),
        Some(9),
        "malformed item is partial: {}",
        stdout(&out)
    );

    server.reset().await;
    serve(&server, "/feed.xml", feed("<html><body>gone</body></html>")).await;
    let out = uguisu(&server, dir.path(), &["podcast", "refresh", &id, "--force"]);
    assert_eq!(out.status.code(), Some(7), "{}", stdout(&out));

    let out = uguisu(&server, dir.path(), &["podcast", "refresh", "--all"]);
    assert_eq!(
        out.status.code(),
        Some(7),
        "worst code wins: {}",
        stdout(&out)
    );

    let out = uguisu(&server, dir.path(), &["podcast", "show", "not-an-id"]);
    assert_eq!(out.status.code(), Some(2));
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "show", "01ARZ3NDEKTSV4RRFFQ69G5FAV"],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("not found"));
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "add", "http://10.0.0.1/feed.xml"],
    );
    assert_eq!(out.status.code(), Some(6), "{}", stderr(&out));
}

#[tokio::test]
async fn feed_inspect_never_touches_the_data_directory() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, dir.path(), &["--json", "feed", "inspect", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let j = json(&out);
    assert_eq!(j["schema"], 1);
    assert_eq!(j["title"], "Versioned Show");
    assert_eq!(j["items"], 3);
    assert_eq!(j["identity_sources"]["guid"], 3);
    assert!(
        !dir.path().join("uguisu.db").exists(),
        "inspect must not create the database"
    );
    assert!(
        !dir.path().join("uguisu.lock").exists(),
        "inspect must not take the lock"
    );
    assert!(
        !dir.path().join("uguisu.pid").exists(),
        "inspect must not record a holder"
    );
    let out = uguisu(&server, dir.path(), &["feed", "inspect", &url]);
    let text = stdout(&out);
    assert!(text.contains("Items:       3"), "{text}");
    assert!(text.contains("Episode One"), "{text}");

    serve(&server, "/bad.xml", feed(MALFORMED)).await;
    let out = uguisu(
        &server,
        dir.path(),
        &["feed", "inspect", &format!("{}/bad.xml", server.uri())],
    );
    assert_eq!(out.status.code(), Some(9), "{}", stdout(&out));
    let out = uguisu(&server, dir.path(), &["feed", "inspect", "not a url"]);
    assert_eq!(out.status.code(), Some(2));
    let out = uguisu(
        &server,
        dir.path(),
        &["feed", "inspect", &format!("{}/missing.xml", server.uri())],
    );
    assert_eq!(out.status.code(), Some(8), "{}", stderr(&out));
}

#[tokio::test]
async fn website_needs_confirmation_lock_is_reported() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    serve(
        &server,
        "/",
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/html")
            .set_body_string(format!(
                "<html><head><link rel=\"alternate\" type=\"application/rss+xml\" href=\"{}/feed.xml\"></head><body>site</body></html>",
                server.uri()
            )),
    )
    .await;
    let site = format!("{}/", server.uri());
    let out = uguisu(&server, dir.path(), &["podcast", "add", &site]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    assert!(stdout(&out).contains("Versioned Show"), "{}", stdout(&out));
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "list"]);
    assert!(json(&out)["podcasts"].as_array().unwrap().is_empty());
    let out = uguisu(&server, dir.path(), &["podcast", "add", "--yes", &site]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("Added Versioned Show"),
        "{}",
        stdout(&out)
    );

    // Another process holds the lock: stateful commands refuse with a hint.
    let lock = uguisu_engine::LockFile::acquire(&dir.path().join("uguisu.lock")).unwrap();
    let out = uguisu(&server, dir.path(), &["podcast", "list"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("--server"), "{}", stderr(&out));
    let out = uguisu(
        &server,
        dir.path(),
        &[
            "--json",
            "feed",
            "inspect",
            &format!("{}/feed.xml", server.uri()),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "inspect needs no lock: {}",
        stderr(&out)
    );
    drop(lock);
}

/// Over the API the podcast list is paged (ADR 0040): the client follows
/// the cursor until the last page, rather than showing the first.
#[tokio::test]
async fn remote_list_follows_the_cursor() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "add", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "list"]);
    let detail = json(&out)["podcasts"][0].clone();
    let cursor = detail["podcast"]["id"].as_str().unwrap().to_owned();

    let api = MockServer::start().await;
    let page = |next: Option<&str>| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "podcasts": [detail.clone()],
            "next_after": next,
            "schema": 1,
        }))
    };
    Mock::given(method("GET"))
        .and(path("/api/v1/podcasts"))
        .and(query_param("after", cursor.as_str()))
        .respond_with(page(None))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/podcasts"))
        .and(query_param_is_missing("after"))
        .respond_with(page(Some(&cursor)))
        .mount(&api)
        .await;
    let out = uguisu(
        &server,
        dir.path(),
        &["--server", &api.uri(), "--json", "podcast", "list"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        json(&out)["podcasts"].as_array().unwrap().len(),
        2,
        "{}",
        stdout(&out)
    );
}

fn opml(urls: &[&str]) -> String {
    let outlines: Vec<String> = urls
        .iter()
        .map(|u| format!(r#"<outline text="Feed" xmlUrl="{u}"/>"#))
        .collect();
    format!(
        r#"<?xml version="1.0"?><opml version="2.0"><body>{}</body></opml>"#,
        outlines.concat()
    )
}

fn opml_file(dir: &Path, urls: &[&str]) -> String {
    let file = dir.join("subscriptions.opml");
    std::fs::write(&file, opml(urls)).unwrap();
    file.display().to_string()
}

#[tokio::test]
async fn import_dry_run_adds_nothing() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let url = format!("{}/feed.xml", server.uri());
    let file = opml_file(files.path(), &[&url, "feed://a.example/rss"]);

    let out = uguisu(&server, dir.path(), &["podcast", "import", &file]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Would add 1 of 2 feeds (dry run)\n  invalid: 1\n  invalid: feed://a.example/rss \
         (feed: is not http or https)\nRun again with --apply to add them.\n"
    );

    let out = uguisu(&server, dir.path(), &["--json", "podcast", "import", &file]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["schema"], 1);
    assert_eq!(report["applied"], false);
    assert_eq!(report["counts"]["add"], 1);
    assert_eq!(report["items"][0]["action"], "add");
    assert!(server.received_requests().await.unwrap().is_empty());
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "list"]);
    assert_eq!(json(&out)["podcasts"], serde_json::json!([]));
}

#[tokio::test]
async fn import_apply_adds_and_reports() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let missing = format!("{}/missing.xml", server.uri());
    let file = opml_file(files.path(), &[&url, &missing]);

    let out = uguisu(
        &server,
        dir.path(),
        &[
            "podcast",
            "import",
            &file,
            "--apply",
            "--mode",
            "auto",
            "--max-backlog",
            "2",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("Added 1 of 2 feeds\n  failed: 1\n"),
        "{text}"
    );
    assert!(text.contains(&format!("  failed: {missing} (")), "{text}");
    assert!(
        text.ends_with("`uguisu podcast refresh --all` fetches them now.\n"),
        "{text}"
    );

    let out = uguisu(&server, dir.path(), &["--json", "podcast", "list"]);
    let listed = json(&out);
    assert_eq!(listed["podcasts"].as_array().unwrap().len(), 1);
    assert_eq!(listed["podcasts"][0]["source"]["provider"], "opml");
    assert_eq!(listed["podcasts"][0]["episodes_total"], 0);
    let id = listed["podcasts"][0]["podcast"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "archive", "policy", "show", &id],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let policy = json(&out);
    assert_eq!(policy["stored"]["mode"], "auto", "{policy}");
    assert_eq!(policy["stored"]["max_backlog"], 2, "{policy}");
}

#[tokio::test]
async fn import_reads_standard_input() {
    use std::io::Write as _;
    use std::process::Stdio;

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/feed.xml", server.uri());
    let mut child = command(&server, dir.path(), &["--json", "podcast", "import", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(opml(&[&url]).as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["items"][0]["xml_url"], url);
}

#[tokio::test]
async fn policy_flags_need_a_mode() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let file = opml_file(files.path(), &["https://a.example/feed"]);
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "import", &file, "--max-backlog", "2"],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "import", &file, "--mode", "sometimes"],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("`sometimes` is not a policy mode"),
        "{}",
        stderr(&out)
    );
    let out = uguisu(
        &server,
        dir.path(),
        &[
            "podcast",
            "import",
            &file,
            "--mode",
            "auto",
            "--priority",
            "urgent",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[tokio::test]
async fn unparsable_file_exits_two() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let rss = files.path().join("feed.xml");
    std::fs::write(&rss, V1).unwrap();
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "import", &rss.display().to_string()],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("not an OPML file"),
        "{}",
        stderr(&out)
    );

    let big = files.path().join("big.opml");
    std::fs::write(&big, " ".repeat(uguisu_engine::opml::MAX_BYTES + 1)).unwrap();
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "import", &big.display().to_string()],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("the most an import reads"),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn missing_file_exits_one() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nowhere.opml");
    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "import", &missing.display().to_string()],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
}

#[tokio::test]
async fn export_prints_the_library() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "add", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let out = uguisu(&server, dir.path(), &["podcast", "export"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let document = stdout(&out);
    assert!(document.starts_with("<?xml"), "{document}");
    assert!(
        document.contains(&format!("xmlUrl=\"{url}\"")),
        "{document}"
    );

    let out = uguisu(&server, dir.path(), &["--json", "podcast", "export"]);
    let wrapped = json(&out);
    assert_eq!(wrapped["schema"], 1);
    assert_eq!(wrapped["opml"], document);
}

#[tokio::test]
async fn remote_import_posts_file_text() {
    use wiremock::matchers::body_partial_json;

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let document = opml(&["https://a.example/feed"]);
    std::fs::write(files.path().join("s.opml"), &document).unwrap();
    let file = files.path().join("s.opml").display().to_string();

    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/podcasts/opml"))
        .and(body_partial_json(serde_json::json!({
            "opml": document,
            "apply": true,
            "policy": { "mode": "manual", "priority": "low" },
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "schema": 1,
            "applied": true,
            "counts": {
                "add": 1, "already_present": 0, "duplicate": 0, "invalid": 0,
                "needs_review": 0, "conflict": 0, "failed": 0,
            },
            "items": [{
                "title": "Feed", "xml_url": "https://a.example/feed", "action": "add",
                "podcast_id": null, "detail": null,
            }],
        })))
        .expect(1)
        .mount(&api)
        .await;
    let out = uguisu(
        &server,
        dir.path(),
        &[
            "--server",
            &api.uri(),
            "--json",
            "podcast",
            "import",
            &file,
            "--apply",
            "--mode",
            "manual",
            "--priority",
            "low",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["counts"]["add"], 1);
    assert!(
        !dir.path().join("uguisu.db").exists(),
        "nothing opened locally"
    );
}

#[tokio::test]
async fn remote_export_prints_server_text() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let api = MockServer::start().await;
    let document = opml(&["https://a.example/feed"]);
    Mock::given(method("GET"))
        .and(path("/api/v1/podcasts/opml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/x-opml; charset=utf-8")
                .set_body_string(document.clone()),
        )
        .mount(&api)
        .await;
    let out = uguisu(
        &server,
        dir.path(),
        &["--server", &api.uri(), "podcast", "export"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), document);
}

/// Adds `episodes_v1.xml`, then refreshes `guid_changed.xml`, which brings
/// Episode One back under a new GUID: one candidate duplicate.
async fn with_candidate(server: &MockServer, dir: &Path) -> serde_json::Value {
    serve(server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(server, dir, &["--json", "podcast", "add", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let id = json(&out)["podcast"]["id"].as_str().unwrap().to_owned();
    server.reset().await;
    serve(server, "/feed.xml", feed(GUID_CHANGED)).await;
    let out = uguisu(server, dir, &["podcast", "refresh", &id]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = uguisu(
        server,
        dir,
        &["--json", "episode", "duplicates", "--podcast", &id],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let listed = json(&out);
    assert_eq!(listed["schema"], 1);
    assert_eq!(
        listed["duplicates"].as_array().unwrap().len(),
        1,
        "{listed}"
    );
    listed["duplicates"][0].clone()
}

#[tokio::test]
async fn duplicates_list_and_resolve() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let pair = with_candidate(&server, dir.path()).await;
    let candidate = pair["candidate"]["id"].as_str().unwrap().to_owned();
    let original = pair["original"]["id"].as_str().unwrap().to_owned();

    let out = uguisu(&server, dir.path(), &["episode", "duplicates"]);
    let text = stdout(&out);
    assert!(
        text.contains(&format!("probably the same as {original}")),
        "{text}"
    );
    assert!(text.contains("same enclosure url"), "{text}");

    let out = uguisu(
        &server,
        dir.path(),
        &["episode", "resolve", &original, "--same"],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "not a candidate: {}",
        stderr(&out)
    );
    let out = uguisu(
        &server,
        dir.path(),
        &["episode", "resolve", &candidate, "--same"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("Merged episode {candidate} into {original}\n")
    );
    let out = uguisu(
        &server,
        dir.path(),
        &["episode", "resolve", &candidate, "--separate"],
    );
    assert_eq!(out.status.code(), Some(2), "gone: {}", stderr(&out));
    let out = uguisu(&server, dir.path(), &["episode", "duplicates"]);
    assert_eq!(stdout(&out), "No candidate duplicates\n");
}

#[tokio::test]
async fn resolve_needs_one_resolution() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["episode", "resolve", "01J"][..],
        &["episode", "resolve", "01J", "--same", "--separate"],
        &["episode", "resolve", "not-an-id", "--same"],
    ] {
        let out = uguisu(&server, dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
}

#[tokio::test]
async fn remote_duplicates_follow_the_cursor() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let pair = with_candidate(&server, dir.path()).await;
    let cursor = pair["candidate"]["id"].as_str().unwrap().to_owned();

    let api = MockServer::start().await;
    let page = |next: Option<&str>| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "duplicates": [pair.clone()],
            "next_after": next,
            "schema": 1,
        }))
    };
    Mock::given(method("GET"))
        .and(path("/api/v1/episodes/duplicates"))
        .and(query_param("after", cursor.as_str()))
        .respond_with(page(None))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/episodes/duplicates"))
        .and(query_param_is_missing("after"))
        .respond_with(page(Some(&cursor)))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/episodes/{cursor}/resolve")))
        .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
            "error": { "kind": "conflict", "message": "both are in the feed" },
            "schema": 1,
        })))
        .mount(&api)
        .await;
    let uri = api.uri();
    let remote = ["--server", uri.as_str(), "--json"];
    let out = uguisu(
        &server,
        dir.path(),
        &[&remote[..], &["episode", "duplicates"]].concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["duplicates"].as_array().unwrap().len(), 2);
    let out = uguisu(
        &server,
        dir.path(),
        &[&remote[..], &["episode", "resolve", &cursor, "--same"]].concat(),
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("both are in the feed"),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn move_feed_needs_force_when_unverified() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let renamed = format!("{}/renamed.xml", server.uri());
    let announcing = V1.replacen(
        "<itunes:author>V Host</itunes:author>",
        &format!("<itunes:new-feed-url>{renamed}</itunes:new-feed-url>"),
        1,
    );
    serve(&server, "/feed.xml", feed(&announcing)).await;
    serve(
        &server,
        "/renamed.xml",
        feed(&V1.replacen("<title>Versioned Show</title>", "<title>Renamed</title>", 1)),
    )
    .await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "add", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let id = json(&out)["podcast"]["id"].as_str().unwrap().to_owned();

    let out = uguisu(&server, dir.path(), &["podcast", "show", &id]);
    let text = stdout(&out);
    assert!(text.contains(&format!("Announced:   {renamed}")), "{text}");
    assert!(
        text.contains(&format!("uguisu podcast move-feed {id} {renamed}")),
        "{text}"
    );

    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "move-feed", &id, &renamed],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stdout(&out).contains("Not moved"), "{}", stdout(&out));
    assert!(stderr(&out).contains("--force"), "{}", stderr(&out));
    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "podcast", "move-feed", &id, &renamed, "--dry-run"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["moved"], false);

    let out = uguisu(
        &server,
        dir.path(),
        &["podcast", "move-feed", &id, &renamed, "--force"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("Moved podcast"), "{}", stdout(&out));
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "show", &id]);
    let shown = json(&out);
    assert_eq!(shown["source"]["feed_url"], renamed, "{shown}");
    assert_eq!(shown["announced"], serde_json::Value::Null);

    let out = uguisu(&server, dir.path(), &["podcast", "move-feed", "nope", &url]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[tokio::test]
async fn archive_then_remove_needs_yes() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "add", &url]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let id = json(&out)["podcast"]["id"].as_str().unwrap().to_owned();

    let out = uguisu(&server, dir.path(), &["podcast", "archive", &id]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{id} is archived\n"));
    let out = uguisu(&server, dir.path(), &["podcast", "refresh", &id]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("archived"), "{}", stderr(&out));

    let out = uguisu(&server, dir.path(), &["--json", "podcast", "remove", &id]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "show", &id]);
    assert_eq!(out.status.code(), Some(0), "nothing was removed");

    let out = uguisu(
        &server,
        dir.path(),
        &["--json", "podcast", "remove", &id, "--yes"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let removed = json(&out);
    assert_eq!(removed["schema"], 1);
    assert_eq!(removed["episodes"], 3, "{removed}");
    assert_eq!(removed["files"], 0, "{removed}");
    let out = uguisu(&server, dir.path(), &["podcast", "show", &id]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = uguisu(&server, dir.path(), &["podcast", "remove", &id, "--yes"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[tokio::test]
async fn remote_remove_posts_and_reports() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let api = MockServer::start().await;
    let id = "01J00000000000000000000000";
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/podcasts/{id}/archive")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "podcast_id": id, "status": "archived", "schema": 1,
        })))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/podcasts/{id}/remove")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "podcast_id": id, "title": "Gone", "episodes": 4, "files": 2, "schema": 1,
        })))
        .up_to_n_times(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/podcasts/{id}/remove")))
        .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
            "error": { "kind": "conflict", "message": "a download of Gone is running" },
            "schema": 1,
        })))
        .mount(&api)
        .await;
    let uri = api.uri();
    let remote = ["--server", uri.as_str()];
    let out = uguisu(
        &server,
        dir.path(),
        &[&remote[..], &["podcast", "archive", id]].concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{id} is archived\n"));
    let out = uguisu(
        &server,
        dir.path(),
        &[&remote[..], &["podcast", "remove", id, "--yes"]].concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("Removed podcast {id} (Gone): 4 episodes; its 2 archived files stay on disk\n")
    );
    let out = uguisu(
        &server,
        dir.path(),
        &[&remote[..], &["podcast", "remove", id, "--yes"]].concat(),
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("is running"), "{}", stderr(&out));
}

#[tokio::test]
async fn episode_list_and_show() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    serve(&server, "/feed.xml", feed(V1)).await;
    let url = format!("{}/feed.xml", server.uri());
    let out = uguisu(&server, dir.path(), &["--json", "podcast", "add", &url]);
    let id = json(&out)["podcast"]["id"].as_str().unwrap().to_owned();

    let out = uguisu(&server, dir.path(), &["--json", "episode", "list", &id]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let episodes = json(&out)["episodes"].as_array().unwrap().clone();
    assert_eq!(episodes.len(), 3);
    let first = episodes[0]["id"].as_str().unwrap().to_owned();
    let title = episodes[0]["title"].as_str().unwrap().to_owned();
    let out = uguisu(&server, dir.path(), &["episode", "list", &id]);
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 4, "{text}");
    assert!(text.lines().nth(1).unwrap().starts_with(&first), "{text}");

    let out = uguisu(&server, dir.path(), &["episode", "show", &first]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with(&format!("{title}\n")), "{text}");
    assert!(
        text.contains(&format!("  ID:          {first}\n")),
        "{text}"
    );
    assert!(text.contains("  Archive:     expected"), "{text}");
    let out = uguisu(&server, dir.path(), &["--json", "episode", "show", &first]);
    assert_eq!(json(&out)["episode"]["id"], first.as_str());

    for args in [
        ["episode", "show", "nope"],
        ["episode", "list", "nope"],
        ["episode", "show", "01J00000000000000000000000"],
    ] {
        let out = uguisu(&server, dir.path(), &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
}
