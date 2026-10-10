//! The quick look a resolver takes at bytes that may be a feed: it may refuse
//! them, never panic.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = uguisu_feed::probe(data);
});
