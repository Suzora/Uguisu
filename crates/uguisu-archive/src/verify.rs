//! Checking that a recorded artifact is still the file it claims to be
//! (`docs/ARCHIVE_ENGINE.md`, ADR 0021).
//!
//! The pass is **read-only**, on purpose and without exception: it never
//! writes, truncates, renames or deletes anything, and it never re-hashes
//! a file "to fix" a record. A file that has been edited in place is
//! reported as `invalid` and left exactly as the user left it; deciding
//! what to do about it is the user's, not the archive's.
//!
//! Three depths, because the cost differs by orders of magnitude:
//! [`VerifyDepth::Existence`] stats the path, [`VerifyDepth::Light`]
//! compares type, size and modification time, and [`VerifyDepth::Full`]
//! streams the whole file through SHA-256 with a bounded buffer, so a
//! 10 GiB artifact costs 1 MiB of memory rather than 10 GiB.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};
use uguisu_core::archive::{ArchiveFile, VerificationState, VerifyDepth, reason};

use crate::path::{self, PathError, RelativePath};

/// How much of a file is read at once while hashing. Large enough that the
/// syscall overhead disappears, small enough to stay out of the way.
pub const HASH_BUFFER: usize = 1024 * 1024;

/// What a record claims about the file, independent of where it is stored.
///
/// Verification takes this rather than an [`ArchiveFile`] so it can also
/// check a file that has not been registered yet (a finished download) and
/// so tests need no database row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectation {
    /// Path relative to the archive root.
    pub relative_path: RelativePath,
    /// Length in bytes.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub hash_value: String,
    /// Modification time when the record was written, when one was
    /// recorded. A changed mtime alone is **not** a failure — archives get
    /// copied, restored and rsynced — it only decides whether a light pass
    /// can trust the size.
    pub mtime_unix: Option<i64>,
}

impl Expectation {
    /// The expectation a stored record describes.
    pub fn from_record(file: &ArchiveFile) -> Result<Self, PathError> {
        Ok(Self {
            relative_path: RelativePath::parse(&file.relative_path)?,
            size_bytes: file.size_bytes,
            hash_value: file.hash_value.clone(),
            mtime_unix: file.mtime_unix,
        })
    }
}

/// What a verification pass found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The resulting state.
    pub state: VerificationState,
    /// Why, from [`reason`](uguisu_core::archive::reason).
    pub reason: &'static str,
    /// How deep the pass actually went.
    pub depth: VerifyDepth,
    /// The length found on disk, when the file could be stat'ed.
    pub size_bytes: Option<u64>,
    /// The modification time found on disk.
    pub mtime_unix: Option<i64>,
    /// The hash computed, when the pass hashed the file.
    pub hash_value: Option<String>,
    /// A message for the operator when the pass could not complete.
    pub detail: Option<String>,
}

impl Outcome {
    fn new(state: VerificationState, reason: &'static str, depth: VerifyDepth) -> Self {
        Self {
            state,
            reason,
            depth,
            size_bytes: None,
            mtime_unix: None,
            hash_value: None,
            detail: None,
        }
    }

    /// Whether the artifact is intact as far as this pass looked.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self.state, VerificationState::Verified)
    }
}

