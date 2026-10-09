//! The per-podcast `manifest.sha256` (ADR 0007, ADR 0024).
//!
//! The manifest exists so that the archive can be checked **without
//! Uguisu**. It is written in GNU coreutils' checksum format, so
//! `cd <media dir> && sha256sum -c .uguisu/manifests/<id>/manifest.sha256`
//! verifies the podcast with a tool every system already has. Paths are
//! relative to the media root, not to the manifest's own directory, which
//! makes them byte-identical to `archive_files.relative_path`: nothing has
//! to be rewritten when the template changes, and there are no `../../`
//! chains to get wrong.
//!
//! It lists **episode media only**. Sidecars, artwork, `.part` files and
//! the manifest itself are not artifacts and have their own records.
//!
//! What the manifest is *not* is independent evidence. It is rendered from
//! the index, so it reports what Uguisu believes rather than what the
//! bytes say; the header says so in the file itself, where `sha256sum -c`
//! skips it as a comment. Re-hashing while writing would prove nothing to
//! the external tool the format exists for, would cost hours of disk I/O
//! on every registration, and — the decisive part — would have nowhere to
//! record a disagreement it found. Reading the bytes is
//! `archive verify --full`'s job, which already batches, streams and
//! writes its findings onto the record.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uguisu_core::archive::{ArchiveErrorKind, ManifestEntry};
use uguisu_core::ids::PodcastId;

use crate::layout;
use crate::path::{PathError, RelativePath, resolve_checked};

/// Length of a lowercase hex SHA-256 digest.
const SHA256_HEX_LEN: usize = 64;

/// How many paths a report names before it only counts them.
///
/// A findings list is unbounded input: an archive of 500 000 files whose
/// manifest is stale would otherwise build a 500 000-element `Vec` of
/// strings to describe the problem, which is a worse problem.
pub const MAX_SAMPLE: usize = 100;

/// Largest manifest a verification reads. A line is a hash, two spaces and a
/// path of at most 200 characters, so this holds about 200 000 entries; the
/// cap exists because the file sits in the media root, where anything with
/// that name may be.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;

/// Reads a manifest file, refusing one larger than [`MAX_MANIFEST_BYTES`]
/// after reading a single byte past it.
///
/// # Errors
/// The file's own I/O errors, and `InvalidData` for a file over the cap.
pub fn read_file(path: &std::path::Path) -> std::io::Result<String> {
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(std::fs::File::open(path)?, MAX_MANIFEST_BYTES + 1),
        &mut text,
    )?;
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("larger than {MAX_MANIFEST_BYTES} bytes"),
        ));
    }
    Ok(text)
}

/// Why a manifest could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// A line is not `<hash>  <path>`.
    #[error("line {line}: {detail}")]
    Malformed {
        /// 1-based line number.
        line: usize,
        /// What was wrong.
        detail: String,
    },
    /// The same path is listed twice, so the file has two expected hashes.
    #[error("line {line}: `{path}` is listed more than once")]
    Duplicate {
        /// 1-based line number.
        line: usize,
        /// The repeated path.
        path: String,
    },
    /// A listed path is absolute, traverses, or is otherwise unusable.
    #[error("line {line}: {source}")]
    Path {
        /// 1-based line number.
        line: usize,
        /// Cause.
        source: PathError,
    },
    /// The file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
}

