//! Parser tests on the hand-made corpus (`tests/fixtures/feeds/parser`),
//! the probe fixtures and the recorded live feeds.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use uguisu_core::config::FeedLimits;
use uguisu_core::model::FeedKind;
use uguisu_feed::{EnclosureOrigin, ParseError, ParsedFeed, parse};

fn corpus(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../tests/fixtures/feeds/parser/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn probe_fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../tests/fixtures/feeds/probe/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Body of a recorded live feed (`tests/fixtures/discovery/live/web/<host>/<case>.json`).
fn live_feed(host: &str, case: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../tests/fixtures/discovery/live/web/{host}/{case}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["response"]["body"]
        .as_str()
        .unwrap()
        .as_bytes()
        .to_vec()
}

fn ok(name: &str) -> ParsedFeed {
    parse(&corpus(name), &FeedLimits::default()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn minimal_rss_and_atom() {
    let f = ok("minimal_rss.xml");
    assert_eq!(f.kind, FeedKind::Rss2);
    assert_eq!(f.channel.title.as_deref(), Some("Minimal Show"));
    assert_eq!(f.channel.link.as_deref(), Some("https://minimal.example/"));
    assert_eq!(f.items.len(), 1);
    let i = &f.items[0];
    assert_eq!(i.title.as_deref(), Some("Only Episode"));
    assert_eq!(i.guid.as_deref(), Some("ep-1"));
    assert_eq!(i.guid_is_permalink, None);
    assert_eq!(i.enclosures[0].url, "https://minimal.example/ep1.mp3");
    assert_eq!(i.enclosures[0].length.as_deref(), Some("1234"));
    assert_eq!(i.enclosures[0].origin, EnclosureOrigin::Enclosure);
    assert!(f.looks_like_podcast());
    assert!(!f.truncated && !f.is_partial());

    let a = ok("minimal_atom.xml");
    assert_eq!(a.kind, FeedKind::Atom);
    assert_eq!(a.channel.title.as_deref(), Some("Atom Show"));
    assert_eq!(a.channel.language.as_deref(), Some("en"));
    assert_eq!(
        a.channel.self_link.as_deref(),
        Some("https://atom.example/feed.atom")
    );
    assert_eq!(a.channel.link.as_deref(), Some("https://atom.example/"));
    assert_eq!(a.channel.author.as_deref(), Some("Atom Author"));
    assert_eq!(a.channel.updated.as_deref(), Some("2025-09-01T10:00:00Z"));
    let e = &a.items[0];
    assert_eq!(e.guid.as_deref(), Some("urn:atom-ep-1"));
    assert_eq!(e.pub_date.as_deref(), Some("2025-09-01T10:00:00Z"));
    assert_eq!(e.updated.as_deref(), Some("2025-09-02T10:00:00Z"));
    assert_eq!(e.description.as_deref(), Some("Atom summary"));
    assert_eq!(e.link.as_deref(), Some("https://atom.example/ep1"));
    assert_eq!(e.enclosures.len(), 1);
    assert_eq!(e.enclosures[0].origin, EnclosureOrigin::AtomLink);
    assert_eq!(e.enclosures[0].mime_type.as_deref(), Some("audio/mp4"));
}

#[test]
fn rdf_feed_from_probe_fixtures() {
    let f = parse(&probe_fixture("rss_rdf.xml"), &FeedLimits::default()).unwrap();
    assert_eq!(f.kind, FeedKind::Rss1);
    assert_eq!(f.items.len(), 1);
    assert_eq!(f.items[0].enclosures[0].origin, EnclosureOrigin::RdfEnc);
    assert!(f.looks_like_podcast());
}

#[test]
fn guid_variants_are_kept_as_written() {
    let f = ok("missing_guid.xml");
    assert_eq!(f.items.len(), 3);
    assert!(f.items.iter().all(|i| i.guid.is_none()));
    assert_eq!(f.stats.items_with_enclosure, 2);
    let d = ok("duplicate_guid.xml");
    assert_eq!(d.items[0].guid.as_deref(), Some("same"));
    assert_eq!(d.items[1].guid.as_deref(), Some("same"));
    assert_eq!(d.items[0].guid_is_permalink, Some(false));
    assert_eq!(d.items[2].guid_is_permalink, None);
}

#[test]
fn dates_and_durations_are_raw_strings() {
    let f = ok("dates.xml");
    let raw: Vec<Option<&str>> = f.items.iter().map(|i| i.pub_date.as_deref()).collect();
    assert_eq!(raw[0], None);
    assert_eq!(raw[1], Some("yesterday-ish"));
    assert_eq!(raw[6], Some("2025-09-01T10:00:00+00:00"), "dc:date");
    assert_eq!(raw[7], Some("Mon, 01 Sep 2025 10:00 GMT"));
    let d = ok("durations.xml");
    assert_eq!(d.items[0].duration.as_deref(), Some("1:02:03"));
    assert_eq!(d.items[5].duration, None);
    assert_eq!(d.items[6].duration.as_deref(), Some("00:05:00"), "trimmed");
}

#[test]
fn enclosure_variants() {
    let f = ok("enclosures.xml");
    assert_eq!(f.items.len(), 6);
    assert!(f.items[0].enclosures.is_empty());
    let multi = &f.items[1];
    assert_eq!(multi.enclosures.len(), 3);
    assert_eq!(multi.enclosures[0].origin, EnclosureOrigin::Enclosure);
    let opus = &multi.enclosures[1];
    assert_eq!(opus.origin, EnclosureOrigin::AlternateEnclosure);
    assert_eq!(opus.url, "https://cdn.example/e2.opus");
    assert_eq!(opus.mime_type.as_deref(), Some("audio/opus"));
    assert_eq!(opus.bitrate.as_deref(), Some("64000"));
    assert_eq!(opus.sources, vec!["ipfs://Qm123"]);
    assert_eq!(opus.integrity_type.as_deref(), Some("sri"));
    assert_eq!(opus.integrity_value.as_deref(), Some("sha384-abc"));
    let video = &multi.enclosures[2];
    assert_eq!(video.mime_type.as_deref(), Some("video/mp4"));
    assert_eq!(video.height.as_deref(), Some("720"));
    assert_eq!(
        f.items[2].enclosures[0].length.as_deref(),
        Some("not-a-number")
    );
    assert_eq!(
        f.items[3].enclosures[0].origin,
        EnclosureOrigin::MediaContent
    );
    assert_eq!(f.items[3].enclosures[0].length.as_deref(), Some("777"));
    assert!(f.items[4].enclosures.is_empty());
    assert!(
        f.items[4]
            .warnings
            .iter()
            .any(|w| w.contains("without url"))
    );
    assert_eq!(
        f.items[5].enclosures[0].mime_type.as_deref(),
        Some("Audio/OGG")
    );
    assert_eq!(f.stats.items_with_enclosure, 4);
}

#[test]
fn unicode_long_titles_html_and_cdata() {
    let u = ok("unicode_heavy.xml");
    assert_eq!(
        u.channel.title.as_deref(),
        Some("Ünïcödé — 日本語のポッドキャスト 🎙️")
    );
    assert_eq!(u.channel.author.as_deref(), Some("Zoë Ångström & 李雷"));
    assert_eq!(
        u.items[0].title.as_deref(),
        Some("Épisode № 1 – „Zitat“ 🚀")
    );
    assert_eq!(u.items[0].guid.as_deref(), Some("ü-1"));

    let l = ok("very_long_title.xml");
    let title = l.items[0].title.as_ref().unwrap();
    assert!(title.len() <= FeedLimits::default().max_field_bytes);
    assert!(title.starts_with("Very long title"));

    let h = ok("html_cdata_empty.xml");
    assert_eq!(h.channel.title.as_deref(), Some("HTML & CDATA"));
    assert_eq!(
        h.channel.description.as_deref(),
        Some("<p>Show notes with <b>bold</b> &amp; entities</p>"),
        "CDATA is kept literally"
    );
    let i = &h.items[0];
    assert_eq!(i.title.as_deref(), Some("Episode with <em>markup</em>"));
    assert_eq!(i.guid, None, "empty guid is absent");
    assert_eq!(
        i.description.as_deref(),
        Some("<p>Escaped <i>HTML</i> in description</p>")
    );
    assert!(i.content.as_ref().unwrap().contains("<script>"));
    assert_eq!(i.subtitle, None);
    assert_eq!(i.duration, None);
    assert_eq!(i.pub_date, None);
    assert_eq!(i.enclosures[0].length.as_deref(), Some(""));
}

#[test]
fn full_podcasting20_surface() {
    let f = ok("podcasting20.xml");
    let c = &f.channel;
    assert_eq!(c.title.as_deref(), Some("Podcasting 2.0 Show"));
    assert_eq!(c.language.as_deref(), Some("en-us"));
    assert_eq!(c.copyright.as_deref(), Some("© 2025 P20"));
    assert_eq!(c.publisher.as_deref(), Some("editor@p20.example (Ed Itor)"));
    assert_eq!(
        c.updated.as_deref(),
        Some("Mon, 01 Sep 2025 10:00:00 +0000")
    );
    assert_eq!(c.generator.as_deref(), Some("Uguisu fixtures"));
    assert_eq!(c.self_link.as_deref(), Some("https://p20.example/feed.xml"));
    assert_eq!(c.author.as_deref(), Some("P20 Host"));
    assert_eq!(c.subtitle.as_deref(), Some("Sub"));
    assert_eq!(c.summary.as_deref(), Some("Summary text"));
    assert_eq!(c.owner_name.as_deref(), Some("Owner Name"));
    assert_eq!(c.owner_email.as_deref(), Some("owner@p20.example"));
    assert_eq!(c.image.as_deref(), Some("https://p20.example/art.jpg"));
    assert_eq!(
        c.categories,
        vec!["Technology", "Technology / Tech News", "Society & Culture"]
    );
    assert_eq!(c.explicit.as_deref(), Some("false"));
    assert_eq!(c.podcast_type.as_deref(), Some("episodic"));
    assert_eq!(
        c.podcast_guid.as_deref(),
        Some("917393e3-1b1e-5cef-ace4-edaa54e1f810")
    );
    assert_eq!(c.locked.as_deref(), Some("yes"));
    assert_eq!(c.medium.as_deref(), Some("podcast"));
    assert_eq!(c.funding[0].url, "https://p20.example/support");
    assert_eq!(c.funding[0].text.as_deref(), Some("Support the show"));

    let i = &f.items[0];
    assert_eq!(i.title.as_deref(), Some("Full P2.0 Episode"));
    assert_eq!(
        i.itunes_title, None,
        "identical itunes:title is not duplicated"
    );
    assert_eq!(i.guid.as_deref(), Some("p20-ep-1"));
    assert_eq!(i.guid_is_permalink, Some(false));
    assert_eq!(i.link.as_deref(), Some("https://p20.example/ep/1"));
    assert_eq!(i.description.as_deref(), Some("Short"));
    assert_eq!(i.content.as_deref(), Some("<p>Long <b>notes</b></p>"));
    assert_eq!(i.duration.as_deref(), Some("00:30:00"));
    assert_eq!(i.season.as_deref(), Some("2"));
    assert_eq!(i.episode.as_deref(), Some("7"));
    assert_eq!(i.episode_type.as_deref(), Some("full"));
    assert_eq!(i.explicit.as_deref(), Some("no"));
    assert_eq!(i.image.as_deref(), Some("https://p20.example/ep1.jpg"));
    assert_eq!(i.author.as_deref(), Some("Guest Author"));
    assert_eq!(i.enclosures.len(), 1);
    assert_eq!(i.transcripts.len(), 2);
    assert_eq!(i.transcripts[0].rel.as_deref(), Some("captions"));
    assert_eq!(i.chapters[0].url, "https://p20.example/ep1.chapters.json");
    assert_eq!(i.persons.len(), 2);
    assert_eq!(i.persons[0].name, "P20 Host");
    assert_eq!(i.persons[0].role.as_deref(), Some("host"));
    assert_eq!(
        i.persons[0].href.as_deref(),
        Some("https://p20.example/host")
    );
    assert_eq!(i.location.as_ref().unwrap().name, "Berlin");
    assert_eq!(
        i.location.as_ref().unwrap().geo.as_deref(),
        Some("geo:52.52,13.405")
    );
    assert_eq!(i.soundbites.len(), 1);
    assert!((i.soundbites[0].start_time - 73.0).abs() < f64::EPSILON);
    assert_eq!(i.license.as_ref().unwrap().text, "cc-by-4.0");
    assert_eq!(i.txt[0].purpose.as_deref(), Some("verify"));
    let v = i.value.as_ref().unwrap();
    assert_eq!(v.kind.as_deref(), Some("lightning"));
    assert_eq!(v.recipients.len(), 2);
    assert_eq!(v.recipients[1].get("fee").map(String::as_str), Some("true"));
    assert_eq!(i.raw_extensions.len(), 1);
    assert_eq!(i.raw_extensions[0].name, "custom:thing");
    assert_eq!(
        i.raw_extensions[0]
            .attributes
            .get("attr")
            .map(String::as_str),
        Some("x")
    );
    assert_eq!(i.raw_extensions[0].text.as_deref(), Some("kept raw"));
    assert!(i.warnings.is_empty());
}

#[test]
fn namespaces_are_resolved_by_uri() {
    let f = ok("odd_prefixes.xml");
    assert_eq!(
        f.channel.self_link.as_deref(),
        Some("https://odd.example/feed")
    );
    assert_eq!(f.channel.author.as_deref(), Some("Prefix Person"));
    assert_eq!(f.channel.podcast_guid.as_deref(), Some("odd-guid"));
    assert_eq!(f.items[0].duration.as_deref(), Some("10:00"));
    assert_eq!(f.items[0].transcripts.len(), 1);
}

#[test]
fn malformed_item_is_isolated() {
    let f = ok("malformed_item.xml");
    assert_eq!(f.items.len(), 2, "the two good items survive");
    assert_eq!(f.items[0].guid.as_deref(), Some("g1"));
    assert_eq!(f.items[1].guid.as_deref(), Some("g2"));
    assert_eq!(f.malformed_items.len(), 1);
    let m = &f.malformed_items[0];
    assert_eq!(m.index, 1);
    assert_eq!(m.partial_guid.as_deref(), Some("bad"));
    assert!(m.reason.contains("mismatched end tag"), "{}", m.reason);
    assert!(f.is_partial());
    assert!(!f.truncated);
    assert_eq!(f.stats.malformed_items, 1);
}

#[test]
fn truncated_document_keeps_completed_items() {
    let f = ok("truncated.xml");
    assert_eq!(f.items.len(), 1);
    assert_eq!(f.items[0].guid.as_deref(), Some("t1"));
    assert!(f.truncated);
    assert!(
        f.warnings
            .iter()
            .any(|w| w.contains("stopped at malformed xml")),
        "{:?}",
        f.warnings
    );
}

#[test]
fn limits_are_enforced() {
    let limits = FeedLimits {
        max_items: 2,
        ..FeedLimits::default()
    };
    let f = parse(&corpus("dates.xml"), &limits).unwrap();
    assert_eq!(f.items.len(), 2);
    assert!(f.truncated);
    let tiny = FeedLimits {
        max_bytes: 100,
        ..FeedLimits::default()
    };
    assert!(matches!(
        parse(&corpus("dates.xml"), &tiny),
        Err(ParseError::TooLarge { .. })
    ));
    let shallow = FeedLimits {
        max_depth: 3,
        ..FeedLimits::default()
    };
    // rss > channel > item > title is depth 4: no item completes → error
    assert!(matches!(
        parse(&corpus("minimal_rss.xml"), &shallow),
        Err(ParseError::TooDeep(3))
    ));
    let no_malformed = FeedLimits {
        max_malformed_items: 1,
        ..FeedLimits::default()
    };
    let f = parse(&corpus("malformed_item.xml"), &no_malformed).unwrap();
    assert!(f.truncated, "stops once the malformed budget is used up");
    assert_eq!(f.items.len(), 1);
}

#[test]
fn rejects_non_feeds_like_the_probe() {
    let l = FeedLimits::default();
    assert!(matches!(parse(b"   ", &l), Err(ParseError::Empty)));
    assert!(matches!(
        parse(&probe_fixture("html_page.html"), &l),
        Err(ParseError::NotXml {
            looks_like_html: true
        })
    ));
    assert!(matches!(
        parse(b"plain text", &l),
        Err(ParseError::NotXml {
            looks_like_html: false
        })
    ));
    assert!(matches!(
        parse(b"<svg/>", &l),
        Err(ParseError::UnknownRoot(_))
    ));
    assert!(matches!(
        parse(&probe_fixture("malformed.xml"), &l),
        Err(ParseError::Malformed(_))
    ));
    let bomb = parse(&probe_fixture("entity_bomb.xml"), &l).unwrap();
    assert!(
        bomb.warnings
            .iter()
            .any(|w| w.contains("entity references were not expanded"))
    );
}

#[test]
fn every_probe_fixture_agrees() {
    for name in [
        "rss_itunes_podcast.xml",
        "atom_with_enclosures.xml",
        "rss_no_enclosures.xml",
        "rss_alternate_enclosure_only.xml",
        "rss_latin1.xml",
        "rss_entities_and_cdata.xml",
        "rss_rdf.xml",
        "entity_bomb.xml",
    ] {
        let bytes = probe_fixture(name);
        let probe = uguisu_feed::probe(&bytes).unwrap();
        let parsed = parse(&bytes, &FeedLimits::default()).unwrap();
        assert_eq!(parsed.items.len(), probe.item_count, "{name}: item count");
        assert_eq!(
            parsed.stats.items_with_enclosure, probe.items_with_enclosure,
            "{name}: items with media"
        );
        assert_eq!(parsed.channel.title, probe.title, "{name}: title");
        assert_eq!(
            parsed.channel.podcast_guid, probe.podcast_guid,
            "{name}: guid"
        );
        assert_eq!(
            parsed.channel.new_feed_url,
            probe.new_feed_url.map(|u| u.to_string()),
            "{name}: new-feed-url"
        );
    }
}

#[test]
fn recorded_live_feeds_parse_fully() {
    let cases = [
        ("anchor_fm", "anchor_feed", 2, "Placeholder Radio"),
        (
            "rss_buzzsprout_com",
            "buzzsprout_feed",
            11,
            "How to Start a Podcast",
        ),
        (
            "feeds_feedburner_com",
            "feedburner_feed",
            13,
            "Dan Carlin's Hardcore History",
        ),
        ("feeds_twit_tv", "twit_feed", 10, "Security Now (Audio)"),
        (
            "rss_libsyn_com",
            "libsyn_feed",
            34,
            "Dan Carlin's Hardcore History: Addendum",
        ),
    ];
    for (host, case, items, title) in cases {
        let f = parse(&live_feed(host, case), &FeedLimits::default())
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(f.items.len(), items, "{case}");
        assert_eq!(
            f.stats.items_with_enclosure, items,
            "{case}: every item has media"
        );
        assert_eq!(f.channel.title.as_deref(), Some(title), "{case}");
        assert!(
            f.malformed_items.is_empty(),
            "{case}: {:?}",
            f.malformed_items
        );
        assert!(!f.truncated, "{case}");
        assert!(f.channel.self_link.is_some(), "{case}: atom:link rel=self");
        for i in &f.items {
            assert!(
                i.title.is_some() && i.guid.is_some() && i.pub_date.is_some(),
                "{case} item {}",
                i.index
            );
        }
    }
    let buzz = parse(
        &live_feed("rss_buzzsprout_com", "buzzsprout_feed"),
        &FeedLimits::default(),
    )
    .unwrap();
    assert!(buzz.channel.podcast_guid.is_some());
    assert_eq!(buzz.channel.locked.as_deref(), Some("yes"));
    assert!(buzz.items.iter().any(|i| !i.transcripts.is_empty()));
    assert!(buzz.items.iter().any(|i| !i.chapters.is_empty()));
    assert!(
        buzz.items.iter().all(|i| i.content.is_some()),
        "content:encoded on every item"
    );
    let twit = parse(
        &live_feed("feeds_twit_tv", "twit_feed"),
        &FeedLimits::default(),
    )
    .unwrap();
    assert_eq!(
        twit.channel.new_feed_url.as_deref(),
        Some("https://feeds.twit.tv/sn.xml")
    );
    assert!(!twit.channel.funding.is_empty());
    assert!(twit.items.iter().any(|i| !i.persons.is_empty()));
    assert!(twit.items.iter().any(|i| !i.transcripts.is_empty()));
    assert!(
        twit.items
            .iter()
            .all(|i| i.enclosures[0].origin == EnclosureOrigin::Enclosure),
        "media:content never displaces the real enclosure"
    );
}