/// Verifies one artifact under `root`.
///
/// Never mutates anything. A path that resolves outside the root — because
/// a directory in it became a symlink — is reported as `invalid` with
/// [`reason::OUTSIDE_ROOT`] and the file is not opened at all.
#[must_use]
pub fn verify(root: &Path, expect: &Expectation, depth: VerifyDepth) -> Outcome {
    let resolved = match path::resolve_checked(root, &expect.relative_path) {
        Ok(p) => p,
        Err(e) => {
            let mut out = Outcome::new(
                VerificationState::Invalid,
                reason::OUTSIDE_ROOT,
                VerifyDepth::Existence,
            );
            out.detail = Some(e.to_string());
            return out;
        }
    };

    // `symlink_metadata` does not follow the final component: a symlink
    // where the artifact should be is not the artifact, whatever it points
    // at, and a link pointing outside the root must not be read.
    let meta = match std::fs::symlink_metadata(&resolved) {
        Ok(m) => m,
        Err(e) => return from_io(&e, VerifyDepth::Existence),
    };
    if !meta.is_file() {
        let mut out = Outcome::new(
            VerificationState::Invalid,
            reason::NOT_A_FILE,
            VerifyDepth::Existence,
        );
        out.detail = Some(if meta.is_dir() {
            "a directory is at the recorded path".to_owned()
        } else {
            "the recorded path is a symlink or a special file".to_owned()
        });
        return out;
    }

    let size = meta.len();
    let mtime = mtime_of(&meta);
    if depth == VerifyDepth::Existence {
        // A `stat` can prove a file is gone; it cannot prove one is right.
        // So a present file comes back `unchecked`, never `verified` — a
        // startup pass must not be able to clear a hash mismatch that a
        // full run found earlier.
        let mut out = Outcome::new(
            VerificationState::Unchecked,
            reason::PRESENT,
            VerifyDepth::Existence,
        );
        out.size_bytes = Some(size);
        out.mtime_unix = mtime;
        return out;
    }

    let mut base = Outcome::new(VerificationState::Verified, reason::SIZE_MATCH, depth);
    base.size_bytes = Some(size);
    base.mtime_unix = mtime;

    if size == 0 && expect.size_bytes > 0 {
        // A zero-length file is never a usable artifact, and it is what an
        // interrupted copy or a full disk leaves behind.
        base.state = VerificationState::Invalid;
        base.reason = reason::EMPTY;
        return base;
    }
    if size != expect.size_bytes {
        base.state = VerificationState::Invalid;
        base.reason = reason::SIZE_MISMATCH;
        base.detail = Some(format!(
            "expected {} bytes, found {size}",
            expect.size_bytes
        ));
        return base;
    }
    if depth != VerifyDepth::Full {
        // A changed mtime is never a failure on its own (archives get
        // copied and restored), but it means the size proves nothing, and
        // a light pass does not hash (ADR 0021).
        if let (Some(recorded), Some(found)) = (expect.mtime_unix, mtime)
            && recorded != found
        {
            base.state = VerificationState::Unchecked;
            base.reason = reason::MTIME_CHANGED;
            base.detail = Some("modified since it was recorded; a full pass decides".to_owned());
        }
        return base;
    }

    match hash_file(&resolved) {
        Ok(hash) => {
            base.hash_value = Some(hash.clone());
            if hash.eq_ignore_ascii_case(&expect.hash_value) {
                base.reason = reason::HASH_MATCH;
            } else {
                base.state = VerificationState::Invalid;
                base.reason = reason::HASH_MISMATCH;
                base.detail = Some(format!("expected {}, computed {hash}", expect.hash_value));
            }
            base
        }
        Err(e) => from_io(&e, VerifyDepth::Full),
    }
}

