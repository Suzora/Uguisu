//! Walking a directory tree without holding it in memory (ADR 0025).
//!
//! Both `reconcile --rebuild` and `archive import` read a tree they did
//! not create: one the user's own archive, the other someone else's. Three
//! properties matter more than speed.
//!
//! **Bounded memory.** The walk streams; it never builds a list of
//! everything. An archive of 500 000 files must cost the same as one of
//! 500, which is why this is an iterator and why every report that comes
//! out of it counts rather than enumerates.
//!
//! **It never follows a link.** A symlink is reported and skipped, not
//! traversed. Following one would let a planted link walk Uguisu out of
//! the tree it was pointed at — and, in the import case, out of the root
//! the user consented to.
//!
//! **One unreadable directory does not end the scan.** A permission error
//! on one subtree is yielded as an item, so the rest of the archive is
//! still examined and the user is still told what was missed.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::layout;
use crate::path::{PathError, RelativePath};

/// Bytes read from the start of each file.
///
/// Enough to recognise every container Uguisu downloads. The point is to
/// tell media from a text file that happens to be called `.mp3`, not to
/// identify a codec, and reading more would turn a scan into a read of
/// the whole archive.
pub const HEAD_LEN: usize = 16;

/// Extensions a scan treats as media.
///
/// The extension decides only whether the file is *looked at*; what it
/// actually is comes from [`looks_like_media`]. A tree is mostly artwork,
/// notes and `.nfo` files, and opening every one of them to find out
/// would make an import of a large archive pointlessly slow.
pub const MEDIA_EXTENSIONS: [&str; 15] = [
    "mp3", "m4a", "m4b", "mp4", "m4v", "aac", "ogg", "oga", "opus", "flac", "wav", "wma", "webm",
    "mkv", "mov",
];

/// How deep a scan descends before it refuses to go further.
pub const DEFAULT_MAX_DEPTH: usize = 12;

/// What a scan looks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// How deep to descend.
    pub max_depth: usize,
    /// Whether the first bytes of each file are read.
    pub read_head: bool,
    /// Extensions to yield, lowercase and without the dot.
    pub extensions: Vec<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            read_head: true,
            extensions: MEDIA_EXTENSIONS.iter().map(|e| (*e).to_owned()).collect(),
        }
    }
}

impl ScanOptions {
    /// Looks for the portable sidecars rather than for media.
    ///
    /// What `reconcile --rebuild` walks: the documents are the source, and
    /// their first bytes are of no interest because they are parsed in
    /// full anyway.
    #[must_use]
    pub fn sidecars() -> Self {
        Self {
            read_head: false,
            extensions: vec![crate::layout::SIDECAR_EXT.to_owned()],
            ..Self::default()
        }
    }
}

/// One media file a scan found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    /// Path relative to the scanned root, POSIX separators.
    pub relative: RelativePath,
    /// Where it is on this machine.
    pub absolute: PathBuf,
    /// Length in bytes.
    pub size_bytes: u64,
    /// Modification time, when the platform reported one.
    pub mtime_unix: Option<i64>,
    /// The first [`HEAD_LEN`] bytes, when they were read.
    pub head: Vec<u8>,
}

impl ScannedFile {
    /// The file name without its extension.
    #[must_use]
    pub fn stem(&self) -> &str {
        let name = self.relative.file_name();
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }

    /// The lowercase extension, without the dot.
    #[must_use]
    pub fn extension(&self) -> Option<String> {
        self.relative
            .file_name()
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
    }

    /// The directory the file sits in, which is what most foreign layouts
    /// use to say which podcast it belongs to.
    #[must_use]
    pub fn parent_name(&self) -> Option<&str> {
        self.relative
            .parent()
            .map(|p| p.rsplit('/').next().unwrap_or(p))
    }

    /// Whether the first bytes look like media rather than like text.
    #[must_use]
    pub fn looks_like_media(&self) -> bool {
        looks_like_media(&self.head)
    }
}

/// Why one entry could not be scanned. A scan yields these and carries on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// The entry could not be read.
    #[error("{path}: {detail}")]
    Unreadable {
        /// What was being read.
        path: PathBuf,
        /// Why not.
        detail: String,
    },
    /// The entry is a symbolic link, which a scan never follows.
    #[error("{0}: symbolic links are not followed")]
    Symlink(PathBuf),
    /// The name cannot be expressed as an archive path.
    #[error("{path}: {source}")]
    Unusable {
        /// The offending path.
        path: PathBuf,
        /// Why.
        source: PathError,
    },
}

