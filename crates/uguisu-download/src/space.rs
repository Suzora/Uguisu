//! Free-space probing for the media directory, behind a trait so tests can
//! simulate a full disk on any platform.

use std::path::Path;

/// Reports the bytes available to this process on the file system of a path.
pub trait SpaceProbe: Send + Sync + std::fmt::Debug {
    /// Available bytes at `dir` (the directory must exist).
    fn available(&self, dir: &Path) -> std::io::Result<u64>;
}

/// The real probe (`statvfs` / `GetDiskFreeSpaceEx` through `fs4`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Fs4Probe;

impl SpaceProbe for Fs4Probe {
    fn available(&self, dir: &Path) -> std::io::Result<u64> {
        fs4::available_space(dir)
    }
}

/// A probe that always reports the same number (tests).
#[derive(Debug, Clone, Copy)]
pub struct FixedSpace(pub u64);

impl SpaceProbe for FixedSpace {
    fn available(&self, _dir: &Path) -> std::io::Result<u64> {
        Ok(self.0)
    }
}

/// Bytes a download still needs on disk: the remaining part of a known
/// total, else the enclosure's declared length, else a floor.
#[must_use]
pub fn bytes_needed(
    total_bytes: Option<u64>,
    bytes_downloaded: u64,
    declared_length: Option<u64>,
) -> u64 {
    /// Assumed size when nothing is known (64 MiB).
    const FLOOR: u64 = 64 * 1024 * 1024;
    match (total_bytes, declared_length) {
        (Some(t), _) => t.saturating_sub(bytes_downloaded),
        (None, Some(d)) => d.saturating_sub(bytes_downloaded),
        (None, None) => FLOOR,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_probe_reports_free_space() {
        let dir = tempfile::tempdir().unwrap();
        let n = Fs4Probe.available(dir.path()).unwrap();
        assert!(n > 0);
        assert_eq!(FixedSpace(7).available(dir.path()).unwrap(), 7);
    }

    #[test]
    fn needed_bytes() {
        assert_eq!(bytes_needed(Some(100), 40, Some(1)), 60);
        assert_eq!(bytes_needed(None, 40, Some(100)), 60);
        assert_eq!(bytes_needed(None, 0, None), 64 * 1024 * 1024);
        assert_eq!(bytes_needed(Some(10), 40, None), 0);
    }
}
