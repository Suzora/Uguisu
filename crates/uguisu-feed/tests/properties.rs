//! Property tests for the parser, normalizer and identity cascade:
//! nothing panics on arbitrary input, normalization is deterministic and
//! idempotent, identities are stable and order-independent.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use std::fmt::Write as _;
use time::OffsetDateTime;
use uguisu_core::config::FeedLimits;
use uguisu_core::ids::EpisodeId;
use uguisu_feed::identity::{
    IdentitySignals, fingerprint, normalize_enclosure_url, normalize_title, resolve_identities,
};
use uguisu_feed::normalize::{html_to_text, parse_date_detailed, parse_duration};
use uguisu_feed::{opml, parse};
use url::Url;

fn any_feed() -> impl Strategy<Value = String> {
    let item = (
        "[a-zA-Z0-9 ]{0,20}",
        prop::option::of("[a-zA-Z0-9-]{0,12}"),
        prop::option::of("[0-9]{1,2}:[0-9]{1,2}"),
        prop::option::of("[a-z0-9]{1,8}"),
    )
        .prop_map(|(title, guid, duration, file)| {
            let mut s = String::from("<item>");
            let _ = write!(s, "<title>{title}</title>");
            if let Some(g) = guid {
                let _ = write!(s, "<guid>{g}</guid>");
            }
            if let Some(d) = duration {
                let _ = write!(s, "<itunes:duration>{d}</itunes:duration>");
            }
            if let Some(f) = file {
                let _ = write!(
                    s,
                    "<enclosure url=\"https://cdn.example/{f}.mp3\" type=\"audio/mpeg\" length=\"1\"/>"
                );
            }
            s.push_str("<pubDate>Mon, 01 Sep 2025 10:00:00 +0000</pubDate></item>");
            s
        });
    prop::collection::vec(item, 0..6).prop_map(|items| {
        format!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\"><channel><title>P</title>{}</channel></rss>",
            items.concat()
        )
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn parser_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = parse(&bytes, &FeedLimits::default());
    }

    #[test]
    fn parser_never_panics_on_arbitrary_xmlish_text(s in "[<>/a-z=\"' &;#0-9\\n]{0,512}") {
        let _ = parse(s.as_bytes(), &FeedLimits::default());
    }

    #[test]
    fn opml_parser_never_panics(s in "[<>/a-zA-Z=\"' &;#0-9\\n]{0,512}") {
        let _ = opml::parse(&s);
    }

    #[test]
    fn opml_round_trips_any_text(
        title in "\\PC{0,40}",
        path in "[a-zA-Z0-9&<>'\"=?/._~-]{0,30}",
    ) {
        let outline = opml::Outline {
            title: Some(title.clone()),
            xml_url: format!("https://a.example/{path}"),
            html_url: None,
        };
        let read = opml::parse(&opml::write("t", std::slice::from_ref(&outline))).unwrap();
        let collapsed = title.split_whitespace().collect::<Vec<_>>().join(" ");
        prop_assert_eq!(read.len(), 1);
        prop_assert_eq!(read[0].title.clone(), (!collapsed.is_empty()).then_some(collapsed));
        prop_assert_eq!(&read[0].xml_url, &outline.xml_url);
    }

    #[test]
    fn generated_feeds_parse_deterministically(feed in any_feed()) {
        let a = parse(feed.as_bytes(), &FeedLimits::default()).unwrap();
        let b = parse(feed.as_bytes(), &FeedLimits::default()).unwrap();
        prop_assert_eq!(&a, &b);
        prop_assert!(!a.truncated);
        prop_assert!(a.malformed_items.is_empty());
    }

    #[test]
    fn dates_and_durations_never_panic(s in "\\PC{0,64}") {
        let _ = parse_date_detailed(&s, OffsetDateTime::now_utc());
        let _ = parse_duration(&s);
        let _ = html_to_text(&s, 32);
        let _ = normalize_title(&s);
    }

    #[test]
    fn html_to_text_is_idempotent_on_markup(
        tokens in prop::collection::vec(
            prop::sample::select(vec![
                // The output is plain text, so only inputs whose text part
                // carries no angle brackets can be stable under a second pass.
                "word", "Zoë", "&amp;", "&copy;", " ", "\n", "<p>", "</p>", "<br/>", "<b>", "</b>",
                "<script>x</script>", "<div class=\"a\">", "</div>", "e\u{301}",
            ]),
            0..40,
        )
    ) {
        let s = tokens.concat();
        let once = html_to_text(&s, 1000);
        let twice = html_to_text(&once, 1000);
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn url_normalization_is_idempotent(
        host in "[a-z]{1,8}\\.(com|fm|net)",
        path in "[a-zA-Z0-9/_-]{0,24}",
        query in prop::option::of("[a-z]{1,4}=[a-z0-9]{1,4}"),
    ) {
        let q = query.map(|q| format!("?{q}")).unwrap_or_default();
        let https = Url::parse(&format!("https://{host}/{path}{q}")).unwrap();
        let http = Url::parse(&format!("http://{host}/{path}{q}")).unwrap();
        let a = normalize_enclosure_url(&https);
        let b = normalize_enclosure_url(&http);
        prop_assert_eq!(&a, &b);
        // Re-normalizing the normalized form (with a scheme) is stable.
        let again = normalize_enclosure_url(&Url::parse(&format!("https://{a}")).unwrap());
        prop_assert_eq!(a, again);
    }

    #[test]
    fn fingerprint_is_deterministic_and_title_case_insensitive(
        title in "[a-zA-Z0-9 ]{1,32}",
        len in prop::option::of(0u64..10_000_000),
    ) {
        let now = OffsetDateTime::now_utc();
        let a = fingerprint(&title, Some(now), len);
        let b = fingerprint(&title.to_uppercase(), Some(now), len);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn the_cascade_is_order_independent(
        signals in prop::collection::vec(
            (prop::option::of("[a-f]{1,3}"), prop::option::of("[x-z]{1,3}"), "[0-9]{1,4}"),
            0..12,
        )
    ) {
        let items: Vec<IdentitySignals> = signals
            .iter()
            .map(|(g, u, f)| IdentitySignals { guid: g.clone(), enclosure: u.clone(), fingerprint: Some(f.clone()) })
            .collect();
        let forward = resolve_identities(&items);
        let mut reversed_items = items.clone();
        reversed_items.reverse();
        let mut backward = resolve_identities(&reversed_items);
        backward.reverse();
        prop_assert_eq!(&forward, &backward);
        for (s, id) in items.iter().zip(forward.iter()) {
            prop_assert_eq!(&id.guid_key, &s.guid);
            prop_assert_eq!(&id.enclosure_key, &s.enclosure);
            match id.key.split_once(':').map(|(k, _)| k) {
                Some("guid") => {
                    let g = s.guid.as_deref().unwrap();
                    prop_assert_eq!(items.iter().filter(|o| o.guid.as_deref() == Some(g)).count(), 1);
                }
                Some("url") => {
                    let u = s.enclosure.as_deref().unwrap();
                    prop_assert_eq!(items.iter().filter(|o| o.enclosure.as_deref() == Some(u)).count(), 1);
                }
                Some("fp") => {}
                other => prop_assert!(false, "unexpected key prefix {other:?}"),
            }
        }
    }

    #[test]
    fn normalization_of_generated_feeds_is_idempotent(feed in any_feed()) {
        let parsed = parse(feed.as_bytes(), &FeedLimits::default()).unwrap();
        let now = OffsetDateTime::now_utc();
        for item in &parsed.items {
            let id = EpisodeId::new();
            let a = uguisu_feed::normalize::normalize_item(item, id, now);
            let b = uguisu_feed::normalize::normalize_item(item, id, now);
            // Enclosure ids are fresh per call; compare everything else.
            prop_assert_eq!(a.title, b.title);
            prop_assert_eq!(a.duration_secs, b.duration_secs);
            prop_assert_eq!(a.published.value, b.published.value);
            prop_assert_eq!(a.enclosures.len(), b.enclosures.len());
        }
    }
}
