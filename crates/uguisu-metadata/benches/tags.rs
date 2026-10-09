//! Reading and writing tags.
//!
//! Tag work happens once per file, never per byte of audio, so every
//! number here is per document. The fixtures are built in code for the
//! same reason the tests build theirs: a hand-written MP3 frame is
//! auditable, a checked-in binary blob is not.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use uguisu_core::archive::TagMode;
use uguisu_metadata::{Field, TagSet, read_tags, write_tags};

/// A valid MPEG-1 Layer III stream of silence: 20 frames of 417 bytes.
fn silent_mp3() -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..20 {
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        out.extend(std::iter::repeat_n(0u8, 413));
    }
    out
}

/// A FLAC file with a `STREAMINFO` block and no audio frames.
fn silent_flac() -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
    out.extend_from_slice(&[0x10, 0x00]);
    out.extend_from_slice(&[0x10, 0x00]);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(&[10, 196, 66, 240, 0, 0, 0, 0]);
    out.extend_from_slice(&[0u8; 16]);
    out
}

fn episode_tags() -> TagSet {
    let mut set = TagSet::default();
    set.set(Field::Title, "Folge 12: Grüße aus Köln");
    set.set(Field::Album, "Uguisu Test Show");
    set.set(Field::Artist, "Die Autorin");
    set.set(Field::AlbumArtist, "Uguisu");
    set.set(Field::Publisher, "Uguisu");
    set.set(
        Field::Description,
        "Eine Folge über Pizza, Käse und Ketchup.",
    );
    set.set(Field::Genre, "Podcast");
    set.set(Field::RecordingDate, "2024-01-05");
    set.set(Field::TrackNumber, "12");
    set.set(Field::DiscNumber, "2");
    set.set(Field::Language, "de");
    set.set(Field::Copyright, "© 2024 Uguisu");
    set.set(Field::EpisodeGuid, "urn:uguisu:episode:12");
    set
}

fn bench_tags(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let desired = episode_tags();

    let mut group = c.benchmark_group("tags");
    group.throughput(Throughput::Elements(1));

    for (name, bytes) in [("mp3", silent_mp3()), ("flac", silent_flac())] {
        // A file that already carries every managed value: this is the
        // second `sync` run, the one that must find nothing to do.
        let tagged = dir.path().join(format!("tagged.{name}"));
        std::fs::write(&tagged, &bytes).unwrap();
        write_tags(&tagged, &desired, TagMode::Sync).unwrap();

        group.bench_function(format!("read/{name}"), |b| {
            b.iter(|| read_tags(black_box(&tagged)).unwrap());
        });

        // Writing every managed field into a file that has none. The copy
        // is part of the measurement because the engine never tags an
        // original in place - it tags a copy and renames it over.
        let scratch = dir.path().join(format!("scratch.{name}"));
        group.bench_function(format!("write/{name}"), |b| {
            b.iter(|| {
                std::fs::write(&scratch, &bytes).unwrap();
                write_tags(black_box(&scratch), black_box(&desired), TagMode::Sync).unwrap()
            });
        });

        // The no-op path: every managed field already agrees, so nothing
        // is opened for writing and the file's hash cannot move.
        group.bench_function(format!("write_noop/{name}"), |b| {
            b.iter(|| write_tags(black_box(&tagged), black_box(&desired), TagMode::Sync).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_tags);
criterion_main!(benches);