impl ScanError {
    /// The path the problem is about.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Unreadable { path, .. } | Self::Unusable { path, .. } | Self::Symlink(path) => {
                path
            }
        }
    }
}

/// Whether some leading bytes look like a media container.
///
/// Deliberately a short list rather than a dependency on the download
/// engine's sniffer: `uguisu-archive` sits below `uguisu-download` in the
/// layering, and the question here is only "is this plausibly media", not
/// "which codec is it".
#[must_use]
pub fn looks_like_media(head: &[u8]) -> bool {
    if head.len() < 4 {
        // Nothing was read, so nothing is claimed either way. A scan that
        // could not read the head must not thereby reject the file.
        return head.is_empty();
    }
    head.starts_with(b"ID3")
        || head.starts_with(b"fLaC")
        || head.starts_with(b"OggS")
        || head.starts_with(b"RIFF")
        || head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
        || (head.len() >= 8 && &head[4..8] == b"ftyp")
        // MPEG audio frame sync: eleven set bits.
        || (head[0] == 0xFF && (head[1] & 0xE0) == 0xE0)
}

/// Streams the media files under `root`.
///
/// Control directories are pruned rather than filtered, so the walk never
/// descends into them at all. Non-media files are skipped silently;
/// symlinks and unreadable entries are yielded as errors so a caller can
/// report what it did not look at.
pub fn walk<'a>(
    root: &'a Path,
    options: &'a ScanOptions,
) -> impl Iterator<Item = Result<ScannedFile, ScanError>> + 'a {
    let root_owned = root.to_path_buf();
    WalkDir::new(root)
        .max_depth(options.max_depth)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            // Depth 0 is the root itself, which is named by the caller and
            // not by the tree: pruning it would scan nothing.
            entry.depth() == 0
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(layout::is_control_name)
        })
        .filter_map(move |entry| {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    let path = e.path().unwrap_or(&root_owned).to_path_buf();
                    return Some(Err(ScanError::Unreadable {
                        path,
                        detail: e.to_string(),
                    }));
                }
            };
            let file_type = entry.file_type();
            if file_type.is_symlink() {
                return Some(Err(ScanError::Symlink(entry.path().to_path_buf())));
            }
            if !file_type.is_file() {
                return None;
            }
            let Ok(relative) = entry.path().strip_prefix(&root_owned) else {
                return None;
            };
            let relative = match relative.to_str() {
                Some(text) => match RelativePath::parse(text) {
                    Ok(r) => r,
                    Err(source) => {
                        return Some(Err(ScanError::Unusable {
                            path: entry.path().to_path_buf(),
                            source,
                        }));
                    }
                },
                None => {
                    return Some(Err(ScanError::Unusable {
                        path: entry.path().to_path_buf(),
                        source: PathError::UnusableComponent(
                            entry.path().to_string_lossy().into_owned(),
                        ),
                    }));
                }
            };
            let wanted = relative
                .file_name()
                .rsplit_once('.')
                .is_some_and(|(stem, ext)| {
                    !stem.is_empty()
                        && options
                            .extensions
                            .iter()
                            .any(|known| ext.eq_ignore_ascii_case(known))
                });
            if !wanted {
                return None;
            }
            Some(read_file(entry.path(), relative, options))
        })
}

