//! Writing tags into real containers.
//!
//! The fixtures are built here rather than checked in, for two reasons: a
//! hand-built file is auditable in a way a binary blob in the repository
//! is not, and every byte of it is accounted for in the comments below. An
//! MP3 frame, a FLAC `STREAMINFO` block, a few Ogg pages and a handful of
//! MP4 atoms are small enough to write out honestly.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use uguisu_core::archive::TagMode;
use uguisu_metadata::{
    Artwork, Field, Identity, OutcomeState, TagSet, read_identity, read_tags, write_tags,
};

/// A valid MPEG-1 Layer III stream of silence.
///
/// Header `FF FB 90 00`: sync, MPEG-1, Layer III, no CRC, bitrate index 9
/// (128 kbit/s), sampling index 0 (44.1 kHz), no padding, stereo. At that
/// rate a frame is `144 * 128000 / 44100 = 417` bytes, so each frame is
/// the four header bytes plus 413 bytes of payload.
fn silent_mp3() -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..20 {
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        out.extend(std::iter::repeat_n(0u8, 413));
    }
    out
}

/// A FLAC file with a `STREAMINFO` block and no audio frames.
///
/// `fLaC`, then a metadata block header (`0x80` = last block, type 0) with
/// a 24-bit length of 34, then the block: 4096-sample blocks, unknown
/// frame sizes, 44.1 kHz, two channels, 16 bits, an unknown sample count
/// and an all-zero MD5, which the format defines as "not computed".
fn silent_flac() -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
    out.extend_from_slice(&[0x10, 0x00]); // minimum block size
    out.extend_from_slice(&[0x10, 0x00]); // maximum block size
    out.extend_from_slice(&[0, 0, 0]); // minimum frame size: unknown
    out.extend_from_slice(&[0, 0, 0]); // maximum frame size: unknown
    // 20 bits sample rate, 3 bits channels-1, 5 bits bits-per-sample-1,
    // 36 bits total samples.
    out.extend_from_slice(&[10, 196, 66, 240, 0, 0, 0, 0]);
    out.extend_from_slice(&[0u8; 16]); // MD5: unknown
    out
}

fn atom(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(8 + body.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(&kind);
    out.extend_from_slice(body);
    out
}

/// An MP4 file without audio: `ftyp`, a `moov` holding only `mvhd`, and
/// an empty `mdat`.
///
/// Enough for `lofty`, which Uguisu asks for tags and never for properties:
/// the brand identifies the container, and `moov` is where the tags go.
/// With no sample table there are no chunk offsets to move when it grows.
fn silent_m4a() -> Vec<u8> {
    // Major brand, minor version 0, compatible brands.
    let ftyp = atom(*b"ftyp", b"M4A \0\0\0\0M4A isom");
    let mut mvhd = vec![0u8; 4]; // version 0, no flags
    mvhd.extend_from_slice(&[0; 8]); // creation and modification time
    mvhd.extend_from_slice(&1000u32.to_be_bytes()); // timescale
    mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    mvhd.extend_from_slice(&[0; 10]); // reserved
    for value in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        mvhd.extend_from_slice(&value.to_be_bytes()); // identity matrix
    }
    mvhd.extend_from_slice(&[0; 24]); // pre-defined
    mvhd.extend_from_slice(&1u32.to_be_bytes()); // next track id
    [
        ftyp,
        atom(*b"moov", &atom(*b"mvhd", &mvhd)),
        atom(*b"mdat", &[]),
    ]
    .concat()
}

/// Stands in for audio. It is not a decodable frame: nothing here decodes
/// audio, and tagging must carry it over byte for byte.
const AUDIO: &[u8] = b"uguisu audio marker";

