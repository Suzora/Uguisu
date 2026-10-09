//! Reading and writing the portable `<media file>.json` sidecar
//! (ADR 0007, ADR 0024).
//!
//! The sidecar is what makes the archive self-describing. Copy one episode
//! out of the archive and its metadata travels with it; lose the database
//! and `archive reconcile --rebuild` reads the sidecars back into records.
//!
//! Two rules decide everything here. **A sidecar is metadata, not
//! evidence**: it says what Uguisu knew when it was written, so a rebuilt
//! record is `unchecked` and only a real verification pass may ever write
//! `verified`. And **a sidecar is never half-written**: the document is
//! rendered in full, flushed to a temporary file beside its target and
//! renamed over it, so a reader either sees the previous document or the
//! new one.
//!
//! Forward compatibility is asymmetric on purpose. An unknown *field* is
//! ignored, because a newer Uguisu adding one must not make its sidecars
//! unreadable here. An unknown *schema* is refused by name, because that
//! is a statement this build cannot interpret and guessing would be worse
//! than saying so.

use std::io::Write;
use std::path::{Path, PathBuf};

use uguisu_core::archive::{ArchiveErrorKind, Sidecar};

use crate::layout;
use crate::path::{PathError, RelativePath, resolve_checked};

/// Largest sidecar this build reads.
///
/// A sidecar holds one episode's metadata with the description already
/// truncated by the feed engine, so a real one is a few kilobytes. The cap
/// exists because a rebuild reads whatever is on disk: a scan of an
/// untrusted archive must not be turned into an out-of-memory by one
/// enormous file with a `.json` name.
pub const MAX_SIDECAR_BYTES: usize = 256 * 1024;

/// Why a sidecar could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// The file is larger than [`MAX_SIDECAR_BYTES`].
    #[error("sidecar is {bytes} bytes, larger than the {max} byte limit")]
    TooLarge {
        /// What was found.
        bytes: usize,
        /// The limit.
        max: usize,
    },
    /// The bytes are not the JSON document this build expects.
    #[error("sidecar is malformed: {0}")]
    Malformed(String),
    /// The document announces a schema this build does not know.
    #[error("sidecar schema {found} is newer than the supported schema {supported}")]
    UnsupportedSchema {
        /// What the document said.
        found: u32,
        /// What this build reads.
        supported: u32,
    },
    /// The path is not usable, or leaves the archive root.
    #[error(transparent)]
    Path(#[from] PathError),
    /// The file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
}

impl SidecarError {
    /// How the error is classified for the API and the CLI.
    #[must_use]
    pub const fn kind(&self) -> ArchiveErrorKind {
        match self {
            Self::Path(_) => ArchiveErrorKind::PathInvalid,
            Self::Io { .. } => ArchiveErrorKind::VerificationIo,
            _ => ArchiveErrorKind::SidecarInvalid,
        }
    }

    /// The short reason an event carries.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::TooLarge { .. } => "too_large",
            Self::Malformed(_) => "malformed",
            Self::UnsupportedSchema { .. } => "unsupported_schema",
            Self::Path(_) => "path_invalid",
            Self::Io { .. } => "io_error",
        }
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> SidecarError + '_ {
    move |source| SidecarError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Where the sidecar of a media file lives.
#[must_use]
pub fn path_for(media: &RelativePath) -> RelativePath {
    layout::sidecar_of(media)
}

/// Renders a sidecar as the exact bytes that go on disk.
///
/// Pretty-printed with a trailing newline, and in declaration order, so a
/// human can read it and two writes of the same record produce identical
/// bytes — which is what lets the engine skip a rewrite.
#[must_use]
pub fn render(sidecar: &Sidecar) -> String {
    // The document is a plain struct tree with string keys: serialization
    // cannot fail, and an empty file would be worse than an obvious one.
    let body = serde_json::to_string_pretty(sidecar)
        .unwrap_or_else(|e| format!("{{\"error\":\"sidecar could not be rendered: {e}\"}}"));
    format!("{body}\n")
}

/// Only the field that decides whether the rest may be read at all.
#[derive(serde::Deserialize)]
struct SchemaProbe {
    schema: u32,
}

/// Reads a sidecar from bytes.
///
/// The schema is checked first and on its own, so a document from a newer
/// Uguisu is refused by name even when its other fields changed shape.
pub fn parse(bytes: &[u8]) -> Result<Sidecar, SidecarError> {
    if bytes.len() > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge {
            bytes: bytes.len(),
            max: MAX_SIDECAR_BYTES,
        });
    }
    let probe: SchemaProbe = serde_json::from_slice(bytes)
        .map_err(|e| SidecarError::Malformed(format!("no readable `schema` field: {e}")))?;
    if probe.schema > Sidecar::SCHEMA {
        return Err(SidecarError::UnsupportedSchema {
            found: probe.schema,
            supported: Sidecar::SCHEMA,
        });
    }
    serde_json::from_slice(bytes).map_err(|e| SidecarError::Malformed(e.to_string()))
}

