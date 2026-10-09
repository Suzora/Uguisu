//! OPML subscription lists: what is read, what is refused, and that a
//! written list reads back.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;

use uguisu_feed::opml::{MAX_BYTES, MAX_DEPTH, MAX_OUTLINES, OpmlError, Outline, parse, write};

fn doc(body: &str) -> String {
    format!("<?xml version=\"1.0\"?><opml version=\"2.0\"><head/><body>{body}</body></opml>")
}

fn feed(title: Option<&str>, url: &str) -> Outline {
    Outline {
        title: title.map(str::to_owned),
        xml_url: url.to_owned(),
        html_url: None,
    }
}

#[test]
fn nested_folders_are_flattened() {
    let outlines = parse(&doc(
        r#"<outline text="News">
             <outline text="Daily" xmlUrl="https://a.example/feed"/>
             <outline text="Deep"><outline text="Inner" xmlUrl="https://b.example/rss"/></outline>
           </outline>
           <outline text="Top" xmlUrl="https://c.example/podcast.xml" htmlUrl="https://c.example/"/>"#,
    ))
    .unwrap();
    let urls: Vec<_> = outlines.iter().map(|o| o.xml_url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://a.example/feed",
            "https://b.example/rss",
            "https://c.example/podcast.xml"
        ]
    );
    assert_eq!(outlines[2].html_url.as_deref(), Some("https://c.example/"));
}

#[test]
fn title_falls_back_to_text() {
    let outlines = parse(&doc(
        r#"<outline text="From  text" xmlUrl="https://a.example/1"/>
           <outline text="ignored" title=" From
               title " xmlUrl="https://a.example/2"/>
           <outline text="  " xmlUrl="https://a.example/3"/>"#,
    ))
    .unwrap();
    let titles: Vec<_> = outlines.iter().map(|o| o.title.as_deref()).collect();
    assert_eq!(titles, [Some("From text"), Some("From title"), None]);
}

#[test]
fn outline_without_xml_url_skipped() {
    let outlines = parse(&doc(
        r#"<outline text="A folder"/><outline text="Empty" xmlUrl="  "/>
           <outline text="Link" type="link" url="https://a.example/"/>"#,
    ))
    .unwrap();
    assert!(outlines.is_empty(), "{outlines:?}");
}

#[test]
fn attribute_names_ignore_case() {
    let outlines = parse(&doc(
        r#"<OUTLINE TEXT="Loud" XMLURL="https://a.example/feed" HTMLURL="https://a.example/"/>"#,
    ))
    .unwrap();
    assert_eq!(
        outlines,
        [Outline {
            title: Some("Loud".to_owned()),
            xml_url: "https://a.example/feed".to_owned(),
            html_url: Some("https://a.example/".to_owned()),
        }]
    );
}

#[test]
fn entity_bomb_stays_unexpanded() {
    let bomb = r#"<?xml version="1.0"?>
<!DOCTYPE opml [
  <!ENTITY lol "lol">
  <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
  <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
  <!ENTITY ext SYSTEM "file:///etc/passwd">
]>
<opml version="2.0"><body>
  <outline text="&lol3;" xmlUrl="https://a.example/&ext;"/>
</body></opml>"#;
    let outlines = parse(bomb).unwrap();
    assert_eq!(outlines.len(), 1);
    assert_eq!(outlines[0].title.as_deref(), Some("&lol3;"));
    assert_eq!(outlines[0].xml_url, "https://a.example/&ext;");
}

#[test]
fn bare_ampersand_url_kept_raw() {
    let outlines = parse(&doc(
        r#"<outline text="Caf&#233; &amp; Talk&nbsp;Show" xmlUrl="https://a.example/feed?id=1&amp;fmt=mp3"/>
           <outline text="Raw" xmlUrl="https://a.example/feed?id=2&fmt=mp3"/>"#,
    ))
    .unwrap();
    assert_eq!(outlines[0].title.as_deref(), Some("Café & Talk Show"));
    assert_eq!(outlines[0].xml_url, "https://a.example/feed?id=1&fmt=mp3");
    assert_eq!(outlines[1].xml_url, "https://a.example/feed?id=2&fmt=mp3");
}

