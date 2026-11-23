//! Copies of the database: on demand with `uguisu backup`, and when the server stops.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn uguisu(data: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_uguisu"));
    command
        .env("UGUISU_DATA_DIR", data)
        .env("UGUISU_BACKUPS", "7")
        .env_remove("UGUISU_WEBHOOK_URL");
    command
}

fn copies(data: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(data.join("backups"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| n.ends_with(".db"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn a_copy_is_made_on_demand() {
    let data = tempfile::tempdir().unwrap();
    let output = uguisu(data.path()).arg("backup").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let printed = String::from_utf8(output.stdout).unwrap();
    let path = Path::new(printed.trim());
    assert!(path.starts_with(data.path().join("backups")), "{printed}");
    // A complete SQLite database.
    let bytes = std::fs::read(path).unwrap();
    assert!(bytes.starts_with(b"SQLite format 3\0"));
}

#[test]
fn a_copy_is_made_when_the_server_stops() {
    let data = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut server = uguisu(data.path())
        .env("UGUISU_LISTEN", format!("127.0.0.1:{port}"))
        .arg("serve")
        .spawn()
        .unwrap();
    // Wait until it answers; the first start makes its daily copy.
    let started = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the server did not start"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Copies are named by the second.
    std::thread::sleep(Duration::from_millis(1100));
    let before = copies(data.path());

    let stopped = Command::new("kill")
        .args(["-TERM", &server.id().to_string()])
        .status()
        .unwrap();
    assert!(stopped.success());
    assert!(server.wait().unwrap().success());
    let after = copies(data.path());
    assert_eq!(after.len(), before.len() + 1, "{before:?} -> {after:?}");
}
