//! Media metadata: reading and writing tags (ADR 0012, ADR 0026).
//!
//! This crate knows about audio containers and nothing else. It is handed
//! **a path and a set of values**, and it writes them into the file at
//! that path; it has never heard of the archive, the database or the
//! template. That is what lets the engine hand it a *copy* and keep the
//! rule that matters most: Uguisu never modifies the only copy of a file
//! in place.
//!
//! Two modes, and both of them preserve everything Uguisu does not
//! manage — not by remembering to, but by construction. A write starts
//! from a clone of the tag already in the file and changes only the keys
//! in [`Field::ALL`], so a publisher's lyrics, ReplayGain values, cover
//! art Uguisu did not put there and anything else survive untouched.
//! ADR 0012 sketched five policies; `overwrite` and `custom` stay unbuilt
//! rather than half-built, which ADR 0026 records.
//!
//! A container Uguisu does not tag is a **result, not an error**: one WAV
//! file in a batch must not fail the batch, and the caller records what
//! could not be written rather than pretending it did.
//!
//! [`read_identity`] reads what a foreign file says it is, for an import to
//! match it to an episode (ADR 0050). It is the only reader that parses
//! the audio properties.
//!
//! A file `lofty` cannot get through, because it panics or because it runs
//! past [`DEADLINE`], is an error of that file, never a crash or a hang
//! (ADR 0065).

pub mod capability;
pub mod field;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::error::FileParseError;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::io::{Length, Truncate};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Tag, TagExt, TagType};
use uguisu_core::archive::TagMode;

pub use capability::{SUPPORTED, Support};
pub use field::Field;

/// Why tags could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// The file could not be opened or written.
    #[error("{path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// The file could not be parsed as an audio container.
    #[error("{path}: {detail}")]
    Unreadable {
        /// Path involved.
        path: PathBuf,
        /// What `lofty` said.
        detail: String,
    },
    /// The tag could not be written back.
    #[error("{path}: {detail}")]
    NotWritten {
        /// Path involved.
        path: PathBuf,
        /// What `lofty` said.
        detail: String,
    },
}

/// A picture to embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    /// Media type, as recognised from the bytes by the caller.
    pub mime: String,
    /// The image itself.
    pub bytes: Vec<u8>,
}

/// The values Uguisu manages for one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagSet {
    /// Managed fields that have a value. A field Uguisu knows nothing
    /// about is simply absent: an empty value is never invented, and
    /// nothing is written for a field that is not here.
    pub values: BTreeMap<Field, String>,
    /// Cover art, when there is any.
    pub cover: Option<Artwork>,
}

impl TagSet {
    /// Records a value, ignoring an empty or whitespace-only one.
    pub fn set(&mut self, field: Field, value: impl Into<String>) {
        let value = value.into();
        if !value.trim().is_empty() {
            self.values.insert(field, value);
        }
    }

    /// Records a value when there is one.
    pub fn set_opt(&mut self, field: Field, value: Option<impl Into<String>>) {
        if let Some(value) = value {
            self.set(field, value);
        }
    }

    /// Whether anything is set at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.cover.is_none()
    }
}

/// How a write ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeState {
    /// The file was rewritten.
    Written,
    /// Everything Uguisu manages already said what it should.
    NothingToWrite,
    /// Uguisu does not write tags into this container.
    Unsupported,
}

