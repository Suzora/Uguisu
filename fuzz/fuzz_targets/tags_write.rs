//! Writing Uguisu's tags into a file it did not make, as `archive tags write`
//! does to its scratch copy: never a panic, and after a write that counted, a
//! second `sync` write of the same values changes nothing, or every run would
//! move the archive's hash.
#![no_main]

use std::io::Write as _;

use libfuzzer_sys::fuzz_target;
use uguisu_core::archive::TagMode;
use uguisu_metadata::{Field, OutcomeState, TagSet, write_tags};

// uguisu-metadata turns a panic inside `lofty` into that file's error, and
// libFuzzer's hook would abort before it could. A panic nothing catches still
// ends in libFuzzer's abort.
fuzz_target!(init: drop(std::panic::take_hook()), |data: &[u8]| {
    // The writer works on a path, as the engine's copy-tag-rename does.
    let mut file = tempfile::NamedTempFile::new().expect("a scratch file");
    file.write_all(data).expect("the bytes written");
    let mut desired = TagSet::default();
    desired.set(Field::Title, "Folge 7: Grüße aus Köln");
    desired.set(Field::Artist, "Uguisu");
    desired.set(Field::Album, "Fuzz");
    desired.set(Field::RecordingDate, "2026-10-10");
    desired.set(Field::TrackNumber, "7");
    let Ok(first) = write_tags(file.path(), &desired, TagMode::Sync) else {
        return;
    };
    if first.state != OutcomeState::Written {
        return;
    }
    let second = write_tags(file.path(), &desired, TagMode::Sync).expect("a file just written takes a second write");
    assert_eq!(second.state, OutcomeState::NothingToWrite, "a second sync write changed the file");
});