/// An MP4 file whose `moov` holds the path down to a one-entry chunk-offset
/// table, and the `mdat` that entry points into.
///
/// A player needs far more (`mvhd`, `tkhd`, `stsd`, …). `lofty`'s writer
/// touches only `udta` and the offsets in `stco`, which is what this file
/// is for: a tag written into `moov` moves `mdat`, and the offset has to
/// move with it.
fn m4a_with_audio() -> Vec<u8> {
    // Major brand, minor version 0, compatible brands.
    let ftyp = atom(*b"ftyp", b"M4A \0\0\0\0M4A isom");
    let moov = |offset: u32| {
        let mut stco = vec![0; 4]; // version and flags
        stco.extend_from_slice(&1u32.to_be_bytes()); // entry count
        stco.extend_from_slice(&offset.to_be_bytes());
        let stbl = atom(*b"stbl", &atom(*b"stco", &stco));
        atom(
            *b"moov",
            &atom(*b"trak", &atom(*b"mdia", &atom(*b"minf", &stbl))),
        )
    };
    let audio_at = u32::try_from(ftyp.len() + moov(0).len() + 8).unwrap();
    [ftyp, moov(audio_at), atom(*b"mdat", AUDIO)].concat()
}

/// Where the first `stco` entry says the audio starts.
fn first_chunk_offset(file: &[u8]) -> usize {
    let at = file.windows(4).position(|w| w == b"stco").unwrap();
    // The type is followed by version and flags, then the entry count.
    u32::from_be_bytes(file[at + 12..at + 16].try_into().unwrap()) as usize
}

/// One Ogg page carrying one whole packet.
///
/// The 27-byte header, then the lacing values, then the packet; the CRC is
/// the format's own (polynomial `0x04C11DB7`, no reflection, initial value
/// 0) over the page with its CRC field zeroed.
fn ogg_page(header_type: u8, sequence: u32, packet: &[u8]) -> Vec<u8> {
    let mut lacing = vec![255u8; packet.len() / 255];
    lacing.push(u8::try_from(packet.len() % 255).unwrap());
    let mut page = b"OggS".to_vec();
    page.push(0); // stream structure version
    page.push(header_type); // 0x02 begins the stream
    page.extend_from_slice(&0u64.to_le_bytes()); // granule position
    page.extend_from_slice(&1u32.to_le_bytes()); // stream serial number
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]); // CRC, filled in below
    page.push(u8::try_from(lacing.len()).unwrap());
    page.extend_from_slice(&lacing);
    page.extend_from_slice(packet);
    let mut crc = 0u32;
    for &byte in &page {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x04C1_1DB7
            };
        }
    }
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    page
}

/// An Ogg Opus stream of headers only: `OpusHead`, then `OpusTags` with a
/// vendor string and no comments.
fn silent_opus() -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(2); // channels
    head.extend_from_slice(&312u16.to_le_bytes()); // pre-skip
    head.extend_from_slice(&48_000u32.to_le_bytes()); // input sample rate
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&6u32.to_le_bytes());
    tags.extend_from_slice(b"uguisu");
    tags.extend_from_slice(&0u32.to_le_bytes()); // no comments
    [ogg_page(0x02, 0, &head), ogg_page(0, 1, &tags)].concat()
}

/// An Ogg Vorbis stream of headers only: identification, comments, and a
/// setup header that holds no codebooks, so it decodes no audio.
fn silent_vorbis() -> Vec<u8> {
    let mut ident = b"\x01vorbis".to_vec();
    ident.extend_from_slice(&0u32.to_le_bytes()); // version
    ident.push(2); // channels
    ident.extend_from_slice(&44_100u32.to_le_bytes()); // sample rate
    ident.extend_from_slice(&0i32.to_le_bytes()); // maximum bitrate
    ident.extend_from_slice(&128_000i32.to_le_bytes()); // nominal bitrate
    ident.extend_from_slice(&0i32.to_le_bytes()); // minimum bitrate
    ident.push(0xB8); // block sizes 2^8 and 2^11
    ident.push(1); // framing
    let mut comments = b"\x03vorbis".to_vec();
    comments.extend_from_slice(&6u32.to_le_bytes());
    comments.extend_from_slice(b"uguisu");
    comments.extend_from_slice(&0u32.to_le_bytes()); // no comments
    comments.push(1); // framing
    [
        ogg_page(0x02, 0, &ident),
        ogg_page(0, 1, &comments),
        ogg_page(0, 2, b"\x05vorbis"),
    ]
    .concat()
}

/// A one-pixel PNG, so the cover-art path runs over bytes a real decoder
/// would accept.
fn tiny_png() -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    // IHDR: 1x1, 8-bit greyscale.
    out.extend_from_slice(&[0, 0, 0, 13]);
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]);
    out.extend_from_slice(&[0x3A, 0x7E, 0x9B, 0x55]);
    out.extend_from_slice(&[0, 0, 0, 0x0A]);
    out.extend_from_slice(b"IDAT");
    out.extend_from_slice(&[0x78, 0x9C, 0x63, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01]);
    out.extend_from_slice(&[0x0D, 0x0A, 0x2D, 0xB4]);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
    out
}