/// Streams a file through SHA-256 with a bounded buffer.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUFFER];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Modification time in whole seconds since the epoch, when the platform
/// reports one at all (some filesystems do not).
fn mtime_of(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        // Before 1970: rare, but a negative timestamp is still a timestamp.
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

/// Turns an I/O failure into an outcome.
///
/// Only "not found" means the artifact is gone. A permission problem or a
/// broken disk says nothing about the file, so the artifact is *not*
/// marked missing — that would invite a re-download of a file that is
/// sitting right there.
fn from_io(e: &std::io::Error, depth: VerifyDepth) -> Outcome {
    let (state, why) = match e.kind() {
        std::io::ErrorKind::NotFound => (VerificationState::Missing, reason::NOT_FOUND),
        std::io::ErrorKind::PermissionDenied => {
            (VerificationState::Unchecked, reason::PERMISSION_DENIED)
        }
        _ => (VerificationState::Unchecked, reason::IO_ERROR),
    };
    let mut out = Outcome::new(state, why, depth);
    out.detail = Some(e.to_string());
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Write;

    use super::*;

    struct Fixture {
        dir: tempfile::TempDir,
        expect: Expectation,
    }

    fn fixture(bytes: &[u8]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Show")).unwrap();
        let relative = RelativePath::parse("Show/Ep.mp3").unwrap();
        let full = dir.path().join("Show").join("Ep.mp3");
        std::fs::write(&full, bytes).unwrap();
        let meta = std::fs::metadata(&full).unwrap();
        Fixture {
            expect: Expectation {
                relative_path: relative,
                size_bytes: bytes.len() as u64,
                hash_value: hex::encode(Sha256::digest(bytes)),
                mtime_unix: mtime_of(&meta),
            },
            dir,
        }
    }

    #[test]
    fn an_intact_file_verifies_everywhere() {
        let f = fixture(b"podcast bytes");
        for depth in [VerifyDepth::Light, VerifyDepth::Full] {
            let out = verify(f.dir.path(), &f.expect, depth);
            assert!(out.is_ok(), "{depth}: {out:?}");
            assert_eq!(out.size_bytes, Some(13));
        }
        // An existence pass reports presence, not correctness.
        let seen = verify(f.dir.path(), &f.expect, VerifyDepth::Existence);
        assert_eq!(seen.state, VerificationState::Unchecked);
        assert_eq!(seen.reason, reason::PRESENT);
        assert_eq!(seen.size_bytes, Some(13));
        let full = verify(f.dir.path(), &f.expect, VerifyDepth::Full);
        assert_eq!(full.reason, reason::HASH_MATCH);
        assert_eq!(full.hash_value.as_ref(), Some(&f.expect.hash_value));
        assert_eq!(
            verify(f.dir.path(), &f.expect, VerifyDepth::Light).hash_value,
            None,
            "a light pass must not read the file"
        );
    }

    #[test]
    fn a_deleted_file_is_missing() {
        let f = fixture(b"gone soon");
        std::fs::remove_file(f.dir.path().join("Show").join("Ep.mp3")).unwrap();
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Light);
        assert_eq!(out.state, VerificationState::Missing);
        assert_eq!(out.reason, reason::NOT_FOUND);
        // The expectation is untouched: nothing is repaired or forgotten.
        assert_eq!(out.size_bytes, None);
    }

    #[test]
    fn an_existence_pass_cannot_clear_a_finding() {
        // The file is wrong, but it is there. A `stat` must not be able to
        // say anything that would overwrite a hash mismatch.
        let f = fixture(b"original");
        std::fs::write(f.dir.path().join("Show").join("Ep.mp3"), b"tampered").unwrap();
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Existence);
        assert_eq!(out.state, VerificationState::Unchecked);
        assert_eq!(out.reason, reason::PRESENT);
        assert!(!out.state.is_problem(), "it is not a finding either");
    }

    #[test]
    fn a_truncated_file_fails_on_size() {
        let f = fixture(b"0123456789");
        let path = f.dir.path().join("Show").join("Ep.mp3");
        std::fs::write(&path, b"0123").unwrap();
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Light);
        assert_eq!(out.state, VerificationState::Invalid);
        assert_eq!(out.reason, reason::SIZE_MISMATCH);
        assert_eq!(out.size_bytes, Some(4));
        assert!(out.hash_value.is_none(), "size is enough to know");
        // The file is exactly as the test left it.
        assert_eq!(std::fs::read(&path).unwrap(), b"0123");
    }

    fn set_mtime(path: &Path, secs: i64) {
        let at =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(secs).unwrap());
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    #[test]
    fn changed_mtime_needs_full() {
        let f = fixture(b"original");
        let path = f.dir.path().join("Show").join("Ep.mp3");
        std::fs::write(&path, b"tampered").unwrap();
        set_mtime(&path, f.expect.mtime_unix.unwrap() + 3600);

        let light = verify(f.dir.path(), &f.expect, VerifyDepth::Light);
        assert_eq!(light.state, VerificationState::Unchecked, "{light:?}");
        assert_eq!(light.reason, reason::MTIME_CHANGED);
        assert!(
            light.hash_value.is_none(),
            "a light pass must not read the file"
        );
        let full = verify(f.dir.path(), &f.expect, VerifyDepth::Full);
        assert_eq!(full.reason, reason::HASH_MISMATCH);

        // Without a recorded mtime the size is all a light pass has.
        let unrecorded = Expectation {
            mtime_unix: None,
            ..f.expect.clone()
        };
        let light = verify(f.dir.path(), &unrecorded, VerifyDepth::Light);
        assert_eq!(light.reason, reason::SIZE_MATCH);
    }

    #[test]
    fn an_equal_length_edit_needs_full() {
        let f = fixture(b"original");
        let path = f.dir.path().join("Show").join("Ep.mp3");
        std::fs::write(&path, b"tampered").unwrap();
        // An edit that kept its mtime, as a tool restoring timestamps does.
        set_mtime(&path, f.expect.mtime_unix.unwrap());

        let light = verify(f.dir.path(), &f.expect, VerifyDepth::Light);
        assert!(light.is_ok(), "a light pass cannot see this: {light:?}");

        let full = verify(f.dir.path(), &f.expect, VerifyDepth::Full);
        assert_eq!(full.state, VerificationState::Invalid);
        assert_eq!(full.reason, reason::HASH_MISMATCH);
        assert!(full.detail.is_some());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"tampered",
            "verification must never write"
        );
    }

    #[test]
    fn an_empty_file_is_invalid() {
        let f = fixture(b"something");
        std::fs::write(f.dir.path().join("Show").join("Ep.mp3"), b"").unwrap();
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Light);
        assert_eq!(out.state, VerificationState::Invalid);
        assert_eq!(out.reason, reason::EMPTY);
    }

    #[test]
    fn a_directory_is_invalid_and_kept() {
        let f = fixture(b"x");
        let path = f.dir.path().join("Show").join("Ep.mp3");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Existence);
        assert_eq!(out.state, VerificationState::Invalid);
        assert_eq!(out.reason, reason::NOT_A_FILE);
        assert!(path.is_dir(), "nothing is deleted to restore consistency");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_never_the_artifact() {
        let f = fixture(b"real bytes");
        let path = f.dir.path().join("Show").join("Ep.mp3");
        let elsewhere = f.dir.path().join("elsewhere.mp3");
        std::fs::rename(&path, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Full);
        assert_eq!(out.state, VerificationState::Invalid);
        assert_eq!(out.reason, reason::NOT_A_FILE);
    }

    #[cfg(unix)]
    #[test]
    fn an_escaping_symlink_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("media");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("Ep.mp3"), b"not ours").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Show")).unwrap();

        let expect = Expectation {
            relative_path: RelativePath::parse("Show/Ep.mp3").unwrap(),
            size_bytes: 8,
            hash_value: hex::encode(Sha256::digest(b"not ours")),
            mtime_unix: None,
        };
        let out = verify(&root, &expect, VerifyDepth::Full);
        assert_eq!(out.state, VerificationState::Invalid);
        assert_eq!(out.reason, reason::OUTSIDE_ROOT);
        assert!(out.hash_value.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_stays_unchecked() {
        use std::os::unix::fs::PermissionsExt;

        let f = fixture(b"secret");
        let path = f.dir.path().join("Show").join("Ep.mp3");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Full);
        // Running as root defeats the permission bits; then the file simply
        // verifies, which is also correct.
        if !out.is_ok() {
            assert_eq!(out.state, VerificationState::Unchecked);
            assert_eq!(out.reason, reason::PERMISSION_DENIED);
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn a_large_file_hashes_bounded() {
        let bytes: Vec<u8> = (0..HASH_BUFFER * 2 + 7)
            .map(|i| u8::try_from(i % 251).unwrap_or(0))
            .collect();
        let f = fixture(&bytes);
        let out = verify(f.dir.path(), &f.expect, VerifyDepth::Full);
        assert!(out.is_ok(), "{out:?}");
        assert_eq!(out.reason, reason::HASH_MATCH);
        assert_eq!(out.size_bytes, Some(bytes.len() as u64));
    }

    #[test]
    fn an_upper_case_hash_still_matches() {
        let mut f = fixture(b"case");
        f.expect.hash_value = f.expect.hash_value.to_uppercase();
        assert!(verify(f.dir.path(), &f.expect, VerifyDepth::Full).is_ok());
    }

    #[test]
    fn hashing_matches_a_one_shot_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let bytes = b"stream me";
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        drop(file);
        assert_eq!(
            hash_file(&path).unwrap(),
            hex::encode(Sha256::digest(bytes))
        );
    }
}
