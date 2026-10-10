//! Writes the starting corpus of every fuzz target into the directory given:
//! `<dir>/<target>/<name>`.
//!
//! The containers are the ones `crates/uguisu-metadata/tests/tags.rs` builds
//! by hand, each byte explained there: a fuzzer that starts from a valid
//! file reaches the tag code, while one that starts from nothing spends its
//! time on the magic number. Each is written once bare and once with every
//! field Uguisu manages, through Uguisu's own writer.

use std::path::Path;

use uguisu_core::archive::TagMode;
use uguisu_metadata::{Artwork, Field, TagSet, write_tags};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: uguisu-fuzz-seeds <corpus directory>");
    let dir = Path::new(&dir);
    let audio: [(&str, Vec<u8>); 5] = [
        ("silent.mp3", silent_mp3()),
        ("silent.flac", silent_flac()),
        ("silent.m4a", silent_m4a()),
        ("silent.ogg", silent_vorbis()),
        ("silent.opus", silent_opus()),
    ];
    let scratch = dir.join(".tagging");
    std::fs::create_dir_all(&scratch).expect("the scratch directory");
    for target in ["tags_read", "identity_read", "tags_write"] {
        let into = dir.join(target);
        std::fs::create_dir_all(&into).expect("the target's corpus directory");
        for (name, bytes) in &audio {
            std::fs::write(into.join(name), bytes).expect("a bare seed");
            let copy = scratch.join(name);
            std::fs::write(&copy, bytes).expect("a copy to tag");
            write_tags(&copy, &every_field(), TagMode::Sync).expect("Uguisu tags its own seed");
            std::fs::copy(&copy, into.join(format!("tagged-{name}"))).expect("a tagged seed");
        }
    }
    std::fs::remove_dir_all(&scratch).expect("the scratch directory removed");

    for target in ["feed_parse", "feed_probe"] {
        let into = dir.join(target);
        std::fs::create_dir_all(&into).expect("the target's corpus directory");
        std::fs::write(into.join("rss.xml"), RSS).expect("a feed seed");
        std::fs::write(into.join("atom.xml"), ATOM).expect("a feed seed");
    }
    let into = dir.join("opml_parse");
    std::fs::create_dir_all(&into).expect("the target's corpus directory");
    std::fs::write(into.join("nested.opml"), OPML).expect("an OPML seed");
}

fn every_field() -> TagSet {
    let mut tags = TagSet::default();
    for field in Field::ALL {
        tags.set(field, format!("{} — Grüße 7", field.as_str()));
    }
    tags.set(Field::RecordingDate, "2026-10-10");
    tags.set(Field::TrackNumber, "7");
    tags.set(Field::DiscNumber, "1");
    tags.set(Field::Language, "de");
    tags.cover = Some(Artwork {
        mime: "image/png".to_owned(),
        bytes: tiny_png(),
    });
    tags
}

const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:podcast="https://podcastindex.org/namespace/1.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
<channel><title>Seed &amp; Show</title><link>https://seed.example/</link><description>One</description>
<itunes:image href="https://seed.example/cover.png"/><podcast:guid>8f2b1e2a-0000-4000-8000-000000000000</podcast:guid>
<item><title>Folge 1: Grüße</title><guid isPermaLink="false">s-1</guid><pubDate>Sat, 10 Oct 2026 10:00:00 +0200</pubDate>
<description><![CDATA[<p>Notes</p>]]></description><content:encoded><![CDATA[<b>more</b>]]></content:encoded>
<itunes:duration>1:02:03</itunes:duration><itunes:episode>1</itunes:episode><podcast:chapters url="https://seed.example/1.json" type="application/json+chapters"/>
<enclosure url="https://seed.example/1.mp3" length="12345" type="audio/mpeg"/></item>
</channel></rss>
"#;

const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Seed</title><id>urn:seed</id><updated>2026-10-10T10:00:00Z</updated>
<entry><title>One</title><id>urn:seed:1</id><updated>2026-10-10T10:00:00Z</updated>
<link rel="enclosure" href="https://seed.example/1.ogg" type="audio/ogg" length="99"/></entry></feed>
"#;