impl ManifestError {
    /// How the error is classified for the API and the CLI.
    #[must_use]
    pub const fn kind(&self) -> ArchiveErrorKind {
        match self {
            Self::Path { .. } => ArchiveErrorKind::PathInvalid,
            Self::Io { .. } => ArchiveErrorKind::VerificationIo,
            _ => ArchiveErrorKind::ManifestInvalid,
        }
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ManifestError + '_ {
    move |source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Where a podcast's manifest lives.
#[must_use]
pub fn path_for(podcast: PodcastId) -> RelativePath {
    layout::manifest_path(podcast)
}

/// The comment block at the top of a manifest.
///
/// `sha256sum -c` skips lines beginning with `#`, so this is free to say
/// plainly what the file is and is not.
#[must_use]
pub fn header(podcast: PodcastId, generated_at: OffsetDateTime) -> String {
    let when = generated_at
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_owned());
    format!(
        "# uguisu manifest - podcast {podcast} - generated {when}\n\
         # rendered from the archive index; the bytes were not re-read\n\
         # paths are relative to the media directory\n\
         # verify: cd <media directory> && sha256sum -c <this file>\n"
    )
}

/// One `<hash>  <path>` line, GNU-escaped where the path needs it.
///
/// Uguisu's own paths never need escaping — sanitization drops control
/// characters and `RelativePath` normalizes separators — but an imported
/// path can carry a newline or a backslash, and a manifest that silently
/// mangled such a name would be worse than one that escapes it.
#[must_use]
pub fn entry_line(entry: &ManifestEntry) -> String {
    let needs_escape = entry.relative_path.contains(['\\', '\n']);
    let path = if needs_escape {
        entry
            .relative_path
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
    } else {
        entry.relative_path.clone()
    };
    let prefix = if needs_escape { "\\" } else { "" };
    format!("{prefix}{}  {path}\n", entry.hash_value)
}

/// Renders a whole manifest.
///
/// Sorted by the byte order of the path, so the same set of artifacts
/// always produces the same file however the rows were ordered on the way
/// in. Convenient for small podcasts, tests and benchmarks; the engine
/// uses [`write_atomic`], which streams.
#[must_use]
pub fn render<I: IntoIterator<Item = ManifestEntry>>(
    podcast: PodcastId,
    generated_at: OffsetDateTime,
    entries: I,
) -> String {
    let mut sorted: Vec<ManifestEntry> = entries.into_iter().collect();
    sorted.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    sorted.dedup_by(|a, b| a.relative_path == b.relative_path);
    let mut out = header(podcast, generated_at);
    for e in &sorted {
        out.push_str(&entry_line(e));
    }
    out
}

/// What a write produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// Where the manifest went, relative to the media root.
    pub relative_path: RelativePath,
    /// How many artifacts it lists.
    pub entries: u64,
    /// SHA-256 of the manifest text itself.
    pub hash_value: String,
}

/// Writes a manifest one entry at a time.
///
/// The engine pages its rows out of the database, so it cannot hand over
/// an iterator; this lets it write each page as it arrives and never hold
/// more than one line, whatever the podcast's size. The digest of the text
/// is computed while writing, so the row can record it without a second
/// pass over the file.
#[derive(Debug)]
pub struct Writer {
    tmp: PathBuf,
    // Released when the writer is finished or dropped, renamed or not.
    _held: layout::Scratch,
    full: PathBuf,
    parent: PathBuf,
    relative: RelativePath,
    file: std::fs::File,
    hasher: Sha256,
    entries: u64,
    previous: Option<String>,
}

impl Writer {
    /// Opens the temporary file and writes the header.
    pub fn create(
        root: &Path,
        podcast: PodcastId,
        generated_at: OffsetDateTime,
    ) -> Result<Self, ManifestError> {
        let relative = path_for(podcast);
        let full = resolve_checked(root, &relative)
            .map_err(|source| ManifestError::Path { line: 0, source })?;
        let parent = full
            .parent()
            .ok_or_else(|| ManifestError::Path {
                line: 0,
                source: PathError::OutsideRoot(relative.as_str().to_owned()),
            })?
            .to_path_buf();
        std::fs::create_dir_all(&parent).map_err(io(&parent))?;
        // Unique per writer: a fixed name let two concurrent writes of one
        // manifest rename each other's file away.
        let mut tmp = full.clone().into_os_string();
        tmp.push(layout::tmp_suffix());
        let tmp = PathBuf::from(tmp);
        let scratch = layout::Scratch::hold(&tmp);

        let mut file = std::fs::File::create(&tmp).map_err(io(&tmp))?;
        let mut hasher = Sha256::new();
        let head = header(podcast, generated_at);
        hasher.update(head.as_bytes());
        file.write_all(head.as_bytes()).map_err(io(&tmp))?;
        Ok(Self {
            tmp,
            _held: scratch,
            full,
            parent,
            relative,
            file,
            hasher,
            entries: 0,
            previous: None,
        })
    }

    /// Appends one entry. Entries must arrive in ascending path order,
    /// which is how the repository pages them.
    pub fn push(&mut self, entry: &ManifestEntry) -> Result<(), ManifestError> {
        debug_assert!(
            self.previous
                .as_deref()
                .is_none_or(|p| p < entry.relative_path.as_str()),
            "manifest entries arrived out of path order: `{:?}` then `{}`",
            self.previous,
            entry.relative_path
        );
        self.previous = Some(entry.relative_path.clone());
        let line = entry_line(entry);
        self.hasher.update(line.as_bytes());
        self.file
            .write_all(line.as_bytes())
            .map_err(io(&self.tmp))?;
        self.entries += 1;
        Ok(())
    }