fn write_fixture(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn episode_tags() -> TagSet {
    let mut set = TagSet::default();
    set.set(Field::Title, "Folge 12: Grüße aus Köln");
    set.set(Field::Album, "Uguisu Test Show");
    set.set(Field::Artist, "Die Autorin");
    set.set(Field::AlbumArtist, "Uguisu");
    set.set(Field::RecordingDate, "2024-01-05");
    set.set(Field::TrackNumber, "12");
    set.set(Field::Language, "de");
    set
}

#[test]
fn tags_round_trip_through_an_mp3() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "silence.mp3", &silent_mp3());

    let outcome = write_tags(&path, &episode_tags(), TagMode::FillMissing).unwrap();
    assert_eq!(outcome.state, OutcomeState::Written, "{outcome:?}");
    assert_eq!(outcome.format, Some("mp3"));
    assert_eq!(outcome.written.len(), 7);
    assert!(
        outcome.not_embeddable.is_empty(),
        "ID3v2 has a frame for every field Uguisu manages: {:?}",
        outcome.not_embeddable
    );

    let read = read_tags(&path).unwrap();
    assert_eq!(
        read.values.get(&Field::Title).map(String::as_str),
        Some("Folge 12: Grüße aus Köln"),
        "text survives the round trip unmangled"
    );
    assert_eq!(
        read.values.get(&Field::Album).map(String::as_str),
        Some("Uguisu Test Show")
    );
    assert_eq!(
        read.values.get(&Field::TrackNumber).map(String::as_str),
        Some("12")
    );

    // The audio is still there: tagging adds a header, it does not rewrite
    // the stream.
    let after = std::fs::read(&path).unwrap();
    assert!(
        after.windows(4).any(|w| w == [0xFF, 0xFB, 0x90, 0x00]),
        "the MPEG frames survived"
    );
    assert!(after.len() > silent_mp3().len());
}

#[test]
fn ogg_opus_and_mp4_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    // The Ogg streams get one audio page after their headers (0x04 ends the
    // stream).
    for (name, format, bytes) in [
        (
            "ogg",
            "ogg",
            [silent_vorbis(), ogg_page(0x04, 3, AUDIO)].concat(),
        ),
        (
            "opus",
            "opus",
            [silent_opus(), ogg_page(0x04, 2, AUDIO)].concat(),
        ),
        ("m4a", "mp4", m4a_with_audio()),
    ] {
        let path = write_fixture(dir.path(), &format!("silence.{name}"), &bytes);

        let outcome = write_tags(&path, &episode_tags(), TagMode::FillMissing).unwrap();
        assert_eq!(outcome.state, OutcomeState::Written, "{name}: {outcome:?}");
        assert_eq!(outcome.format, Some(format));

        let read = read_tags(&path).unwrap();
        for field in [
            Field::Title,
            Field::Album,
            Field::Artist,
            Field::TrackNumber,
        ] {
            assert_eq!(
                read.values.get(&field),
                episode_tags().values.get(&field),
                "{name}: `{field}`"
            );
        }
        let after = std::fs::read(&path).unwrap();
        assert!(
            after.windows(AUDIO.len()).any(|w| w == AUDIO),
            "{name}: the audio survived"
        );
    }
}

#[test]
fn mp4_chunk_offset_follows_the_audio() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "silence.m4a", &m4a_with_audio());

    write_tags(&path, &episode_tags(), TagMode::Sync).unwrap();

    let after = std::fs::read(&path).unwrap();
    let offset = first_chunk_offset(&after);
    assert!(
        offset > first_chunk_offset(&m4a_with_audio()),
        "the tag grew `moov`, so `mdat` moved"
    );
    assert_eq!(
        after.get(offset..offset + AUDIO.len()),
        Some(AUDIO),
        "`stco` points at the audio, not at where it used to be"
    );
}