impl OutcomeState {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::NothingToWrite => "nothing_to_write",
            Self::Unsupported => "unsupported",
        }
    }
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagOutcome {
    /// How it ended.
    pub state: OutcomeState,
    /// The container, as recognised from the bytes.
    pub format: Option<&'static str>,
    /// Fields that changed.
    pub written: Vec<Field>,
    /// Fields this container has nowhere to put. Reported so the caller
    /// can keep them in the sidecar instead of losing them.
    pub not_embeddable: Vec<Field>,
    /// Whether cover art was embedded.
    pub cover_written: bool,
    /// Why, when nothing was written.
    pub detail: Option<&'static str>,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> MetadataError + '_ {
    move |source| MetadataError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The short name of a container, as `sniffed_type` spells it.
#[must_use]
pub const fn format_name(file_type: FileType) -> &'static str {
    match file_type {
        FileType::Mpeg => "mp3",
        FileType::Mp4 => "mp4",
        FileType::Flac => "flac",
        FileType::Vorbis => "ogg",
        FileType::Opus => "opus",
        FileType::Wav => "wav",
        FileType::Aiff => "aiff",
        FileType::Aac => "aac",
        FileType::Ape => "ape",
        FileType::Mpc => "mpc",
        FileType::Speex => "speex",
        FileType::WavPack => "wavpack",
        _ => "unknown",
    }
}

/// What managing tags needs: the tags, and not the audio's properties.
fn tags_only() -> ParseOptions {
    ParseOptions::new().read_properties(false)
}

/// How long reading or writing a small file may take; a larger one gets
/// a second more for every [`BYTES_PER_SECOND`].
///
/// `lofty` has loops a hostile file keeps from ending: fuzzing found its
/// ADTS reader seeking for ever over 30 bytes. The loops it found read or
/// seek, so a handle that fails once its deadline has passed ends them
/// (ADR 0065). A legitimate three-hour AAC file, the slowest kind to read,
/// took 2.5 s.
pub const DEADLINE: Duration = Duration::from_secs(30);

/// The slowest storage a read or write of a large file is allowed for:
/// `lofty` reads a whole MP4 or Ogg file and writes it back, and shifts the
/// tail of an MPEG or FLAC one.
pub const BYTES_PER_SECOND: u64 = 4 * 1024 * 1024;

/// When the work on one file has to be done.
#[derive(Clone, Copy)]
struct Deadline {
    until: Instant,
    allowed: Duration,
}

impl Deadline {
    fn for_file(file: &File) -> Self {
        let len = file.metadata().map_or(0, |m| m.len());
        Self::after(DEADLINE + Duration::from_secs(len / BYTES_PER_SECOND))
    }

    fn after(allowed: Duration) -> Self {
        Self {
            until: Instant::now() + allowed,
            allowed,
        }
    }

    fn passed(self) -> bool {
        Instant::now() > self.until
    }

    fn overdue(self) -> String {
        format!("not done after {} seconds", self.allowed.as_secs())
    }
}

/// A file whose every read, write and seek fails once its deadline has
/// passed.
struct Bounded {
    file: File,
    deadline: Deadline,
}

impl Bounded {
    fn check(&self) -> std::io::Result<()> {
        if self.deadline.passed() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                self.deadline.overdue(),
            ));
        }
        Ok(())
    }
}

impl Read for Bounded {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.check()?;
        self.file.read(buf)
    }
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.check()?;
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Seek for Bounded {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.check()?;
        self.file.seek(pos)
    }
}

impl Truncate for Bounded {
    fn truncate(&mut self, new_len: u64) -> std::io::Result<()> {
        self.check()?;
        self.file.set_len(new_len)
    }
}

impl Length for Bounded {
    fn len(&self) -> std::io::Result<u64> {
        self.file.metadata().map(|m| m.len())
    }
}

/// Runs `lofty` on `path`. A panic, which some of its readers raise on
/// hostile files (fuzzing found two overflows), and a run past `deadline`
/// are both `failed`, whatever `lofty` returned: it swallows some failed
/// reads and returns what it had read by then.
fn guarded<T>(
    path: &Path,
    deadline: Deadline,
    failed: fn(PathBuf, String) -> MetadataError,
    work: impl FnOnce() -> Result<T, MetadataError>,
) -> Result<T, MetadataError> {
    let done = catch_unwind(AssertUnwindSafe(work));
    if deadline.passed() {
        return Err(failed(path.to_path_buf(), deadline.overdue()));
    }
    done.unwrap_or_else(|panic| {
        let why = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Err(failed(
            path.to_path_buf(),
            format!("the parser failed: {why}"),
        ))
    })
}

fn unreadable(path: PathBuf, detail: String) -> MetadataError {
    MetadataError::Unreadable { path, detail }
}

fn not_written(path: PathBuf, detail: String) -> MetadataError {
    MetadataError::NotWritten { path, detail }
}