const OPML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0"><head><title>Seeds</title></head><body>
<outline text="Folder"><outline type="rss" text="Grüße &amp; mehr" xmlUrl="https://seed.example/feed?a=1&amp;b=2" htmlUrl="https://seed.example/"/></outline>
<outline type="rss" title="Two" xmlUrl="http://seed.example/two.xml"/>
</body></opml>
"#;

/// MPEG-1 Layer III silence, twenty 417-byte frames.
fn silent_mp3() -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..20 {
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        out.extend(std::iter::repeat_n(0u8, 413));
    }
    out
}

/// `fLaC` and one `STREAMINFO` block, no frames.
fn silent_flac() -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
    out.extend_from_slice(&[0x10, 0x00, 0x10, 0x00]);
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&[10, 196, 66, 240, 0, 0, 0, 0]);
    out.extend_from_slice(&[0u8; 16]);
    out
}

fn atom(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(8 + body.len()).expect("a small atom").to_be_bytes().to_vec();
    out.extend_from_slice(&kind);
    out.extend_from_slice(body);
    out
}

/// `ftyp`, a `moov` holding only `mvhd`, and an empty `mdat`.
fn silent_m4a() -> Vec<u8> {
    let ftyp = atom(*b"ftyp", b"M4A \0\0\0\0M4A isom");
    let mut mvhd = vec![0u8; 12];
    mvhd.extend_from_slice(&1000u32.to_be_bytes());
    mvhd.extend_from_slice(&0u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0; 10]);
    for value in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        mvhd.extend_from_slice(&value.to_be_bytes());
    }
    mvhd.extend_from_slice(&[0; 24]);
    mvhd.extend_from_slice(&1u32.to_be_bytes());
    [ftyp, atom(*b"moov", &atom(*b"mvhd", &mvhd)), atom(*b"mdat", &[])].concat()
}

/// One Ogg page carrying one whole packet, with the format's CRC.
fn ogg_page(header_type: u8, sequence: u32, packet: &[u8]) -> Vec<u8> {
    let mut lacing = vec![255u8; packet.len() / 255];
    lacing.push(u8::try_from(packet.len() % 255).expect("a remainder below 255"));
    let mut page = b"OggS".to_vec();
    page.push(0);
    page.push(header_type);
    page.extend_from_slice(&0u64.to_le_bytes());
    page.extend_from_slice(&1u32.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(u8::try_from(lacing.len()).expect("a short packet"));
    page.extend_from_slice(&lacing);
    page.extend_from_slice(packet);
    let mut crc = 0u32;
    for &byte in &page {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 == 0 { crc << 1 } else { (crc << 1) ^ 0x04C1_1DB7 };
        }
    }
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    page
}

/// `OpusHead` and an empty `OpusTags`.
fn silent_opus() -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(&[1, 2]);
    head.extend_from_slice(&312u16.to_le_bytes());
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&6u32.to_le_bytes());
    tags.extend_from_slice(b"uguisu");
    tags.extend_from_slice(&0u32.to_le_bytes());
    [ogg_page(0x02, 0, &head), ogg_page(0, 1, &tags)].concat()
}

/// Vorbis identification, comment and an empty setup header.
fn silent_vorbis() -> Vec<u8> {
    let mut ident = b"\x01vorbis".to_vec();
    ident.extend_from_slice(&0u32.to_le_bytes());
    ident.push(2);
    ident.extend_from_slice(&44_100u32.to_le_bytes());
    ident.extend_from_slice(&0i32.to_le_bytes());
    ident.extend_from_slice(&128_000i32.to_le_bytes());
    ident.extend_from_slice(&0i32.to_le_bytes());
    ident.extend_from_slice(&[0xB8, 1]);
    let mut comments = b"\x03vorbis".to_vec();
    comments.extend_from_slice(&6u32.to_le_bytes());
    comments.extend_from_slice(b"uguisu");
    comments.extend_from_slice(&0u32.to_le_bytes());
    comments.push(1);
    [ogg_page(0x02, 0, &ident), ogg_page(0, 1, &comments), ogg_page(0, 2, b"\x05vorbis")].concat()
}

/// A 1x1 greyscale PNG.
fn tiny_png() -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
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