#[test]
fn a_second_identical_write_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "silence.mp3", &silent_mp3());
    write_tags(&path, &episode_tags(), TagMode::Sync).unwrap();
    let after_first = std::fs::read(&path).unwrap();

    let outcome = write_tags(&path, &episode_tags(), TagMode::Sync).unwrap();
    assert_eq!(outcome.state, OutcomeState::NothingToWrite, "{outcome:?}");
    assert!(outcome.written.is_empty());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        after_first,
        "the file was not opened for writing at all, so its hash cannot move"
    );
}

#[test]
fn fill_missing_never_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "silence.mp3", &silent_mp3());

    // What the publisher shipped.
    let mut published = TagSet::default();
    published.set(Field::Title, "Publisher's own title");
    write_tags(&path, &published, TagMode::Sync).unwrap();

    let outcome = write_tags(&path, &episode_tags(), TagMode::FillMissing).unwrap();
    assert_eq!(outcome.state, OutcomeState::Written);
    assert!(
        !outcome.written.contains(&Field::Title),
        "a value that is already there is never replaced"
    );
    assert!(outcome.written.contains(&Field::Album));
    let read = read_tags(&path).unwrap();
    assert_eq!(
        read.values.get(&Field::Title).map(String::as_str),
        Some("Publisher's own title")
    );
    assert_eq!(
        read.values.get(&Field::Album).map(String::as_str),
        Some("Uguisu Test Show"),
        "the gap was filled"
    );

    // `sync` is the mode that does replace it.
    let outcome = write_tags(&path, &episode_tags(), TagMode::Sync).unwrap();
    assert!(outcome.written.contains(&Field::Title));
    assert_eq!(
        read_tags(&path)
            .unwrap()
            .values
            .get(&Field::Title)
            .map(String::as_str),
        Some("Folge 12: Grüße aus Köln")
    );
}

#[test]
fn unmanaged_fields_are_untouched() {
    use lofty::config::WriteOptions;
    use lofty::prelude::ItemKey;
    use lofty::tag::{Tag, TagExt, TagType};

    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "silence.mp3", &silent_mp3());

    // Something Uguisu has no opinion about whatsoever.
    let mut foreign = Tag::new(TagType::Id3v2);
    assert!(foreign.insert_text(ItemKey::Composer, "Eine Komponistin".into()));
    assert!(foreign.insert_text(ItemKey::Comment, "Der Kommentar des Verlags".into()));
    assert!(foreign.insert_text(ItemKey::TrackTitle, "Publisher's own title".into()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    foreign.save_to(&mut file, WriteOptions::default()).unwrap();
    drop(file);

    // The premise, checked rather than assumed: the foreign frames really
    // are in the file before Uguisu touches it.
    let before = read_back(&path);
    assert_eq!(before.0.as_deref(), Some("Eine Komponistin"));
    assert_eq!(before.1.as_deref(), Some("Der Kommentar des Verlags"));

    // `sync` is the most aggressive mode there is, and it still leaves
    // every unmanaged frame exactly as it found it.
    write_tags(&path, &episode_tags(), TagMode::Sync).unwrap();

    let after = read_back(&path);
    assert_eq!(
        after, before,
        "every frame Uguisu does not manage came through unchanged"
    );
    assert_eq!(
        read_tags(&path)
            .unwrap()
            .values
            .get(&Field::Title)
            .map(String::as_str),
        Some("Folge 12: Grüße aus Köln"),
        "...and the managed one was updated"
    );
}

/// The unmanaged values, as the file currently holds them.
fn read_back(path: &Path) -> (Option<String>, Option<String>) {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::ItemKey;

    let tagged = lofty::probe::Probe::open(path)
        .unwrap()
        .guess_file_type()
        .unwrap()
        .read()
        .unwrap();
    let Some(tag) = tagged.primary_tag() else {
        return (None, None);
    };
    (
        tag.get_string(ItemKey::Composer).map(ToOwned::to_owned),
        tag.get_string(ItemKey::Comment).map(ToOwned::to_owned),
    )
}

#[test]
fn cover_art_must_be_an_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "silence.mp3", &silent_mp3());

    let mut with_cover = episode_tags();
    with_cover.cover = Some(Artwork {
        mime: "image/png".to_owned(),
        bytes: tiny_png(),
    });
    let outcome = write_tags(&path, &with_cover, TagMode::FillMissing).unwrap();
    assert!(outcome.cover_written, "{outcome:?}");
    let read = read_tags(&path).unwrap();
    let cover = read.cover.expect("the picture came back");
    assert_eq!(cover.bytes, tiny_png());
    assert_eq!(cover.mime, "image/png");

    // Bytes that are not an image are refused rather than embedded: an
    // audio file is something other people's players parse.
    let other = write_fixture(dir.path(), "second.mp3", &silent_mp3());
    let bogus = TagSet {
        cover: Some(Artwork {
            mime: "image/png".to_owned(),
            bytes: b"<!DOCTYPE html><html>not a picture".to_vec(),
        }),
        ..TagSet::default()
    };
    let err = write_tags(&other, &bogus, TagMode::FillMissing).unwrap_err();
    assert!(err.to_string().contains("cover art was refused"), "{err}");
    assert_eq!(
        std::fs::read(&other).unwrap(),
        silent_mp3(),
        "and the file is untouched"
    );
}

