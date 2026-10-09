//! This launch's log file, which never takes a running launch's.
//!
//! The log directory is per user, and a launch opens its log before the data
//! directory's lock can refuse it. A sidecar lock decides instead: the launch
//! that takes it keeps the previous file as `.1` and starts a new one, and a
//! launch that finds it held appends to the running launch's file, so a
//! refused start adds its lines there rather than rotating them away. Both
//! open for append, so their lines interleave instead of overwriting.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// The file to write, and the lock that makes this launch the rotating one.
#[derive(Debug)]
pub struct Log {
    /// Where this launch's lines go.
    pub file: File,
    /// Holds the sidecar lock while it is open; `None` when another launch
    /// held it and this one appended.
    pub rotation: Option<File>,
}

/// Opens this launch's log in `dir`, rotating only when no other launch holds it.
pub fn open(dir: &Path) -> io::Result<Log> {
    let current = dir.join("uguisu-desktop.log");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("uguisu-desktop.log.lock"))?;
    let rotation = match lock.try_lock() {
        Ok(()) => {
            // Absent on a first launch, which has nothing to keep.
            let _ = std::fs::rename(&current, dir.join("uguisu-desktop.log.1"));
            Some(lock)
        }
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Error(error)) => return Err(error),
    };
    let file = OpenOptions::new().create(true).append(true).open(current)?;
    Ok(Log { file, rotation })
}
