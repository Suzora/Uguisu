//! `uguisu auth` against the real binary: what it prints, what it refuses, and
//! what must never appear anywhere.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::process::{Command, Stdio};

fn binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("uguisu")
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs the binary with `password` on stdin, which is how a password reaches
/// it: never an argument, never an environment variable.
fn run(dir: &std::path::Path, args: &[&str], stdin: &[&str]) -> Out {
    let mut child = Command::new(binary())
        .args(["--data-dir", &dir.display().to_string()])
        .args(args)
        .env_remove("UGUISU_TOKEN")
        .env_remove("UGUISU_SERVER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut pipe = child.stdin.take().unwrap();
        for line in stdin {
            writeln!(pipe, "{line}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    Out {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

const PASSWORD: &str = "correct horse battery";

#[tokio::test]
async fn setting_a_password_echoes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["auth", "set-password"], &[PASSWORD]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Password set"), "{}", out.stdout);
    assert!(
        !out.stdout.contains(PASSWORD) && !out.stderr.contains(PASSWORD),
        "the password must not be echoed: {}{}",
        out.stdout,
        out.stderr
    );
}

#[tokio::test]
async fn a_password_that_is_too_short_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["auth", "set-password"], &["short"]);
    assert_ne!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(!out.stderr.contains("short"), "{}", out.stderr);
}

#[tokio::test]
async fn a_token_prints_its_secret_once_and_never_again() {
    let dir = tempfile::tempdir().unwrap();
    let created = run(
        dir.path(),
        &["auth", "token", "create", "laptop", "--scope", "read"],
        &[],
    );
    assert_eq!(created.code, 0, "{}{}", created.stdout, created.stderr);
    assert!(
        created.stdout.contains("only time the secret is shown"),
        "{}",
        created.stdout
    );
    let secret = created
        .stdout
        .lines()
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .expect(&created.stdout)
        .to_owned();

    let listed = run(dir.path(), &["auth", "token", "list"], &[]);
    assert_eq!(listed.code, 0, "{}{}", listed.stdout, listed.stderr);
    assert!(listed.stdout.contains("laptop"), "{}", listed.stdout);
    assert!(listed.stdout.contains("read"), "{}", listed.stdout);
    assert!(
        !listed.stdout.contains(&secret),
        "listing a token must not reprint its secret: {}",
        listed.stdout
    );

    let json = run(dir.path(), &["--json", "auth", "token", "list"], &[]);
    assert!(
        !json.stdout.contains(&secret),
        "nor in JSON: {}",
        json.stdout
    );
    let parsed: serde_json::Value = serde_json::from_str(json.stdout.trim()).unwrap();
    assert_eq!(parsed["schema"], 1);
    assert_eq!(parsed["tokens"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_token_is_revoked_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let created = run(
        dir.path(),
        &["--json", "auth", "token", "create", "laptop"],
        &[],
    );
    let parsed: serde_json::Value = serde_json::from_str(created.stdout.trim()).unwrap();
    let id = parsed["token"]["id"].as_str().unwrap();

    let revoked = run(dir.path(), &["auth", "token", "revoke", id], &[]);
    assert_eq!(revoked.code, 0, "{}{}", revoked.stdout, revoked.stderr);
    let again = run(dir.path(), &["auth", "token", "revoke", id], &[]);
    assert_ne!(again.code, 0, "revoking twice is not a second revocation");

    let nonsense = run(dir.path(), &["auth", "token", "revoke", "not-an-id"], &[]);
    assert_eq!(nonsense.code, 2, "{}", nonsense.stderr);
}

#[tokio::test]
async fn an_unusable_token_is_a_loud_failure() {
    let dir = tempfile::tempdir().unwrap();
    // A value a header cannot carry used to be dropped silently, and every
    // request then went out unauthenticated — indistinguishable from a server
    // that needs no credential.
    let out = Command::new(binary())
        .args(["--data-dir", &dir.path().display().to_string()])
        .args(["--server", "http://127.0.0.1:1", "--token", "bad\nvalue"])
        .args(["podcast", "list"])
        .output()
        .unwrap();
    assert_ne!(out.status.code().unwrap_or(-1), 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("token") && !stderr.contains("bad"),
        "the refusal must name the problem, never the value: {stderr}"
    );
}

#[tokio::test]
async fn the_help_never_carries_a_credential() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["auth", "--help"],
        vec!["auth", "set-password", "--help"],
        vec!["auth", "token", "create", "--help"],
        vec!["--help"],
    ] {
        let out = run(dir.path(), &args, &[]);
        assert_eq!(out.code, 0, "{args:?}: {}", out.stderr);
        for forbidden in ["--password", "password <", "PASSWORD>"] {
            assert!(
                !out.stdout.contains(forbidden),
                "{args:?} offers to take a password as an argument: {}",
                out.stdout
            );
        }
    }
}

/// A network bind with no password set must not start. This runs the real
/// binary against a real port, because the gate exists to stop a process, and
/// a unit test of the predicate cannot show that the process stops.
#[tokio::test]
async fn an_exposed_bind_without_a_credential_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let out = Command::new(binary())
        .args(["--data-dir", &dir.path().display().to_string()])
        .args(["serve", "--bind", &format!("0.0.0.0:{port}")])
        .env_remove("UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(12),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "refusing to bind 0.0.0.0:{port} without authentication"
        )),
        "{stderr}"
    );
    for way_out in [
        "uguisu auth set-password",
        "--bind 127.0.0.1:8484",
        "UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1",
    ] {
        assert!(
            stderr.contains(way_out),
            "the message omits {way_out}: {stderr}"
        );
    }
    // Nothing is listening: the refusal happens before the listener.
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "a refused server still bound the port"
    );
}

/// Loopback is the default and needs no credential: a fresh install has none
/// and has to be usable before it gets one.
#[tokio::test]
async fn a_loopback_bind_needs_no_credential() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = Command::new(binary())
        .args(["--data-dir", &dir.path().display().to_string()])
        .args(["serve", "--bind", &format!("127.0.0.1:{port}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let up = wait_for_port(port);
    let _ = child.kill();
    let _ = child.wait();
    assert!(up, "a loopback server with no password should start");
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn wait_for_port(port: u16) -> bool {
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}