#[test]
fn cover_is_written_once_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let set = TagSet {
        cover: Some(Artwork {
            mime: "image/png".to_owned(),
            bytes: tiny_png(),
        }),
        ..TagSet::default()
    };
    for (name, bytes) in [
        ("mp3", silent_mp3()),
        ("flac", silent_flac()),
        ("m4a", silent_m4a()),
        ("ogg", silent_vorbis()),
        ("opus", silent_opus()),
    ] {
        let path = write_fixture(dir.path(), &format!("cover.{name}"), &bytes);
        let outcome = write_tags(&path, &set, TagMode::Sync).unwrap();
        assert!(outcome.cover_written, "{name}: {outcome:?}");
        let cover = read_tags(&path).unwrap().cover;
        assert_eq!(cover.map(|c| c.bytes), Some(tiny_png()), "{name}");
        // A cover the file already holds is the one `sync` and
        // `fill_missing` find, or each run would add another picture and
        // move the archive's hash.
        for mode in [TagMode::Sync, TagMode::FillMissing] {
            assert_eq!(
                write_tags(&path, &set, mode).unwrap().state,
                OutcomeState::NothingToWrite,
                "{name}: {mode:?} wrote the cover again"
            );
        }
    }
}

#[test]
fn flac_takes_what_vorbis_comments_can_hold() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "silence.flac", &silent_flac());

    let mut set = episode_tags();
    set.set(Field::EpisodeGuid, "guid:abc");
    let outcome = write_tags(&path, &set, TagMode::Sync).unwrap();
    assert_eq!(outcome.state, OutcomeState::Written, "{outcome:?}");
    assert_eq!(outcome.format, Some("flac"));
    assert!(outcome.written.contains(&Field::Title));

    let read = read_tags(&path).unwrap();
    assert_eq!(
        read.values.get(&Field::Title).map(String::as_str),
        Some("Folge 12: Grüße aus Köln")
    );
    assert_eq!(
        read.values.get(&Field::Album).map(String::as_str),
        Some("Uguisu Test Show")
    );
}

#[test]
fn an_untaggable_container_is_a_result() {
    let dir = tempfile::tempdir().unwrap();
    // A RIFF/WAVE file: `lofty` could write `RiffInfo` here, and Uguisu
    // declines because those chunks hold none of the podcast fields.
    let mut wav = b"RIFF".to_vec();
    wav.extend_from_slice(&36u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&[1, 0, 1, 0]);
    wav.extend_from_slice(&44100u32.to_le_bytes());
    wav.extend_from_slice(&88200u32.to_le_bytes());
    wav.extend_from_slice(&[2, 0, 16, 0]);
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&0u32.to_le_bytes());
    let path = write_fixture(dir.path(), "silence.wav", &wav);

    let outcome = write_tags(&path, &episode_tags(), TagMode::Sync).unwrap();
    assert_eq!(outcome.state, OutcomeState::Unsupported, "{outcome:?}");
    assert_eq!(outcome.format, Some("wav"));
    assert!(outcome.written.is_empty());
    assert_eq!(
        outcome.not_embeddable.len(),
        7,
        "everything is reported so the caller can keep it in the sidecar"
    );
    assert!(outcome.detail.unwrap().contains("no player reads them"));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        wav,
        "an unsupported container is left exactly as it was"
    );
}