/// Identifies the container of `file` from its contents and parses it,
/// giving up at `deadline`.
fn parse(
    path: &Path,
    file: File,
    options: ParseOptions,
    deadline: Deadline,
) -> Result<lofty::file::TaggedFile, MetadataError> {
    let mut probe = Probe::new(BufReader::new(Bounded { file, deadline }));
    // What `Probe::open` would guess; the contents decide when they can.
    if let Some(by_name) = FileType::from_path(path) {
        probe = probe.set_file_type(by_name);
    }
    guarded(path, deadline, unreadable, || {
        let failed = |e: &dyn std::fmt::Display| unreadable(path.to_path_buf(), e.to_string());
        probe
            .options(options)
            .guess_file_type()
            .map_err(|e| failed(&e))?
            .read()
            .map_err(|e| failed(&e))
    })
}

/// Opens a file and identifies the container from its contents.
fn open(path: &Path, options: ParseOptions) -> Result<lofty::file::TaggedFile, MetadataError> {
    let file = File::open(path)
        .map_err(|e| unreadable(path.to_path_buf(), FileParseError::from(e).to_string()))?;
    let deadline = Deadline::for_file(&file);
    parse(path, file, options, deadline)
}

/// Reads the managed fields out of a file.
///
/// Unmanaged tags are not returned, because nothing here would do
/// anything with them — and a caller that cannot see them cannot
/// accidentally drop them.
pub fn read_tags(path: &Path) -> Result<TagSet, MetadataError> {
    Ok(tags_of(&open(path, tags_only())?))
}

/// The managed fields and the cover of a parsed file.
fn tags_of(tagged: &lofty::file::TaggedFile) -> TagSet {
    let mut out = TagSet::default();
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return out;
    };
    for field in Field::ALL {
        if let Some(value) = tag.get_string(field.item_key()) {
            out.set(field, value);
        }
    }
    if let Some(picture) = tag
        .pictures()
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())
    {
        out.cover = Some(Artwork {
            mime: picture.mime_type().map_or_else(
                || "application/octet-stream".to_owned(),
                ToString::to_string,
            ),
            bytes: picture.data().to_vec(),
        });
    }
    out
}

/// What a foreign file says it is: its title, an embedded episode GUID and
/// the length of its audio.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// The title tag.
    pub title: Option<String>,
    /// The embedded episode GUID (`TGID`, MP4 `egid`).
    pub episode_guid: Option<String>,
    /// The audio's length in whole seconds, rounded; `None` when it is
    /// shorter than half a second or unknown.
    pub duration_secs: Option<u32>,
}