fn read_file(
    absolute: &Path,
    relative: RelativePath,
    options: &ScanOptions,
) -> Result<ScannedFile, ScanError> {
    let meta = std::fs::metadata(absolute).map_err(|e| ScanError::Unreadable {
        path: absolute.to_path_buf(),
        detail: e.to_string(),
    })?;
    let head = if options.read_head {
        read_head(absolute).map_err(|e| ScanError::Unreadable {
            path: absolute.to_path_buf(),
            detail: e.to_string(),
        })?
    } else {
        Vec::new()
    };
    Ok(ScannedFile {
        relative,
        absolute: absolute.to_path_buf(),
        size_bytes: meta.len(),
        mtime_unix: mtime_of(&meta),
        head,
    })
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; HEAD_LEN];
    let mut filled = 0;
    while filled < buf.len() {
        match f.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

fn mtime_of(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (path, body) in [
            ("Show A/ep1.mp3", b"ID3\x04ffffffffffff".as_slice()),
            ("Show A/ep2.mp3", b"ID3\x04gggggggggggg".as_slice()),
            ("Show A/cover.jpg", b"\xFF\xD8\xFFnot media".as_slice()),
            ("Show A/notes.txt", b"hello".as_slice()),
            ("Show B/deep/ep3.m4a", b"\0\0\0\x20ftypM4A ".as_slice()),
            (
                ".uguisu/manifests/x/manifest.sha256",
                b"# uguisu".as_slice(),
            ),
            (".uguisu/tmp/pending.mp3", b"ID3\x04hhhhhhhhhhhh".as_slice()),
            ("Show A/.uguisu-tmp/job.part", b"ID3\x04partial".as_slice()),
        ] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
        dir
    }

    #[test]
    fn a_scan_skips_its_own_directories() {
        let dir = tree();
        let opts = ScanOptions::default();
        let found: Vec<ScannedFile> = walk(dir.path(), &opts).filter_map(Result::ok).collect();
        let paths: Vec<&str> = found.iter().map(|f| f.relative.as_str()).collect();
        assert_eq!(
            paths,
            vec!["Show A/ep1.mp3", "Show A/ep2.mp3", "Show B/deep/ep3.m4a"],
            "artwork, notes and everything under a control directory are left alone"
        );
        assert!(found.iter().all(ScannedFile::looks_like_media));
        assert_eq!(found[0].size_bytes, 16);
        assert_eq!(found[0].stem(), "ep1");
        assert_eq!(found[0].extension().as_deref(), Some("mp3"));
        assert_eq!(found[0].parent_name(), Some("Show A"));
        assert_eq!(found[2].parent_name(), Some("deep"));
        assert!(found[0].mtime_unix.is_some());
    }

    #[test]
    fn a_file_named_like_media_shows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fake.mp3"), b"<!DOCTYPE html><html>").unwrap();
        let opts = ScanOptions::default();
        let found: Vec<ScannedFile> = walk(dir.path(), &opts).filter_map(Result::ok).collect();
        assert_eq!(found.len(), 1, "it is still scanned");
        assert!(
            !found[0].looks_like_media(),
            "...and the caller can see it is not media"
        );
        assert!(looks_like_media(b""), "an unread head claims nothing");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_reported_and_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.mp3"), b"ID3\x04not yours!!").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        std::fs::write(root.join("real.mp3"), b"ID3\x04mineminemine").unwrap();

        let opts = ScanOptions::default();
        let (ok, errs): (Vec<_>, Vec<_>) = walk(&root, &opts).partition(Result::is_ok);
        let paths: Vec<String> = ok
            .into_iter()
            .map(|f| f.unwrap().relative.as_str().to_owned())
            .collect();
        assert_eq!(
            paths,
            vec!["real.mp3"],
            "the tree behind the link was not walked"
        );
        let errs: Vec<ScanError> = errs.into_iter().map(Result::unwrap_err).collect();
        assert!(
            matches!(errs.as_slice(), [ScanError::Symlink(p)] if p.ends_with("link")),
            "and the caller is told what was skipped: {errs:?}"
        );
    }

    #[test]
    fn depth_is_bounded_entries_survive() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a/b/c/d");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(dir.path().join("a/shallow.mp3"), b"ID3\x04aaaaaaaaaaaa").unwrap();
        std::fs::write(deep.join("deep.mp3"), b"ID3\x04bbbbbbbbbbbb").unwrap();

        let opts = ScanOptions {
            max_depth: 2,
            ..ScanOptions::default()
        };
        let found: Vec<String> = walk(dir.path(), &opts)
            .filter_map(Result::ok)
            .map(|f| f.relative.as_str().to_owned())
            .collect();
        assert_eq!(found, vec!["a/shallow.mp3"]);

        // Reading a head is optional, for a caller that only wants sizes.
        let opts = ScanOptions {
            read_head: false,
            ..ScanOptions::default()
        };
        let found: Vec<ScannedFile> = walk(dir.path(), &opts).filter_map(Result::ok).collect();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|f| f.head.is_empty()));
    }
}