    /// Flushes, syncs and renames the manifest into place.
    pub fn finish(mut self) -> Result<Written, ManifestError> {
        self.file.flush().map_err(io(&self.tmp))?;
        self.file.sync_all().map_err(io(&self.tmp))?;
        drop(self.file);
        std::fs::rename(&self.tmp, &self.full).map_err(io(&self.tmp))?;
        // Best effort: the file is already durable and the rename atomic.
        if let Ok(dir) = std::fs::File::open(&self.parent) {
            drop(dir.sync_all());
        }
        Ok(Written {
            relative_path: self.relative,
            entries: self.entries,
            hash_value: hex::encode(self.hasher.finalize()),
        })
    }
}

/// Writes a podcast's manifest from an iterator of entries.
///
/// `entries` must arrive in ascending path order. Temporary file,
/// `sync_all`, rename, directory sync: a reader sees the previous manifest
/// or the new one. A crash leaves the row stale, which the next flush
/// fixes.
pub fn write_atomic<I: IntoIterator<Item = ManifestEntry>>(
    root: &Path,
    podcast: PodcastId,
    generated_at: OffsetDateTime,
    entries: I,
) -> Result<Written, ManifestError> {
    let mut writer = Writer::create(root, podcast, generated_at)?;
    for entry in entries {
        writer.push(&entry)?;
    }
    writer.finish()
}

/// Reads one line. `Ok(None)` is a comment or a blank line.
pub fn parse_line(line: &str, number: usize) -> Result<Option<ManifestEntry>, ManifestError> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('#') {
        return Ok(None);
    }
    let (escaped, body) = match trimmed.strip_prefix('\\') {
        Some(rest) => (true, rest),
        None => (false, trimmed),
    };
    let (hash, path) = body
        .split_once("  ")
        .ok_or_else(|| ManifestError::Malformed {
            line: number,
            detail: "expected `<hash>  <path>` with two spaces".to_owned(),
        })?;
    if hash.len() != SHA256_HEX_LEN || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ManifestError::Malformed {
            line: number,
            detail: format!("`{hash}` is not a {SHA256_HEX_LEN}-character hex digest"),
        });
    }
    let path = if escaped {
        unescape(path)
    } else {
        path.to_owned()
    };
    // A listed path is untrusted input: it decides which file gets read.
    let checked = RelativePath::parse(&path).map_err(|source| ManifestError::Path {
        line: number,
        source,
    })?;
    if layout::is_control_path(&checked) {
        return Err(ManifestError::Path {
            line: number,
            source: PathError::UnusableComponent(path),
        });
    }
    Ok(Some(ManifestEntry {
        relative_path: checked.into(),
        hash_value: hash.to_ascii_lowercase(),
    }))
}

fn unescape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('\\') | None => out.push('\\'),
            // An escape this format does not define is kept verbatim
            // rather than swallowed, so nothing silently changes shape.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// Reads a whole manifest.
///
/// A repeated path is an error rather than a last-one-wins: two expected
/// hashes for one file is a contradiction, and quietly picking one would
/// hide it.
pub fn parse(text: &str) -> Result<Vec<ManifestEntry>, ManifestError> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let number = i + 1;
        let Some(entry) = parse_line(line, number)? else {
            continue;
        };
        if !seen.insert(entry.relative_path.clone()) {
            return Err(ManifestError::Duplicate {
                line: number,
                path: entry.relative_path,
            });
        }
        out.push(entry);
    }
    Ok(out)
}

/// A group of findings: how many, and the first few by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Findings {
    /// How many paths are in this group.
    pub count: u64,
    /// The first [`MAX_SAMPLE`] of them, in path order.
    pub sample: Vec<String>,
}

impl Findings {
    fn push(&mut self, path: &str) {
        self.count += 1;
        if self.sample.len() < MAX_SAMPLE {
            self.sample.push(path.to_owned());
        }
    }

    /// Whether anything was found.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// What a manifest check found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestDiff {
    /// Listed files whose bytes still hash to the listed value.
    pub unchanged: u64,
    /// Listed files whose bytes differ.
    pub changed: Findings,
    /// Listed files that are not there.
    pub missing: Findings,
    /// Files present that the manifest does not list.
    pub added: Findings,
    /// Listed files that could not be read.
    pub unreadable: Findings,
}

impl ManifestDiff {
    /// Whether the manifest and the files agree completely.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.changed.is_empty()
            && self.missing.is_empty()
            && self.added.is_empty()
            && self.unreadable.is_empty()
    }
}