#[test]
fn html_page_is_not_opml() {
    let err = parse("<!DOCTYPE html><html><body>Subscriptions</body></html>").unwrap_err();
    assert_eq!(
        err,
        OpmlError::NotXml {
            looks_like_html: true
        }
    );
    let err = parse("xmlUrl=https://a.example/feed").unwrap_err();
    assert_eq!(
        err,
        OpmlError::NotXml {
            looks_like_html: false
        }
    );
}

#[test]
fn rss_feed_is_not_opml() {
    let err = parse("<rss version=\"2.0\"><channel><title>x</title></channel></rss>").unwrap_err();
    assert_eq!(
        err,
        OpmlError::NotOpml {
            root: "rss".to_owned()
        }
    );
    let err = parse(&format!("{}<unclosed", doc(""))).unwrap_err();
    assert!(matches!(err, OpmlError::Malformed(_)), "{err:?}");
}

#[test]
fn empty_document_is_refused() {
    assert_eq!(parse(" \n\u{feff}\t").unwrap_err(), OpmlError::Empty);
    assert_eq!(
        parse("<?xml version=\"1.0\"?><!-- nothing -->").unwrap_err(),
        OpmlError::Empty
    );
    assert!(parse("\u{feff}<opml><body/></opml>").unwrap().is_empty());
}

#[test]
fn nesting_past_limit_refused() {
    let at_limit = format!(
        "<opml>{}{}</opml>",
        "<outline>".repeat(MAX_DEPTH - 1),
        "</outline>".repeat(MAX_DEPTH - 1)
    );
    assert!(parse(&at_limit).is_ok());
    let past = format!("<opml>{}", "<outline>".repeat(MAX_DEPTH));
    assert_eq!(parse(&past).unwrap_err(), OpmlError::TooDeep(MAX_DEPTH));
}

#[test]
fn document_over_limit_refused() {
    let padding = " ".repeat(MAX_BYTES);
    let err = parse(&format!("{}{padding}", doc(""))).unwrap_err();
    assert!(
        matches!(err, OpmlError::TooLarge { limit, .. } if limit == MAX_BYTES),
        "{err:?}"
    );
}

#[test]
fn too_many_feeds_refused() {
    let mut body = String::new();
    for n in 0..MAX_OUTLINES {
        let _ = write!(body, "<outline xmlUrl=\"https://a.example/{n}\"/>");
    }
    assert_eq!(parse(&doc(&body)).unwrap().len(), MAX_OUTLINES);
    body.push_str("<outline xmlUrl=\"https://a.example/one-more\"/>");
    assert_eq!(
        parse(&doc(&body)).unwrap_err(),
        OpmlError::TooMany(MAX_OUTLINES)
    );
}

#[test]
fn written_document_reads_back() {
    let outlines = vec![
        Outline {
            title: Some("Rock & Roll <Live> \"Quoted\" 'n' Café 🎙️ 日本語".to_owned()),
            xml_url: "https://a.example/feed?a=1&b=<2>".to_owned(),
            html_url: Some("https://a.example/?x=\"y\"".to_owned()),
        },
        feed(None, "https://b.example/rss"),
    ];
    let written = write("Uguisu & friends", &outlines);
    assert!(written.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(
        written.contains("<title>Uguisu &amp; friends</title>"),
        "{written}"
    );
    let read = parse(&written).unwrap();
    assert_eq!(read[0], outlines[0]);
    // An outline needs `text`; without a title the URL stands in for it.
    assert_eq!(read[1].title.as_deref(), Some("https://b.example/rss"));
    assert_eq!(read[1].xml_url, "https://b.example/rss");
    assert_eq!(written, write("Uguisu & friends", &outlines));
}

#[test]
fn control_characters_never_written() {
    let written = write(
        "t\u{0}",
        &[feed(
            Some("Bell\u{7} and\u{1b}escape\u{fffe}"),
            "https://a.example/f",
        )],
    );
    assert!(
        !written
            .chars()
            .any(|c| (c < ' ' && c != '\n') || c == '\u{fffe}'),
        "{written:?}"
    );
    assert_eq!(
        parse(&written).unwrap()[0].title.as_deref(),
        Some("Bell andescape")
    );
}
