//! An OPML file as an import reads it, and an export of the feeds it could
//! have stored read again: neither may panic, and the round trip keeps
//! every feed.
#![no_main]

use libfuzzer_sys::fuzz_target;
use uguisu_feed::opml;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(outlines) = opml::parse(text) else {
        return;
    };
    // An export lists stored feed URLs, which went through a URL parser:
    // the characters `write` leaves out, because XML cannot carry them,
    // never reach one.
    let stored: Vec<opml::Outline> = outlines
        .into_iter()
        .filter_map(|o| {
            let url = url::Url::parse(&o.xml_url).ok()?;
            matches!(url.scheme(), "http" | "https").then(|| opml::Outline {
                xml_url: url.to_string(),
                ..o
            })
        })
        .collect();
    let written = opml::write("fuzz", &stored);
    // Escaping can grow a document past what an import reads at all.
    if written.len() > opml::MAX_BYTES {
        return;
    }
    let again = opml::parse(&written).expect("an export must import");
    let urls = |list: &[opml::Outline]| list.iter().map(|o| o.xml_url.clone()).collect::<Vec<_>>();
    assert_eq!(urls(&again), urls(&stored), "the round trip lost or changed a feed");
});
