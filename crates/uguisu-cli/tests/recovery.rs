//! A real process killed without warning: `serve` in the middle of its
//! downloads, at more than one offset, and an archive import in the middle
//! of its copies. Nothing is shut down cleanly; `Child::kill` is SIGKILL on
//! Unix and `TerminateProcess` on Windows. Afterwards the archive verifies
//! in full, the database checks clean, and `archive orphans` finds nothing
//! but the one scratch copy an interrupted copy may leave.
//!
//! The checklist's other reliability scenarios, and which test plays each,
//! are in `docs/benchmarks/2026-10-04-phase11-verification.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
            // Windows sockets cannot start without SystemRoot, and SQLite and
            // temp_dir() fall back to C:\Windows without TEMP/TMP.
            .envs(
                ["SystemRoot", "TEMP", "TMP"]
                    .into_iter()
                    .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
            )
            .env("UGUISU_DATA_DIR", self.dir.path())
            .env("UGUISU_DISCOVERY_APPLE_ENABLED", "false")
            .env("UGUISU_HTTP_ALLOW_PRIVATE_HOSTS", "127.0.0.1")
            .env("UGUISU_DOWNLOAD_MIN_FREE_BYTES", "0")
            .env("UGUISU_LOG", "error");
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let out = self.command(args).output().expect("run uguisu");
        eprintln!(
            "$ uguisu {}\n  exit {:?}\n  stderr: {}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        out
    }

    fn json(&self, args: &[&str], exit: i32) -> serde_json::Value {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(exit),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn media_dir(&self) -> PathBuf {
        self.dir.path().join("media")
    }

    /// Serves a feed of `items` (title, `YYYY-MM-DD`, enclosure path) and adds
    /// it; returns the podcast id.
    async fn add_podcast(&self, items: &[(String, String, String)]) -> String {
        let mut body = String::from(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Media Show</title>\
             <link>https://media.example/</link><description>d</description>",
        );
        for (i, (title, date, enclosure)) in items.iter().enumerate() {
            use std::fmt::Write as _;
            let _ = write!(
                body,
                "<item><title>{title}</title><guid isPermaLink=\"false\">m-{i}</guid>\
                 <pubDate>{date}T10:00:00Z</pubDate>\
                 <enclosure url=\"{}{enclosure}\" type=\"audio/mpeg\" length=\"1000\"/></item>",
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
        let added = self.json(&["--json", "podcast", "add", &url], 0);
        added["podcast"]["id"].as_str().unwrap().to_owned()
    }

    /// After a kill: the archive is whole, the database sound, and nothing is
    /// left over but at most one scratch copy.
    fn assert_recovered(&self, files: u64) {
        let verified = self.json(&["--json", "archive", "verify", "--all", "--full"], 0);
        assert_eq!(verified["verified"], files, "{verified}");
        self.json(&["--json", "db", "check"], 0);
        let out = self.run(&["--json", "archive", "orphans"]);
        let orphans: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        for group in [
            "orphan_parts",
            "unknown_media",
            "stray_sidecars",
            "unreadable",
        ] {
            assert_eq!(orphans[group]["count"], 0, "{group}: {orphans}");
        }
        assert!(
            orphans["leftovers"]["count"].as_u64().unwrap() <= 1,
            "{orphans}"
        );
    }
}

/// Every file under `dir` whose name ends in `suffix`, with its size.
fn sizes(dir: &Path, suffix: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(sizes(&path, suffix));
        } else if path.to_string_lossy().ends_with(suffix) {
            out.push(entry.metadata().map_or(0, |m| m.len()));
        }
    }
    out
}

