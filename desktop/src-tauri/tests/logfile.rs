//! A refused second launch must not rotate the running launch's log away.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write as _;

#[path = "../src/logfile.rs"]
mod logfile;

fn read(dir: &tempfile::TempDir, name: &str) -> String {
    std::fs::read_to_string(dir.path().join(name)).unwrap_or_default()
}

#[test]
fn refused_launch_keeps_the_log() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("uguisu-desktop.log"), "earlier\n").unwrap();
    let running = logfile::open(dir.path()).unwrap();
    assert!(running.rotation.is_some());
    writeln!(&running.file, "running").unwrap();

    let refused = logfile::open(dir.path()).unwrap();
    assert!(refused.rotation.is_none(), "a second launch took the lock");
    writeln!(&refused.file, "refused").unwrap();
    drop(refused);
    writeln!(&running.file, "still running").unwrap();

    assert_eq!(
        read(&dir, "uguisu-desktop.log"),
        "running\nrefused\nstill running\n"
    );
    assert_eq!(read(&dir, "uguisu-desktop.log.1"), "earlier\n");
}

#[test]
fn next_launch_rotates_it() {
    let dir = tempfile::tempdir().unwrap();
    let first = logfile::open(dir.path()).unwrap();
    writeln!(&first.file, "first").unwrap();
    drop(first);

    let second = logfile::open(dir.path()).unwrap();
    assert!(second.rotation.is_some());
    writeln!(&second.file, "second").unwrap();

    assert_eq!(read(&dir, "uguisu-desktop.log"), "second\n");
    assert_eq!(read(&dir, "uguisu-desktop.log.1"), "first\n");
}