#[test]
fn every_managed_field_can_actually_be_written() {
    // The guard that matters: a field in the table that the encoder
    // refuses does not fail one value, it fails the whole tag write and
    // takes every other field with it. ID3v2's `PCST` flag did exactly
    // that, which is why it is not in the table.
    use lofty::tag::TagType;
    use uguisu_metadata::capability::{self, Support};

    let dir = tempfile::tempdir().unwrap();
    for (name, bytes, tag_type) in [
        ("mp3", silent_mp3(), TagType::Id3v2),
        ("flac", silent_flac(), TagType::VorbisComments),
        ("m4a", silent_m4a(), TagType::Mp4Ilst),
        ("ogg", silent_vorbis(), TagType::VorbisComments),
        ("opus", silent_opus(), TagType::VorbisComments),
    ] {
        for field in Field::ALL {
            if capability::support(tag_type, field) == Support::Unsupported {
                // Declared, not discovered: the table already says this
                // format cannot hold it, and the caller keeps it in the
                // sidecar.
                continue;
            }
            let path = write_fixture(
                dir.path(),
                &format!("{}-{name}.{name}", field.as_str()),
                &bytes,
            );
            let value = sample_value(field);
            let mut set = TagSet::default();
            set.set(field, value);
            let outcome = write_tags(&path, &set, TagMode::Sync)
                .unwrap_or_else(|e| panic!("{name}: `{field}` could not be written: {e}"));
            assert_eq!(
                outcome.state,
                OutcomeState::Written,
                "{name}: `{field}` was not written"
            );
            // Round trip: `sync` decides by comparing what it wants
            // against what the file reports, so a value that comes back
            // different is rewritten on every run - and every rewrite
            // moves the archive's hash.
            assert_eq!(
                read_tags(&path)
                    .unwrap()
                    .values
                    .get(&field)
                    .map(String::as_str),
                Some(value),
                "{name}: `{field}` did not survive a write-then-read"
            );
            assert_eq!(
                write_tags(&path, &set, TagMode::Sync).unwrap().state,
                OutcomeState::NothingToWrite,
                "{name}: writing `{field}` again would move the file's hash"
            );
        }
    }
}

/// A value of the shape each field really carries.
fn sample_value(field: Field) -> &'static str {
    match field {
        Field::RecordingDate => "2024-01-05",
        Field::TrackNumber | Field::DiscNumber => "2",
        Field::Language => "de",
        _ => "Ein Wert",
    }
}

#[test]
fn a_non_audio_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "notes.mp3", b"just some text, honestly");
    assert!(read_tags(&path).is_err());
    assert!(write_tags(&path, &episode_tags(), TagMode::Sync).is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"just some text, honestly",
        "nothing was written to it"
    );
}

#[test]
fn identity_reads_title_and_guid() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "episode.mp3", &silent_mp3());
    let mut tags = episode_tags();
    tags.set(Field::EpisodeGuid, "urn:uuid:5f0e-episode-12");
    tags.cover = Some(Artwork {
        mime: "image/png".to_owned(),
        bytes: tiny_png(),
    });
    write_tags(&path, &tags, TagMode::Sync).unwrap();
    let identity = read_identity(&path).unwrap();
    assert_eq!(identity.title.as_deref(), Some("Folge 12: Grüße aus Köln"));
    assert_eq!(
        identity.episode_guid.as_deref(),
        Some("urn:uuid:5f0e-episode-12")
    );
}

#[test]
fn identity_reads_audio_duration() {
    let dir = tempfile::tempdir().unwrap();
    // Eighty frames of 1152 samples at 44.1 kHz: 2.09 seconds.
    let path = write_fixture(dir.path(), "untagged.mp3", &silent_mp3().repeat(4));
    assert_eq!(
        read_identity(&path).unwrap(),
        Identity {
            title: None,
            episode_guid: None,
            duration_secs: Some(2),
        }
    );
    // Ten frames are a quarter of a second: no length worth matching on.
    let short = write_fixture(dir.path(), "short.mp3", &silent_mp3()[..417 * 10]);
    assert_eq!(read_identity(&short).unwrap().duration_secs, None);
}

#[test]
fn identity_of_text_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(
        dir.path(),
        "notes.mp3",
        b"ID3\x04 an import fixture, not audio",
    );
    assert!(read_identity(&path).is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"ID3\x04 an import fixture, not audio"
    );
}