/// Reads the sidecar at a path under the archive root.
///
/// `Ok(None)` means there is none, which is a normal state and not a
/// failure: a missing sidecar degrades a future rebuild, it never breaks
/// the archive.
pub fn read(root: &Path, sidecar_relative: &RelativePath) -> Result<Option<Sidecar>, SidecarError> {
    let full = resolve_checked(root, sidecar_relative)?;
    let file = match std::fs::File::open(&full) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(&full)(e)),
    };
    // One byte past the cap is enough to refuse it: the rest is never read.
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, MAX_SIDECAR_BYTES as u64 + 1),
        &mut bytes,
    )
    .map_err(io(&full))?;
    parse(&bytes).map(Some)
}

/// Writes the sidecar for a media file and returns where it went.
///
/// Temporary file beside the target, flushed and `sync_all`ed, renamed
/// over it, then the directory is synced: at no point is a partial
/// document visible at the final path, and after this returns the bytes
/// have reached the disk rather than only the page cache.
///
/// The temporary name is unique per writer ([`layout::tmp_suffix`]), so a
/// crash can leave several; they are held while written and reported, never
/// reused or removed.
pub fn write_atomic(
    root: &Path,
    media_relative: &RelativePath,
    sidecar: &Sidecar,
) -> Result<RelativePath, SidecarError> {
    let relative = path_for(media_relative);
    let full = resolve_checked(root, &relative)?;
    let parent = full
        .parent()
        .ok_or_else(|| PathError::OutsideRoot(relative.as_str().to_owned()))?
        .to_path_buf();
    std::fs::create_dir_all(&parent).map_err(io(&parent))?;

    let mut tmp = full.clone().into_os_string();
    tmp.push(layout::tmp_suffix());
    let tmp = PathBuf::from(tmp);
    let _held = layout::Scratch::hold(&tmp);
    let body = render(sidecar);
    {
        let mut f = std::fs::File::create(&tmp).map_err(io(&tmp))?;
        f.write_all(body.as_bytes()).map_err(io(&tmp))?;
        f.flush().map_err(io(&tmp))?;
        f.sync_all().map_err(io(&tmp))?;
    }
    std::fs::rename(&tmp, &full).map_err(io(&tmp))?;
    // Best effort: a file system that will not open a directory still gave
    // us a durable file and an atomic rename.
    if let Ok(dir) = std::fs::File::open(&parent) {
        drop(dir.sync_all());
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::OffsetDateTime;
    use uguisu_core::archive::{
        ArchiveOrigin, SidecarArchive, SidecarEpisode, SidecarPodcast, SidecarSource, TagState,
    };
    use uguisu_core::ids::{EpisodeId, PodcastId};

    use super::*;

    fn sample() -> Sidecar {
        let at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        Sidecar {
            schema: Sidecar::SCHEMA,
            generator: Sidecar::GENERATOR.to_owned(),
            written_at: at,
            podcast: SidecarPodcast {
                id: PodcastId::new(),
                title: "Grüße aus Köln".to_owned(),
                author: None,
                publisher: None,
                feed_url: Some("https://feeds.example/a.xml".parse().unwrap()),
                language: Some("de".to_owned()),
                categories: vec![],
            },
            episode: SidecarEpisode {
                id: EpisodeId::new(),
                identity_key: "guid:x".to_owned(),
                identity_source: Some("guid".to_owned()),
                title: "Folge 1".to_owned(),
                published_at: Some(at),
                season: None,
                number: Some(1),
                duration_secs: None,
                description_text: None,
                guid: Some("x".to_owned()),
                link: None,
                enclosure_url: Some("https://cdn.example/a.mp3".parse().unwrap()),
                enclosure_type: Some("audio/mpeg".to_owned()),
                enclosure_length_bytes: Some(1024),
                artwork_url: None,
                chapters: Vec::new(),
                transcripts: Vec::new(),
            },
            archive: SidecarArchive {
                relative_path: "Show/a.mp3".to_owned(),
                size_bytes: 1024,
                hash_algo: "sha256".to_owned(),
                hash_value: "ab".to_owned(),
                content_type: Some("audio/mpeg".to_owned()),
                sniffed_type: Some("mp3".to_owned()),
                origin: ArchiveOrigin::Download,
                tag_state: TagState::Untagged,
                tag_mode: None,
                tagged_at: None,
                registered_at: at,
                original_tags: None,
            },
            source: Some(SidecarSource {
                hash_algo: "sha256".to_owned(),
                hash_value: "ab".to_owned(),
                size_bytes: 1024,
                origin_detail: None,
            }),
        }
    }

    #[test]
    fn a_sidecar_round_trips_exactly() {
        let s = sample();
        let text = render(&s);
        assert!(text.ends_with('\n'), "a text file ends with a newline");
        assert!(text.contains("\"schema\": 1"));
        assert!(
            text.contains("Grüße aus Köln"),
            "text stays readable, not escaped into \\u sequences"
        );
        let back = parse(text.as_bytes()).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            render(&back),
            text,
            "writing the same record twice produces the same bytes"
        );
    }

    #[test]
    fn unreadable_documents_are_refused_by_name() {
        let big = vec![b' '; MAX_SIDECAR_BYTES + 1];
        assert!(matches!(
            parse(&big).unwrap_err(),
            SidecarError::TooLarge { .. }
        ));
        assert!(matches!(
            parse(b"not json").unwrap_err(),
            SidecarError::Malformed(_)
        ));
        assert!(
            matches!(parse(b"{}").unwrap_err(), SidecarError::Malformed(m) if m.contains("schema"))
        );

        // A document from a future Uguisu is refused by name rather than
        // half-read: its fields may mean something else entirely.
        let mut future = serde_json::to_value(sample()).unwrap();
        future["schema"] = serde_json::json!(2);
        let err = parse(serde_json::to_string(&future).unwrap().as_bytes()).unwrap_err();
        match err {
            SidecarError::UnsupportedSchema { found, supported } => {
                assert_eq!((found, supported), (2, 1));
            }
            other => panic!("expected an unsupported schema, got {other}"),
        }
        assert_eq!(err_reason(&parse(b"nope").unwrap_err()), "malformed");
    }

    fn err_reason(e: &SidecarError) -> &'static str {
        e.reason()
    }

    #[test]
    fn writing_never_shows_a_partial_document() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let media = RelativePath::parse("Show/2024/Folge 1.mp3").unwrap();

        // A leftover from an interrupted write is in the way.
        let parent = root.join("Show").join("2024");
        std::fs::create_dir_all(&parent).unwrap();
        let stale = parent.join("Folge 1.mp3.json.999.0.tmp");
        std::fs::write(&stale, b"{\"half").unwrap();

        let s = sample();
        let written = write_atomic(root, &media, &s).unwrap();
        assert_eq!(written.as_str(), "Show/2024/Folge 1.mp3.json");
        assert_eq!(read(root, &written).unwrap().unwrap(), s);
        assert!(
            stale.is_file(),
            "a leftover from an earlier run is reported, never quietly reused or removed"
        );
        let scratch: Vec<String> = std::fs::read_dir(&parent)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| {
                std::path::Path::new(n)
                    .extension()
                    .is_some_and(|e| e == "tmp")
            })
            .collect();
        assert_eq!(
            scratch.len(),
            1,
            "this write left none of its own behind: {scratch:?}"
        );

        // Writing again replaces the document in one step.
        let mut second = sample();
        second.archive.hash_value = "cd".to_owned();
        write_atomic(root, &media, &second).unwrap();
        assert_eq!(
            read(root, &written).unwrap().unwrap().archive.hash_value,
            "cd"
        );

        // Nothing there yet is not an error.
        let other = RelativePath::parse("Show/2024/Folge 2.mp3.json").unwrap();
        assert_eq!(read(root, &other).unwrap(), None);
    }

    #[test]
    fn a_sidecar_stays_inside_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("media");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        // The path type refuses the obvious shapes outright.
        assert!(RelativePath::parse("../escape.mp3").is_err());
        assert!(RelativePath::parse("/etc/passwd").is_err());

        // And a symlink planted inside the archive cannot redirect a write.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            let media = RelativePath::parse("link/a.mp3").unwrap();
            let err = write_atomic(&root, &media, &sample()).unwrap_err();
            assert!(matches!(err, SidecarError::Path(PathError::OutsideRoot(_))));
            assert!(
                !outside.join("a.mp3.json").exists(),
                "nothing was written through the link"
            );
        }
    }

    #[test]
    fn a_newer_sidecar_keeps_known_fields() {
        let mut doc = serde_json::to_value(sample()).unwrap();
        doc["chapters"] = serde_json::json!([{"start_ms": 0, "title": "Intro"}]);
        doc["archive"]["transcoded_from"] = serde_json::json!("x.flac");
        let s = parse(serde_json::to_string(&doc).unwrap().as_bytes()).unwrap();
        assert_eq!(s.archive.hash_value, "ab");
        assert_eq!(s.episode.title, "Folge 1");
    }
}
