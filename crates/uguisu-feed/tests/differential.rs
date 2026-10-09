//! Differential tests against `feed-rs` (ADR 0008): for the fields both
//! parsers model — item count, titles, GUIDs, publication dates and
//! enclosure URLs — Uguisu must agree with the reference parser on the
//! corpus and on the recorded live feeds. Documented tolerances are listed
//! next to each comparison.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use time::OffsetDateTime;
use uguisu_core::config::FeedLimits;
use uguisu_feed::normalize::parse_date_detailed;
use uguisu_feed::parse;

fn read(path: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn live_feed(host: &str, case: &str) -> Vec<u8> {
    let json: serde_json::Value = serde_json::from_slice(&read(&format!(
        "tests/fixtures/discovery/live/web/{host}/{case}.json"
    )))
    .unwrap();
    json["response"]["body"]
        .as_str()
        .unwrap()
        .as_bytes()
        .to_vec()
}

/// Feeds where both parsers must produce the same well-formed item list.
fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for name in [
        "minimal_rss.xml",
        "minimal_atom.xml",
        "missing_guid.xml",
        "duplicate_guid.xml",
        "dates.xml",
        "enclosures.xml",
        "durations.xml",
        "unicode_heavy.xml",
        "podcasting20.xml",
        "new_feed_url.xml",
        "odd_prefixes.xml",
        "episodes_v1.xml",
        "episodes_v2.xml",
        "guid_changed.xml",
        "duplicate_episodes.xml",
    ] {
        out.push((
            name.to_owned(),
            read(&format!("tests/fixtures/feeds/parser/{name}")),
        ));
    }
    for (host, case) in [
        ("anchor_fm", "anchor_feed"),
        ("rss_buzzsprout_com", "buzzsprout_feed"),
        ("feeds_feedburner_com", "feedburner_feed"),
        ("feeds_twit_tv", "twit_feed"),
        ("rss_libsyn_com", "libsyn_feed"),
    ] {
        out.push((case.to_owned(), live_feed(host, case)));
    }
    out
}

#[test]
fn agrees_with_feed_rs_on_common_fields() {
    let now = OffsetDateTime::now_utc();
    for (name, bytes) in corpus() {
        let ours = parse(&bytes, &FeedLimits::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let theirs = feed_rs::parser::parse(bytes.as_slice())
            .unwrap_or_else(|e| panic!("{name}: feed-rs {e}"));
        assert_eq!(ours.items.len(), theirs.entries.len(), "{name}: item count");
        assert_eq!(
            ours.channel.title.as_deref().map(str::trim),
            theirs.title.as_ref().map(|t| t.content.trim()),
            "{name}: channel title"
        );
        for (i, (a, b)) in ours.items.iter().zip(theirs.entries.iter()).enumerate() {
            // Titles: feed-rs decodes entities and keeps CDATA literally, as Uguisu does.
            assert_eq!(
                a.title.as_deref().map(str::trim),
                b.title.as_ref().map(|t| t.content.trim()),
                "{name} item {i}: title"
            );
            // GUIDs: feed-rs synthesises an id when the item has none
            // (tolerance: compare only when Uguisu saw a guid element).
            if let Some(g) = &a.guid {
                assert_eq!(g.trim(), b.id.trim(), "{name} item {i}: guid");
            }
            // Publication dates: compare instants; feed-rs returns None for
            // dates it cannot parse, Uguisu keeps the raw string with
            // quality Invalid (tolerance: only compare when both parsed).
            let ours_date = a
                .pub_date
                .as_deref()
                .and_then(|raw| parse_date_detailed(raw, now).value);
            if let (Some(x), Some(y)) = (ours_date, b.published) {
                assert_eq!(
                    x.unix_timestamp(),
                    y.timestamp(),
                    "{name} item {i}: pubDate"
                );
            }
            // Enclosure URLs: feed-rs models RSS enclosures as `media` groups
            // with content entries; compare the first declared URL when both
            // sides have one (tolerance: alternateEnclosure/media:content are
            // modelled differently and not compared).
            let ours_url = a.enclosures.first().map(|e| e.url.trim().to_owned());
            let theirs_url = b
                .media
                .iter()
                .flat_map(|m| m.content.iter())
                .find_map(|c| {
                    c.url
                        .as_ref()
                        .map(|u| u.as_str().trim_end_matches('/').to_owned())
                })
                .or_else(|| {
                    b.links
                        .iter()
                        .find(|l| l.rel.as_deref() == Some("enclosure"))
                        .map(|l| l.href.trim_end_matches('/').to_owned())
                });
            if let (Some(x), Some(y)) = (ours_url, theirs_url) {
                assert_eq!(x.trim_end_matches('/'), y, "{name} item {i}: enclosure url");
            }
        }
    }
}