/// Kills `child` once `ready` holds, and fails if it never does.
fn kill_when(mut child: Child, what: &str, ready: impl Fn() -> bool) {
    let start = Instant::now();
    while !ready() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("exited ({status}) before {what}");
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "never reached {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    child.kill().unwrap();
    child.wait().unwrap();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_serve_resumes_its_downloads() {
    let env = Env::new().await;
    // Paced at 200 kB/s, so a kill lands in the middle of each transfer.
    let sizes_bytes = [600_000_u64, 500_000, 400_000];
    let items: Vec<_> = sizes_bytes
        .iter()
        .enumerate()
        .map(|(i, size)| {
            (
                format!("Item {i}"),
                format!("2024-01-{:02}", i + 1),
                format!("/slow/{size}/200000"),
            )
        })
        .collect();
    let podcast = env.add_podcast(&items).await;
    env.json(&["--json", "download", "podcast", &podcast], 0);

    // Killed after a quarter of the largest file, then after half of it.
    for fraction in [4, 2] {
        let serve = env
            .command(&["serve", "--bind", &format!("127.0.0.1:{}", free_port())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let media = env.media_dir();
        kill_when(serve, &format!("1/{fraction} of a transfer"), || {
            sizes(&media, ".part")
                .iter()
                .any(|s| *s >= sizes_bytes[0] / fraction)
        });
    }

    let done = env.json(&["--json", "download", "run", "--until-idle"], 0);
    eprintln!("{done}");
    env.assert_recovered(3);
    let listed = env.json(&["--json", "archive", "list"], 0);
    let mut hashes: Vec<String> = listed["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["hash_value"].as_str().unwrap().to_owned())
        .collect();
    hashes.sort();
    let mut expected: Vec<String> = sizes_bytes.iter().map(|s| content_sha256(*s)).collect();
    expected.sort();
    assert_eq!(
        hashes, expected,
        "every file holds the bytes that were served"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_import_finishes_on_rerun() {
    const COUNT: usize = 120;
    let env = Env::new().await;
    let items: Vec<_> = (0..COUNT)
        .map(|i| {
            let day = time::Date::from_ordinal_date(2024, u16::try_from(i + 1).unwrap()).unwrap();
            (
                format!("Item {i}"),
                day.to_string(),
                format!("/normal/{}", 1000 + i),
            )
        })
        .collect();
    env.add_podcast(&items).await;
    let source = tempfile::tempdir().unwrap();
    let show = source.path().join("Media Show");
    std::fs::create_dir_all(&show).unwrap();
    for (i, (title, date, _)) in items.iter().enumerate() {
        let mut bytes = b"ID3\x04".to_vec();
        bytes.extend(std::iter::repeat_n(
            u8::try_from(i % 251).unwrap(),
            256 * 1024,
        ));
        std::fs::write(show.join(format!("{date} - {title}.mp3")), bytes).unwrap();
    }
    let before = digest(source.path());

    let import = env
        .command(&[
            "archive",
            "import",
            source.path().to_str().unwrap(),
            "--apply",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let media = env.media_dir();
    kill_when(import, "ten copies", || sizes(&media, ".mp3").len() >= 10);
    let copied = sizes(&media, ".mp3").len();
    assert!(copied < COUNT, "the kill came after the last copy");

    let rerun = env.json(
        &[
            "--json",
            "archive",
            "import",
            source.path().to_str().unwrap(),
            "--apply",
        ],
        0,
    );
    assert_eq!(
        rerun["imported"].as_u64().unwrap() + rerun["already_present"].as_u64().unwrap(),
        COUNT as u64,
        "{rerun}"
    );
    env.assert_recovered(COUNT as u64);
    assert_eq!(digest(source.path()), before, "the source is only read");
}

/// Every file under `root` with its size and SHA-256, one line each, sorted.
fn digest(root: &Path) -> String {
    use sha2::{Digest, Sha256};
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut lines: Vec<String> = files
        .iter()
        .map(|file| {
            let bytes = std::fs::read(file).unwrap();
            format!(
                "{} {} {}",
                file.strip_prefix(root).unwrap().display(),
                bytes.len(),
                hex::encode(Sha256::digest(&bytes))
            )
        })
        .collect();
    lines.sort();
    lines.join(
        "
",
    )
}