/// Reads a file's [`Identity`] without its cover art.
///
/// Opens the file read-only, like every reader here. The audio properties
/// cost a header for most files; an MP3 without a frame index is searched
/// from its end for its last frame.
pub fn read_identity(path: &Path) -> Result<Identity, MetadataError> {
    let tagged = open(
        path,
        ParseOptions::new()
            .read_properties(true)
            .read_cover_art(false),
    )?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let text = |field: Field| {
        tag.and_then(|t| t.get_string(field.item_key()))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let millis = tagged.properties().duration().as_millis();
    Ok(Identity {
        title: text(Field::Title),
        episode_guid: text(Field::EpisodeGuid),
        duration_secs: u32::try_from(millis.saturating_add(500) / 1000)
            .ok()
            .filter(|&s| s > 0),
    })
}

/// Writes the managed fields into the file **at this path**.
///
/// The caller decides which file that is. The engine passes a copy and
/// renames it over the original once it has been re-read and re-hashed, so
/// this function never has the only copy of anything.
///
/// Returns without opening the file for writing when there is nothing to
/// change, so a repeated call is free and leaves the bytes — and therefore
/// the archive's hash — exactly as they were.
pub fn write_tags(
    path: &Path,
    desired: &TagSet,
    mode: TagMode,
) -> Result<TagOutcome, MetadataError> {
    let tagged = open(path, tags_only())?;
    let file_type = tagged.file_type();
    let name = format_name(file_type);
    let Some(tag_type) = capability::tag_type(file_type) else {
        return Ok(TagOutcome {
            state: OutcomeState::Unsupported,
            format: Some(name),
            written: Vec::new(),
            not_embeddable: desired.values.keys().copied().collect(),
            cover_written: false,
            detail: Some(capability::why_unsupported(file_type)),
        });
    };

    // Start from what the file already says. This clone is the mechanism
    // behind "no mode ever removes a tag Uguisu does not manage": every
    // frame not touched below is written back exactly as it was found.
    let mut tag = tagged
        .tag(tag_type)
        .cloned()
        .or_else(|| {
            tagged.primary_tag().cloned().map(|mut t| {
                t.re_map(tag_type);
                t
            })
        })
        .unwrap_or_else(|| Tag::new(tag_type));

    let mut written = Vec::new();
    let mut not_embeddable = Vec::new();
    for (field, value) in &desired.values {
        if !capability::support(tag_type, *field).is_writable() {
            not_embeddable.push(*field);
            continue;
        }
        let existing = tag.get_string(field.item_key());
        let should_write = match mode {
            // Never replace a value that is already there, whoever wrote
            // it. An empty value counts as absent.
            TagMode::FillMissing => existing.is_none_or(|e| e.trim().is_empty()),
            // Make it agree, but never empty it: a field Uguisu has no
            // value for is simply not in `desired`, and nothing happens.
            TagMode::Sync => existing != Some(value.as_str()),
        };
        if should_write {
            tag.insert_text(field.item_key(), value.clone());
            written.push(*field);
        }
    }

    let mut cover_written = false;
    if let Some(cover) = &desired.cover {
        // MP4 stores no picture type, and `lofty` reads every `covr` image
        // back as `Other`. Looking for a front cover there would never find
        // the one written last time, and each run would add another.
        let cover_type = if tag_type == TagType::Mp4Ilst {
            PictureType::Other
        } else {
            PictureType::CoverFront
        };
        let has_cover = tag.pictures().iter().any(|p| p.pic_type() == cover_type);
        let wanted = match mode {
            TagMode::FillMissing => !has_cover,
            TagMode::Sync => !tag
                .pictures()
                .iter()
                .any(|p| p.pic_type() == cover_type && p.data() == cover.bytes),
        };
        if wanted {
            // `from_reader` reads the signature and refuses anything that
            // is not a real image. The caller has already validated these
            // bytes, so this is a second line of defence rather than the
            // first - but an audio file is a thing other people's players
            // parse, and handing one a "picture" that is not is exactly
            // the kind of mistake that is worth making twice impossible.
            let mut picture = Picture::from_reader(&mut std::io::Cursor::new(&cover.bytes))
                .map_err(|e| MetadataError::NotWritten {
                    path: path.to_path_buf(),
                    detail: format!("cover art was refused: {e}"),
                })?;
            picture.set_pic_type(PictureType::CoverFront);
            if mode == TagMode::Sync {
                tag.remove_picture_type(cover_type);
            }
            tag.push_picture(picture);
            cover_written = true;
        }
    }

    if written.is_empty() && !cover_written {
        return Ok(TagOutcome {
            state: OutcomeState::NothingToWrite,
            format: Some(name),
            written,
            not_embeddable,
            cover_written,
            detail: Some("every managed field already says what it should"),
        });
    }

    if file_type == FileType::Mp4 {
        mp4_depth(path)?;
    }
    save(path, &tag)?;
    read_back(path, file_type, desired, &written)?;
    Ok(TagOutcome {
        state: OutcomeState::Written,
        format: Some(name),
        written,
        not_embeddable,
        cover_written,
        detail: None,
    })
}

/// Writes `tag` into the file at `path` and syncs it, giving up after
/// [`DEADLINE`] and a second for every [`BYTES_PER_SECOND`].
fn save(path: &Path, tag: &Tag) -> Result<(), MetadataError> {
    // `save_to` takes a handle the caller opened, which is what makes it
    // possible to write to a copy. `save_to_path` would open the real
    // artifact; `clippy.toml` forbids it.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(io(path))?;
    let deadline = Deadline::for_file(&file);
    let mut file = Bounded { file, deadline };
    guarded(path, deadline, not_written, || {
        tag.save_to(&mut file, WriteOptions::default())
            .map_err(|e| not_written(path.to_path_buf(), e.to_string()))
    })?;
    file.file.sync_all().map_err(io(path))
}

/// Deepest nesting of the MP4 containers `lofty`'s writer descends into.
/// It recurses once per level, and a stack overflow is no panic: nothing
/// catches it. Real files nest five deep (`moov/trak/mdia/minf/stbl`).
const MP4_MAX_DEPTH: usize = 16;

/// Refuses an MP4 whose containers nest deeper than [`MP4_MAX_DEPTH`],
/// walking its atom headers the way `lofty`'s writer does, without
/// recursion. A malformed header ends the walk: `lofty` refuses it itself.
fn mp4_depth(path: &Path) -> Result<(), MetadataError> {
    const CONTAINERS: [&[u8; 4]; 7] = [
        b"moov", b"udta", b"moof", b"trak", b"mdia", b"minf", b"stbl",
    ];
    let mut file = File::open(path).map_err(io(path))?;
    let len = file.metadata().map_err(io(path))?.len();
    // Where each open container ends, the file itself first.
    let mut ends = vec![len];
    let mut pos = 0;
    loop {
        while ends.len() > 1 && ends.last() == Some(&pos) {
            ends.pop();
        }
        let end = ends.last().copied().unwrap_or(len);
        if end.saturating_sub(pos) < 8 {
            return Ok(());
        }
        let mut header = [0; 16];
        file.seek(SeekFrom::Start(pos)).map_err(io(path))?;
        let long = end - pos >= 16;
        file.read_exact(&mut header[..if long { 16 } else { 8 }])
            .map_err(io(path))?;
        let (size, header_len) =
            match u32::from_be_bytes([header[0], header[1], header[2], header[3]]) {
                0 => (end - pos, 8),
                1 if long => (
                    u64::from_be_bytes(header[8..16].try_into().unwrap_or_default()),
                    16,
                ),
                n => (u64::from(n), 8),
            };
        if size < header_len || size > end - pos {
            return Ok(());
        }
        if CONTAINERS.iter().any(|c| header[4..8] == c[..]) {
            if ends.len() > MP4_MAX_DEPTH {
                return Err(not_written(
                    path.to_path_buf(),
                    format!("its MP4 atoms nest deeper than {MP4_MAX_DEPTH} levels"),
                ));
            }
            ends.push(pos + size);
            pos += header_len;
        } else {
            pos += size;
        }
    }
}

/// Refuses a write unless the file at `path` reads back as the same
/// container, with every value in `written` as `desired` has it: `lofty`
/// can write a file it reads as another container, or without the values,
/// and a sync write that does not read back would rewrite it on every run
/// (fuzzing found both; ADR 0065).
fn read_back(
    path: &Path,
    file_type: FileType,
    desired: &TagSet,
    written: &[Field],
) -> Result<(), MetadataError> {
    let refused = |why: String| not_written(path.to_path_buf(), why);
    let back = open(path, tags_only()).map_err(|e| match e {
        MetadataError::Unreadable { detail, .. } => {
            refused(format!("the written file does not read back: {detail}"))
        }
        other => other,
    })?;
    if back.file_type() != file_type {
        return Err(refused(format!(
            "the written file reads as {}, not {}",
            format_name(back.file_type()),
            format_name(file_type)
        )));
    }
    let read = tags_of(&back);
    if let Some(lost) = written
        .iter()
        .find(|f| read.values.get(f) != desired.values.get(f))
    {
        return Err(refused(format!(
            "{} does not read back as written",
            lost.as_str()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// ADTS frame headers that send the reader back to where it started.
    /// Found by fuzzing (ADR 0065), minimized; without a deadline it never
    /// returns.
    const ADTS_SPIN: [u8; 30] = [
        0xff, 0xf8, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0xff, 0xf8, 0xc4, 0x00, 0x00, 0xf2,
        0x00, 0xff, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xf8, 0x00, 0x00, 0x43,
    ];

    #[test]
    fn spinning_parse_stops_at_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spin");
        std::fs::write(&path, ADTS_SPIN).unwrap();
        let started = Instant::now();
        let options = ParseOptions::new().read_properties(true);
        let file = File::open(&path).unwrap();
        let deadline = Deadline::after(Duration::from_millis(200));
        let Err(err) = parse(&path, file, options, deadline) else {
            panic!("a file that never ends parsed");
        };
        assert!(
            matches!(&err, MetadataError::Unreadable { detail, .. } if detail.contains("not done after")),
            "{err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }
}
