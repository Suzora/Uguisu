//! End-to-end tests of the `download` command group with a temporary data
//! directory, a wiremock feed host and the scenario media server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use uguisu_download::testing::{MediaServer, content_sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Env {
    feeds: MockServer,
    media: MediaServer,
    dir: tempfile::TempDir,
}

impl Env {
    async fn new() -> Self {
        Self {
            feeds: MockServer::start().await,
            media: MediaServer::start().await,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_uguisu"));
        cmd.args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
        // Windows sockets cannot start without SystemRoot (os error 10106), and
        // SQLite and temp_dir() fall back to C:\Windows without TEMP/TMP.
        .envs(
            ["SystemRoot", "TEMP", "TMP"]
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
        )
            .env("UGUISU_DATA_DIR", self.dir.path())
            .env("UGUISU_DISCOVERY_APPLE_ENABLED", "false")
            .env("UGUISU_HTTP_ALLOW_PRIVATE_HOSTS", "127.0.0.1")
            .env("UGUISU_FEED_REFRESH_TIMEOUT_MS", "5000")
            .env("UGUISU_DOWNLOAD_PROGRESS_INTERVAL_MS", "50")
            .env("UGUISU_DOWNLOAD_MIN_FREE_BYTES", "0")
            .env("UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS", "3000")
            .env("UGUISU_LOG", "error");
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let out = self.command(args).output().expect("run uguisu");
        eprintln!(
            "$ uguisu {}\n  exit {:?}\n  stdout: {}\n  stderr: {}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        out
    }

    fn media_dir(&self) -> PathBuf {
        self.dir.path().join("media")
    }

    /// Serves a feed whose items point at the given media scenarios and
    /// adds it; returns the podcast id and the episode ids in feed order.
    async fn add_podcast(&self, scenarios: &[&str]) -> (String, Vec<String>) {
        let mut body = String::from(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Media Show</title>\
             <link>https://media.example/</link><description>d</description>",
        );
        for (i, scenario) in scenarios.iter().enumerate() {
            use std::fmt::Write as _;
            let _ = write!(
                body,
                "<item><title>Item {i}</title><guid isPermaLink=\"false\">m-{i}</guid>\
                 <pubDate>{:02} Jan 2024 10:00:00 +0000</pubDate>\
                 <enclosure url=\"{}{scenario}\" type=\"audio/mpeg\" length=\"1000\"/></item>",
                i + 1,
                self.media.base()
            );
        }
        body.push_str("</channel></rss>");
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(body),
            )
            .mount(&self.feeds)
            .await;
        let url = format!("{}/feed.xml", self.feeds.uri());
        let out = self.run(&["--json", "podcast", "add", &url]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let added = json(&out);
        let podcast = added["podcast"]["id"].as_str().unwrap().to_owned();
        // Episode ids come from the engine's episode listing (server API).
        let engine = self.engine().await;
        let page = engine
            .episodes(podcast.parse().unwrap(), None, 50)
            .await
            .unwrap();
        // The page is newest first; return feed order so `episodes[i]`
        // matches `scenarios[i]`.
        let episodes = page
            .episodes
            .iter()
            .rev()
            .map(|e| e.id.to_string())
            .collect();
        engine.close().await;
        (podcast, episodes)
    }

    async fn engine(&self) -> uguisu_engine::Engine {
        let mut config = uguisu_core::config::Config::from_lookup(|k| match k {
            "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS" => Some("127.0.0.1".into()),
            "UGUISU_DISCOVERY_APPLE_ENABLED" => Some("false".into()),
            _ => None,
        })
        .unwrap();
        config.data.data_dir = Some(self.dir.path().to_path_buf());
        uguisu_engine::Engine::open(uguisu_engine::EngineConfig::from(config))
            .await
            .unwrap()
    }
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

fn sha256_of(path: &Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // one walk through the command group
async fn enqueue_list_show_commands_and_wait() {
    let env = Env::new().await;
    let (podcast, episodes) = env
        .add_podcast(&["/range/300000", "/range/40000", "/status/404"])
        .await;

    // Queue one episode: created, then existing; bad ids are usage errors.
    let out = env.run(&["--json", "download", &episodes[0], "--priority", "high"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let queued = json(&out);
    assert_eq!(queued["schema"], 1);
    assert_eq!(queued["outcome"], "created");
    assert_eq!(queued["job"]["state"], "queued");
    assert_eq!(queued["job"]["priority"], "high");
    let job = queued["job"]["id"].as_str().unwrap().to_owned();
    let out = env.run(&["download", &episodes[0]]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("already exists"), "{}", stdout(&out));
    let out = env.run(&["--json", "download", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    let out = env.run(&["--json", "episode", "download", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    let missing = uguisu_core::ids::EpisodeId::new().to_string();
    let out = env.run(&["--json", "download", &missing]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("not found"), "{}", stderr(&out));

    // Whole podcast: the queued one is reported as existing.
    let out = env.run(&["--json", "download", "podcast", &podcast]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let summary = json(&out);
    assert_eq!(summary["created"], 2);
    assert_eq!(summary["existing"], 1);

    // List, filter, page, show.
    let out = env.run(&["--json", "download", "list"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        json(&out)["jobs"].as_array().map(Vec::len),
        Some(3),
        "stdout={} stderr={}",
        stdout(&out),
        stderr(&out)
    );
    let second = json(&out)["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["episode_id"] == episodes[1].as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = env.run(&["--json", "download", "list", "--limit", "2"]);
    let page = json(&out);
    assert_eq!(page["jobs"].as_array().unwrap().len(), 2);
    let after = page["next_after"].as_str().unwrap().to_owned();
    let out = env.run(&[
        "--json", "download", "list", "--limit", "2", "--after", &after,
    ]);
    assert_eq!(json(&out)["jobs"].as_array().unwrap().len(), 1);
    let out = env.run(&["--json", "download", "list", "--state", "completed"]);
    assert_eq!(json(&out)["jobs"].as_array().unwrap().len(), 0);
    let out = env.run(&["download", "list", "--state", "bogus"]);
    assert_eq!(out.status.code(), Some(2));
    let out = env.run(&["download", "list"]);
    assert!(stdout(&out).contains("JOB"), "{}", stdout(&out));
    assert!(stdout(&out).contains("queued"), "{}", stdout(&out));
    let out = env.run(&["--json", "download", "show", &job]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["job"]["id"], job);
    let out = env.run(&["download", "show", &job]);
    assert!(stdout(&out).contains("Job "), "{}", stdout(&out));
    let out = env.run(&["--json", "download", "show", "nope"]);
    assert_eq!(out.status.code(), Some(2));

    // Job commands and their conflicts.
    let out = env.run(&["--json", "download", "pause", &job]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["job"]["state"], "paused");
    let out = env.run(&["--json", "download", "pause", &job]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("paused"), "{}", stderr(&out));
    let out = env.run(&["--json", "download", "resume", &job]);
    assert_eq!(json(&out)["job"]["state"], "queued");
    let out = env.run(&["--json", "download", "cancel", &job]);
    assert_eq!(json(&out)["job"]["state"], "cancelled");
    let out = env.run(&["--json", "download", "retry", &job]);
    assert_eq!(json(&out)["job"]["state"], "queued");
    let out = env.run(&["--json", "download", "retry-failed"]);
    assert_eq!(json(&out)["requeued"], 0);

    // Global pause is persisted and blocks --wait with a hint.
    let out = env.run(&["--json", "download", "pause-all"]);
    assert_eq!(json(&out)["control"]["paused"], true);
    let out = env.run(&["--json", "download", "stats"]);
    assert_eq!(json(&out)["paused_all"], "user");
    // Pending jobs are parked as paused(paused_all) while the queue is paused.
    assert_eq!(json(&out)["by_state"]["paused"], 3);
    let out = env.run(&["download", &episodes[0], "--wait"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("resume-all"), "{}", stderr(&out));
    let out = env.run(&["--json", "download", "resume-all"]);
    assert_eq!(json(&out)["control"]["paused"], false);
    let out = env.run(&["--json", "download", "stats"]);
    assert_eq!(json(&out)["by_state"]["queued"], 3);
    // A --wait starts every worker in its process, so the second episode
    // waits for `download run` paused, or a --wait would take it along.
    let out = env.run(&["--json", "download", "pause", &second]);
    assert_eq!(json(&out)["job"]["state"], "paused");

    // --wait downloads the job in this process and exits with its outcome.
    let out = env.run(&["--json", "download", &episodes[0], "--wait"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let done = json(&out);
    assert_eq!(done["job"]["state"], "completed", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1);
    let target = env
        .media_dir()
        .join(done["job"]["target_path"].as_str().unwrap());
    assert!(target.exists());
    assert_eq!(sha256_of(&target), content_sha256(300_000));
    // The file lands on the rendered archive template path, not
    // on the identifier layout.
    assert!(
        target.to_string_lossy().contains("Media Show/"),
        "template path: {}",
        target.display()
    );
    // Already completed: --wait returns at once without a request.
    let hits = env.media.hits("/range/300000");
    let out = env.run(&["--json", "download", &episodes[0], "--wait"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["outcome"], "already_completed");
    assert_eq!(env.media.hits("/range/300000"), hits);
    // A 404 is a non-retryable network failure: exit 8, text progress ends.
    let out = env.run(&["download", &episodes[2], "--wait"]);
    assert_eq!(out.status.code(), Some(8), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("failed(not_retryable)"),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("not_found"), "{}", stdout(&out));

    // `download run --until-idle` drains the rest; the text output lists it.
    let out = env.run(&["--json", "download", "resume", &second]);
    assert_eq!(json(&out)["job"]["state"], "queued");
    let out = env.run(&["download", "run", "--until-idle"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("completed  "), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("This run: 1 completed, 0 failed."),
        "{}",
        stdout(&out)
    );
    let out = env.run(&["--json", "download", "run", "--until-idle"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let run = json(&out);
    assert_eq!(run["completed"], 0);
    assert_eq!(run["stats"]["by_state"]["completed"], 2);
    assert_eq!(run["stats"]["by_state"]["failed"], 1);
    let out = env.run(&["--json", "download", "reconcile", "--deep"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["missing_targets"], 0);
    assert!(
        !env.media_dir().join(&podcast).join(".uguisu-tmp").exists()
            || std::fs::read_dir(env.media_dir().join(&podcast).join(".uguisu-tmp"))
                .unwrap()
                .next()
                .is_none(),
        "no .part left behind"
    );
    // `run` is embedded only.
    let out = env.run(&["--server", "http://127.0.0.1:1", "download", "run"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

/// Waits until a `.part` file under the podcast's tmp dir reaches `min`
/// bytes and returns its path.
fn wait_for_part(
    tmp: &Path,
    min: u64,
    deadline: Duration,
    env: &Env,
    child: &mut std::process::Child,
) -> PathBuf {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "child exited early with {status}; requests: {:?}; log: {}",
                env.media.requests(),
                std::fs::read_to_string(env.dir.path().join("child.log")).unwrap_or_default()
            );
        }
        if let Ok(entries) = std::fs::read_dir(tmp) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "part")
                    && std::fs::metadata(&p).map_or(0, |m| m.len()) >= min
                {
                    return p;
                }
            }
        }
        assert!(
            start.elapsed() < deadline,
            "no .part of {min} bytes under {} in time; requests: {:?}; child log: {}",
            tmp.display(),
            env.media.requests(),
            std::fs::read_to_string(env.dir.path().join("child.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_download_resumes_with_a_range_request_after_restart() {
    let env = Env::new().await;
    // 20 MiB at 4 MiB/s: about five seconds, long enough to kill mid-way.
    let scenario = "/slow/20971520/4194304";
    let (podcast, episodes) = env.add_podcast(&[scenario]).await;

    let log = std::fs::File::create(env.dir.path().join("child.log")).unwrap();
    let mut child = env
        .command(&["--json", "download", &episodes[0], "--wait"])
        .env("UGUISU_LOG", "info")
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    let tmp = env.media_dir().join(&podcast).join(".uguisu-tmp");
    let part = wait_for_part(
        &tmp,
        4 * 1024 * 1024,
        Duration::from_secs(30),
        &env,
        &mut child,
    );
    child.kill().unwrap();
    let _ = child.wait();
    assert!(part.exists(), "the partial file survives the crash");
    let partial = std::fs::metadata(&part).unwrap().len();
    assert!(partial < 20_971_520, "killed before completion: {partial}");

    // The restart reconciles the job (downloading -> queued(recovered)) and
    // the retry resumes with a single Range request from a validated offset.
    let out = env.run(&["--json", "download", &episodes[0], "--wait"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let done = json(&out);
    assert_eq!(done["job"]["state"], "completed", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(done["attempts"][0]["outcome"], "interrupted");
    assert_eq!(done["attempts"][1]["outcome"], "completed");
    let resumed_from = done["attempts"][1]["range_start"].as_u64().unwrap();
    assert!(
        resumed_from > 0 && resumed_from <= partial,
        "{resumed_from}"
    );
    let requests = env.media.requests_for(scenario);
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests[0].range_start(), None);
    assert_eq!(requests[1].range_start(), Some(resumed_from));
    assert!(requests[1].if_range.is_some(), "validator sent on resume");
    let target = env
        .media_dir()
        .join(done["job"]["target_path"].as_str().unwrap());
    assert_eq!(sha256_of(&target), content_sha256(20_971_520));
    assert!(!part.exists(), "the partial file was renamed into place");
}

/// A free loopback port.
#[cfg(unix)]
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Minimal HTTP/1.0 GET over a raw socket (no client dependency needed).
#[cfg(unix)]
fn http_get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_runs_workers_answers_remote_wait_and_stops_on_sigterm() {
    let env = Env::new().await;
    let (_podcast, episodes) = env.add_podcast(&["/range/120000"]).await;
    let port = free_port();
    let bind = format!("127.0.0.1:{port}");
    let log = std::fs::File::create(env.dir.path().join("serve.log")).unwrap();
    let mut serve = env
        .command(&["serve", "--bind", &bind])
        .env("UGUISU_LOG", "info")
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if http_get(port, "/api/v1/health").is_some_and(|r| r.contains("\"status\":\"ok\"")) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "server did not come up"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // The server's workers download; the client polls the job.
    let server = format!("http://127.0.0.1:{port}");
    let out = env.run(&[
        "--server",
        &server,
        "--json",
        "download",
        &episodes[0],
        "--wait",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let done = json(&out);
    assert_eq!(done["job"]["state"], "completed", "{done}");
    let target = env
        .media_dir()
        .join(done["job"]["target_path"].as_str().unwrap());
    assert_eq!(sha256_of(&target), content_sha256(120_000));
    let out = env.run(&["--server", &server, "--json", "download", "stats"]);
    assert_eq!(json(&out)["workers_started"], true);
    assert_eq!(json(&out)["by_state"]["completed"], 1);
    // The lock is held by the server: embedded commands point to --server.
    let out = env.run(&["download", "stats"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--server"), "{}", stderr(&out));

    // SIGTERM: graceful stop, exit 0, lock released.
    let status = Command::new("kill")
        .args(["-TERM", &serve.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let start = Instant::now();
    let exit = loop {
        if let Some(status) = serve.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "server did not stop on SIGTERM: {}",
            std::fs::read_to_string(env.dir.path().join("serve.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        exit.code(),
        Some(0),
        "{}",
        std::fs::read_to_string(env.dir.path().join("serve.log")).unwrap_or_default()
    );
    let out = env.run(&["--json", "download", "stats"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "lock released: {}",
        stderr(&out)
    );
}

/// The `archive` command group end to end: a downloaded episode becomes a
/// record, `verify` reports without changing anything, `path-preview`
/// creates nothing, `relocate --dry-run` moves nothing, and the policy
/// commands read and write what the engine resolves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_commands_report_verify_and_relocate() {
    let env = Env::new().await;
    let (podcast, episodes) = env.add_podcast(&["/range/4000", "/range/5000"]).await;

    // An empty archive verifies successfully and checks nothing: this is
    // exactly the contract the CI smoke test asserts.
    let out = env.run(&["--json", "archive", "verify", "--all"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let summary = json(&out);
    assert_eq!(summary["schema"], 1);
    assert_eq!(summary["checked"], 0);

    for episode in &episodes {
        let out = env.run(&["--json", "download", episode, "--wait"]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    }

    // Every download is now an artifact.
    let out = env.run(&["--json", "archive", "list"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let listed = json(&out);
    assert_eq!(listed["files"].as_array().unwrap().len(), 2, "{listed}");

    let out = env.run(&["--json", "archive", "show", &episodes[0]]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let file = json(&out);
    assert_eq!(file["hash_algo"], "sha256");
    let stored = file["relative_path"].as_str().unwrap().to_owned();
    assert!(stored.starts_with("Media Show/"), "{stored}");
    let on_disk = env.media_dir().join(&stored);
    assert!(on_disk.is_file());

    // A full verification passes and says how deep it looked.
    let out = env.run(&["--json", "archive", "verify", "--all", "--full"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let summary = json(&out);
    assert_eq!(summary["checked"], 2);
    assert_eq!(summary["verified"], 2);
    assert_eq!(summary["depth"], "full");

    // A tampered file is reported, exits non-zero, and is left alone.
    let original = std::fs::read(&on_disk).unwrap();
    let mut edited = original.clone();
    edited[0] ^= 0xFF;
    std::fs::write(&on_disk, &edited).unwrap();
    let out = env.run(&["--json", "archive", "verify", "--all", "--full"]);
    assert_eq!(out.status.code(), Some(1), "a finding is a failure");
    let summary = json(&out);
    assert_eq!(summary["invalid"], 1);
    assert_eq!(
        std::fs::read(&on_disk).unwrap(),
        edited,
        "nothing rewritten"
    );

    let out = env.run(&["--json", "archive", "invalid"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json(&out)["files"].as_array().unwrap().len(), 1);
    let out = env.run(&["--json", "archive", "missing"]);
    assert!(json(&out)["files"].as_array().unwrap().is_empty());
    std::fs::write(&on_disk, &original).unwrap();

    // A preview creates nothing; the file is already on its template path.
    let out = env.run(&["--json", "archive", "path-preview", &episodes[0]]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let preview = json(&out);
    assert_eq!(preview["resolved"], stored);
    assert_eq!(preview["would_move"], false);

    let out = env.run(&["--json", "archive", "relocate", "--all", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["dry_run"], true);
    assert!(on_disk.is_file(), "a dry run moves nothing");

    // `relocate` without a target is a usage error, not a whole-archive move.
    let out = env.run(&["--json", "archive", "relocate"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    // Policies: the default is manual, setting one takes effect, clearing
    // it goes back to the default.
    let out = env.run(&["--json", "archive", "policy", "show", &podcast]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let policy = json(&out);
    assert!(policy["stored"].is_null());
    assert_eq!(policy["effective"]["mode"], "manual");

    let out = env.run(&[
        "--json",
        "archive",
        "policy",
        "set",
        &podcast,
        "--mode",
        "auto",
        "--max-backlog",
        "5",
        "--priority",
        "high",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let policy = json(&out);
    assert_eq!(policy["effective"]["mode"], "auto");
    assert_eq!(policy["effective"]["max_backlog"], 5);
    assert_eq!(policy["effective"]["priority"], "high");

    let out = env.run(&["--json", "archive", "policy", "list"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json(&out)["policies"].as_array().unwrap().len(), 1);

    let out = env.run(&[
        "--json", "archive", "policy", "set", &podcast, "--mode", "sideways",
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    let out = env.run(&["--json", "archive", "policy", "clear", &podcast]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["cleared"], true);
    let out = env.run(&["--json", "archive", "policy", "show", &podcast]);
    assert_eq!(json(&out)["effective"]["mode"], "manual");
}

/// `archive import` reads a tree and reports a plan; it writes nothing
/// without `--apply`, and it refuses a source that contains the archive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_import_reports_a_plan_and_refuses_the_archive_itself() {
    let env = Env::new().await;

    // `archive import` scans and reports without being asked to
    // apply anything: an empty tree is a plan with nothing in it.
    let empty = env.dir.path().join("elsewhere");
    std::fs::create_dir_all(&empty).unwrap();
    let out = env.run(&["--json", "archive", "import", empty.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let plan = json(&out);
    assert_eq!(plan["schema"], 1);
    assert_eq!(plan["applied"], false);
    assert_eq!(plan["scanned"], 0);
    assert_eq!(plan["imported"], 0);

    // And it refuses a source that contains the archive: importing Uguisu's
    // own media directory into itself is not an operation.
    let out = env.run(&[
        "--json",
        "archive",
        "import",
        env.dir.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    let err: serde_json::Value = serde_json::from_str(stderr(&out).trim()).unwrap();
    assert_eq!(err["error"], "failed");
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("import_source_invalid"),
        "{err}"
    );
}

#[tokio::test]
async fn podgrab_db_needs_podgrab_format() {
    let env = Env::new().await;
    let source = env.dir.path().join("assets");
    std::fs::create_dir_all(&source).unwrap();
    let db = env.dir.path().join("podgrab.db");
    std::fs::write(&db, b"not opened").unwrap();
    let out = env.run(&[
        "archive",
        "import",
        source.to_str().unwrap(),
        "--format",
        "generic",
        "--podgrab-db",
        db.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--format generic"),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn archive_orphans_exit_one_on_finding() {
    let env = Env::new().await;
    std::fs::create_dir_all(env.media_dir()).unwrap();
    let out = env.run(&["--json", "archive", "orphans"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["clean"], true);

    let leftover = env
        .media_dir()
        .join(".uguisu/tmp/01J0000000000000000000000A.import");
    std::fs::create_dir_all(leftover.parent().unwrap()).unwrap();
    std::fs::write(&leftover, b"half a copy").unwrap();
    let out = env.run(&["archive", "orphans"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("leftovers: 1 (.uguisu/tmp/01J0000000000000000000000A.import)"),
        "{}",
        stdout(&out)
    );
    assert!(leftover.is_file(), "reported, never removed");
}
