//! The single-process lock on the data directory (ADR 0016).
//!
//! Every stateful process — `uguisu serve` and every embedded stateful CLI
//! command — holds `uguisu.lock` for as long as it runs. The lock is an OS
//! advisory lock taken with [`std::fs::File::try_lock`], so it is released
//! automatically when the process exits or crashes; the file itself stays
//! behind. The holder's process id goes to a sibling `uguisu.pid` instead:
//! a Windows lock is mandatory, so a refused process could not read it from
//! the lock file.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use uguisu_archive::layout;
use uguisu_core::UguisuError;
use uguisu_core::config::DataConfig;

/// An acquired lock; dropping it releases the lock.
#[derive(Debug)]
pub struct LockFile {
    file: File,
    path: PathBuf,
    pid_path: PathBuf,
}

impl LockFile {
    /// Takes the lock without blocking. `Locked` means another Uguisu
    /// process holds the data directory.
    pub fn acquire(path: &Path) -> Result<Self, UguisuError> {
        let pid_path = path.with_file_name(DataConfig::PID_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|e| UguisuError::Config(format!("cannot open {}: {e}", path.display())))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let holder = std::fs::read_to_string(&pid_path)
                    .ok()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());
                let detail = match holder {
                    Some(pid) => format!("{} (held by pid {pid})", path.display()),
                    None => path.display().to_string(),
                };
                return Err(UguisuError::Locked(detail));
            }
            Err(TryLockError::Error(e)) => {
                return Err(UguisuError::Config(format!(
                    "cannot lock {}: {e}",
                    path.display()
                )));
            }
        }
        // Best effort: the pid helps a human find the other process.
        if let Err(e) = write_pid(&pid_path) {
            tracing::debug!(path = %pid_path.display(), error = %e, "pid file not written");
        }
        tracing::debug!(path = %path.display(), "data directory locked");
        Ok(Self {
            file,
            path: path.to_path_buf(),
            pid_path,
        })
    }

    /// Where the lock file lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn write_pid(pid_path: &Path) -> std::io::Result<()> {
    let mut tmp = pid_path.to_path_buf().into_os_string();
    tmp.push(layout::tmp_suffix());
    let tmp = PathBuf::from(tmp);
    let written = (|| {
        let mut file = File::create(&tmp)?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, pid_path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

impl Drop for LockFile {
    fn drop(&mut self) {
        // Before unlocking: once the lock is free, the pid file may already
        // be the next holder's.
        let _ = std::fs::remove_file(&self.pid_path);
        let _ = self.file.unlock();
        tracing::debug!(path = %self.path.display(), "data directory unlocked");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_second_holder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("uguisu.lock");
        let first = LockFile::acquire(&path).unwrap();
        let pid = std::process::id().to_string();
        let written = std::fs::read_to_string(dir.path().join("uguisu.pid")).unwrap();
        assert_eq!(written.trim(), pid);
        let err = LockFile::acquire(&path).unwrap_err();
        assert!(matches!(err, UguisuError::Locked(_)), "{err}");
        assert!(
            err.to_string().contains(&pid),
            "message names the holder: {err}"
        );
        drop(first);
        assert!(!dir.path().join("uguisu.pid").exists());
        let again = LockFile::acquire(&path).unwrap();
        assert_eq!(again.path(), path);
    }
}