/// Compares a manifest against what is actually on disk.
///
/// `found` maps each media file under the podcast to its hash, or to
/// `None` when it exists but could not be read. Scoped to one podcast, so
/// the map is bounded by that podcast's episode count rather than by the
/// archive.
#[must_use]
pub fn compare(listed: &[ManifestEntry], found: &BTreeMap<String, Option<String>>) -> ManifestDiff {
    let mut diff = ManifestDiff::default();
    let mut listed_paths = BTreeSet::new();
    for entry in listed {
        listed_paths.insert(entry.relative_path.as_str());
        match found.get(&entry.relative_path) {
            None => diff.missing.push(&entry.relative_path),
            Some(None) => diff.unreadable.push(&entry.relative_path),
            Some(Some(hash)) if hash.eq_ignore_ascii_case(&entry.hash_value) => {
                diff.unchanged += 1;
            }
            Some(Some(_)) => diff.changed.push(&entry.relative_path),
        }
    }
    for path in found.keys() {
        if !listed_paths.contains(path.as_str()) {
            diff.added.push(path);
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const P: &str = "01J0000000000000000000000P";

    fn podcast() -> PodcastId {
        P.parse().unwrap()
    }

    fn at() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    fn entry(path: &str, byte: u8) -> ManifestEntry {
        ManifestEntry {
            relative_path: path.to_owned(),
            hash_value: hex::encode([byte; 32]),
        }
    }

    #[test]
    fn a_manifest_is_a_checksum_file() {
        let text = render(
            podcast(),
            at(),
            [entry("Show/b.mp3", 0xbb), entry("Show/a.mp3", 0xaa)],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("# uguisu manifest"));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("the bytes were not re-read")),
            "the file says what it is: a report of the index"
        );
        let entries: Vec<&&str> = lines.iter().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].ends_with("  Show/a.mp3"), "sorted by path");
        assert!(entries[1].ends_with("  Show/b.mp3"));
        assert_eq!(
            entries[0].split_once("  ").unwrap().0.len(),
            SHA256_HEX_LEN,
            "two spaces separate the digest from the path, as sha256sum writes it"
        );
    }

    #[test]
    fn rendering_is_order_independent() {
        let a = render(
            podcast(),
            at(),
            [
                entry("Show/c.mp3", 3),
                entry("Show/a.mp3", 1),
                entry("Show/b.mp3", 2),
            ],
        );
        let b = render(
            podcast(),
            at(),
            [
                entry("Show/a.mp3", 1),
                entry("Show/b.mp3", 2),
                entry("Show/c.mp3", 3),
            ],
        );
        assert_eq!(a, b, "the manifest is a set, not a log");
        assert_eq!(parse(&a).unwrap().len(), 3);
        assert_eq!(
            parse(&a).unwrap(),
            vec![
                entry("Show/a.mp3", 1),
                entry("Show/b.mp3", 2),
                entry("Show/c.mp3", 3),
            ]
        );
    }

    #[test]
    fn a_line_breaking_path_is_escaped() {
        // A newline is legal in a POSIX file name, and an imported archive
        // can contain one. Unescaped it would turn one entry into two.
        let odd = ManifestEntry {
            relative_path: "Show/two\nlines.mp3".to_owned(),
            hash_value: hex::encode([7; 32]),
        };
        let line = entry_line(&odd);
        assert!(line.starts_with('\\'), "GNU marks an escaped line");
        assert_eq!(
            line.trim_end_matches('\n').lines().count(),
            1,
            "the newline did not split the entry in two"
        );
        assert_eq!(
            parse_line(line.trim_end_matches('\n'), 1).unwrap().unwrap(),
            odd
        );

        // A backslash is escaped on the way out for the same reason, but it
        // cannot survive a round trip: `RelativePath` reads `\` as a
        // separator (ADR 0022), and the manifest reader does not
        // get to override how Uguisu addresses a file.
        let windowsish = ManifestEntry {
            relative_path: "Show/a\\b.mp3".to_owned(),
            hash_value: hex::encode([7; 32]),
        };
        let line = entry_line(&windowsish);
        assert!(
            line.contains("\\\\"),
            "the backslash is escaped, not dropped"
        );
        assert_eq!(
            parse_line(line.trim_end_matches('\n'), 1)
                .unwrap()
                .unwrap()
                .relative_path,
            "Show/a/b.mp3"
        );

        // A plain path is left exactly as it is.
        assert!(!entry_line(&entry("Show/a.mp3", 1)).starts_with('\\'));
    }

    #[test]
    fn a_poisoned_manifest_is_refused() {
        let good = hex::encode([1; 32]);
        for (line, what) in [
            (format!("{good} Show/a.mp3"), "one space is not the format"),
            (format!("zz{}  Show/a.mp3", &good[2..]), "not hex"),
            (format!("{}  Show/a.mp3", &good[..10]), "too short"),
            (format!("{good}  /etc/passwd"), "absolute"),
            (format!("{good}  ../../etc/passwd"), "traversal"),
            (format!("{good}  C:\\Windows\\x.mp3"), "a drive"),
            (
                format!("{good}  .uguisu/manifests/x/manifest.sha256"),
                "Uguisu's own bookkeeping is not an artifact",
            ),
        ] {
            assert!(parse_line(&line, 1).is_err(), "{what}: {line}");
        }

        // Comments and blank lines are skipped, not refused.
        assert_eq!(parse_line("# hello", 1).unwrap(), None);
        assert_eq!(parse_line("   ", 1).unwrap(), None);

        // The same file cannot have two expected hashes.
        let doubled = format!("{good}  Show/a.mp3\n{}  Show/a.mp3\n", hex::encode([2; 32]));
        match parse(&doubled).unwrap_err() {
            ManifestError::Duplicate { line, path } => {
                assert_eq!((line, path.as_str()), (2, "Show/a.mp3"));
            }
            other => panic!("expected a duplicate, got {other}"),
        }
    }

    #[test]
    fn writing_streams_and_reports_what_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let entries = vec![
            entry("Show/a.mp3", 1),
            entry("Show/b.mp3", 2),
            entry("Show/c.mp3", 3),
        ];
        let written = write_atomic(root, podcast(), at(), entries.clone()).unwrap();
        assert_eq!(
            written.relative_path.as_str(),
            ".uguisu/manifests/01J0000000000000000000000P/manifest.sha256"
        );
        assert_eq!(written.entries, 3);

        let text = std::fs::read_to_string(root.join(written.relative_path.as_str())).unwrap();
        assert_eq!(parse(&text).unwrap(), entries);
        assert_eq!(
            written.hash_value,
            hex::encode(Sha256::digest(text.as_bytes())),
            "the recorded digest is the digest of the file"
        );
        assert_eq!(
            text,
            render(podcast(), at(), entries.clone()),
            "streaming and rendering produce the same bytes"
        );
        // No scratch file survived the write.
        let dir = root.join(".uguisu/manifests/01J0000000000000000000000P");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != layout::MANIFEST_FILE)
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // A rewrite replaces it in one step.
        let second = write_atomic(root, podcast(), at(), vec![entry("Show/a.mp3", 1)]).unwrap();
        assert_eq!(second.entries, 1);
        assert_ne!(second.hash_value, written.hash_value);
    }

    #[test]
    fn a_comparison_names_then_counts() {
        let listed = vec![
            entry("Show/a.mp3", 1),
            entry("Show/b.mp3", 2),
            entry("Show/c.mp3", 3),
            entry("Show/d.mp3", 4),
        ];
        let found: BTreeMap<String, Option<String>> = [
            ("Show/a.mp3".to_owned(), Some(hex::encode([1; 32]))),
            ("Show/b.mp3".to_owned(), Some(hex::encode([9; 32]))),
            ("Show/c.mp3".to_owned(), None),
            ("Show/e.mp3".to_owned(), Some(hex::encode([5; 32]))),
        ]
        .into_iter()
        .collect();

        let diff = compare(&listed, &found);
        assert_eq!(diff.unchanged, 1);
        assert_eq!(diff.changed.sample, vec!["Show/b.mp3"]);
        assert_eq!(diff.unreadable.sample, vec!["Show/c.mp3"]);
        assert_eq!(diff.missing.sample, vec!["Show/d.mp3"]);
        assert_eq!(diff.added.sample, vec!["Show/e.mp3"]);
        assert!(!diff.is_clean());
        assert_eq!(compare(&listed, &BTreeMap::new()).missing.count, 4);

        // A big disagreement is described, not enumerated: the report must
        // not become the memory problem.
        let many: Vec<ManifestEntry> = (0..MAX_SAMPLE + 50)
            .map(|i| entry(&format!("Show/{i:04}.mp3"), 1))
            .collect();
        let diff = compare(&many, &BTreeMap::new());
        assert_eq!(diff.missing.count, (MAX_SAMPLE + 50) as u64);
        assert_eq!(diff.missing.sample.len(), MAX_SAMPLE);
    }
}
