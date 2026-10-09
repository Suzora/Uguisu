//! Upgrading keeps everything (ADR 0054): every fixture under
//! `tests/fixtures/upgrade/` is a data directory a tagged build left, with
//! what that build's `--json` commands said about it. This build must open
//! it, say at least the same, find every archived byte intact, and accept
//! the password and the token it was given.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde_json::Value;

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/upgrade");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("fixture.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn command(data: &Path, args: &[&str]) -> Command {
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
        .env("UGUISU_DATA_DIR", data)
        .env("UGUISU_DISCOVERY_APPLE_ENABLED", "false")
        .env("UGUISU_LOG", "error");
    cmd
}

fn uguisu(data: &Path, args: &[&str]) -> Output {
    command(data, args).output().expect("run uguisu")
}

/// What `expected` has and `actual` lacks or changed, as paths into the JSON.
/// A field the newer build added is not a difference: within a schema, fields
/// are only ever added (docs/API.md).
fn lost(expected: &Value, actual: &Value, at: &str, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, value) in e {
                match a.get(key) {
                    Some(now) => lost(value, now, &format!("{at}.{key}"), out),
                    None => out.push(format!("{at}.{key} is gone")),
                }
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                out.push(format!("{at} had {} entries, has {}", e.len(), a.len()));
            }
            for (i, (was, now)) in e.iter().zip(a).enumerate() {
                lost(was, now, &format!("{at}[{i}]"), out);
            }
        }
        _ if expected != actual => out.push(format!("{at}: {expected} became {actual}")),
        _ => {}
    }
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.local_addr().unwrap().port()
}

/// A request over a raw socket; the status code and the body.
fn http(port: u16, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    write!(
        s,
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    let status = out.split(' ').nth(1)?.parse().ok()?;
    let body = out.split_once("\r\n\r\n").map(|(_, b)| b.to_owned())?;
    Some((status, body))
}

#[test]
fn every_fixture_upgrades() {
    let fixtures = fixtures();
    assert!(
        !fixtures.is_empty(),
        "no fixture under tests/fixtures/upgrade"
    );
    for dir in fixtures {
        upgrades(&dir);
    }
}

fn upgrades(dir: &Path) {
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("fixture.json")).unwrap()).unwrap();
    let name = fixture["name"].as_str().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    copy_dir(&dir.join("data"), &data);

    for recorded in fixture["commands"].as_array().unwrap() {
        let args: Vec<&str> = recorded["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let out = uguisu(&data, &[&["--json"], args.as_slice()].concat());
        assert!(
            out.status.success(),
            "{name}: uguisu {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut gone = Vec::new();
        lost(&recorded["output"], &json(&out), "", &mut gone);
        assert!(gone.is_empty(), "{name}: uguisu {args:?} lost {gone:#?}");
    }

    // Every byte the old build archived is what its record and its manifest say.
    let out = uguisu(&data, &["--json", "archive", "verify", "--all", "--full"]);
    assert!(
        out.status.success(),
        "{name}: archive verify: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let podcasts = json(&uguisu(&data, &["--json", "podcast", "list"]));
    for podcast in podcasts["podcasts"].as_array().unwrap() {
        let id = podcast["podcast"]["id"].as_str().unwrap();
        let out = uguisu(
            &data,
            &["archive", "manifest", "verify", "--podcast", id, "--full"],
        );
        assert!(
            out.status.success(),
            "{name}: manifest of {id}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    // The password still signs in, and the token still opens the API.
    let port = free_port();
    let mut serve = command(&data, &["serve", "--bind", &format!("127.0.0.1:{port}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let up = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        http(port, "GET", "/api/v1/health", "").is_some_and(|(status, _)| status == 200)
    });
    let login = serde_json::json!({
        "username": fixture["username"],
        "password": fixture["password"],
    })
    .to_string();
    let signed_in = http(port, "POST", "/api/v1/auth/login", &login);
    let tokens = command(
        &data,
        &[
            "--server",
            &format!("http://127.0.0.1:{port}"),
            "--json",
            "auth",
            "token",
            "list",
        ],
    )
    .env("UGUISU_TOKEN", fixture["token"].as_str().unwrap())
    .output()
    .unwrap();
    let _ = serve.kill();
    let _ = serve.wait();
    assert!(up, "{name}: the server did not come up");
    assert_eq!(signed_in.map(|(s, _)| s), Some(200), "{name}: the password");
    assert!(
        tokens.status.success(),
        "{name}: the token: {}",
        String::from_utf8_lossy(&tokens.stderr)
    );
}
