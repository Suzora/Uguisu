//! The tags of a file Uguisu did not write, as an import reads them: any
//! bytes in a file may be refused, never panic.
#![no_main]

use std::io::Write as _;

use libfuzzer_sys::fuzz_target;

// uguisu-metadata turns a panic inside `lofty` into that file's error, and
// libFuzzer's hook would abort before it could. A panic nothing catches still
// ends in libFuzzer's abort.
fuzz_target!(init: drop(std::panic::take_hook()), |data: &[u8]| {
    // The reader opens a path, so the bytes go through a file.
    let mut file = tempfile::NamedTempFile::new().expect("a scratch file");
    file.write_all(data).expect("the bytes written");
    let _ = uguisu_metadata::read_tags(file.path());
});
