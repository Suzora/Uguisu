//! Tolerant RSS 2.0 parser for podcast feeds.
//!
//! Only the fields Uguisu indexes are extracted. Nothing is lost by that: the complete feed is
//! stored byte for byte as a snapshot, and the raw `<item>` XML is kept with every revision.

use anyhow::{Context, Result, bail};
use roxmltree::{Document, Node, ParsingOptions};

const NS_ITUNES: &str = "http://www.itunes.com/dtds/podcast-1.0.dtd";
const NS_PODCAST: &str = "https://podcastindex.org/namespace/1.0";
const NS_CONTENT: &str = "http://purl.org/rss/1.0/modules/content/";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFeed {
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub link: Option<String>,
    pub image_url: Option<String>,
    pub language: Option<String>,
    pub podcast_guid: Option<String>,
    pub new_feed_url: Option<String>,
    /// Namespace declarations of the root element as `(prefix, uri)`; the default namespace
    /// has an empty prefix.
    pub namespaces: Vec<(String, String)>,
    pub items: Vec<ParsedItem>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedItem {
    pub guid: String,
    pub title: String,
    pub description: Option<String>,
    pub link: Option<String>,
    pub published_at: Option<i64>,
    pub duration: Option<i64>,
    pub enclosure_url: Option<String>,
    pub enclosure_type: Option<String>,
    pub enclosure_length: Option<i64>,
    pub image_url: Option<String>,
    pub season: Option<i64>,
    pub episode_no: Option<i64>,
    pub raw_xml: String,
}

impl ParsedItem {
    /// Hash over the indexed metadata. The query string of the enclosure URL is left out because
    /// many hosts put per-request tracking tokens there, which would create a revision per fetch.
    pub fn meta_hash(&self) -> String {
        let enclosure = self
            .enclosure_url
            .as_deref()
            .map(|u| u.split(['?', '#']).next().unwrap_or(u));
        let mut hasher = blake3::Hasher::new();
        let mut field = |value: Option<&str>| {
            match value {
                Some(v) => {
                    hasher.update(&(v.len() as u64).to_le_bytes());
                    hasher.update(v.as_bytes());
                }
                None => {
                    hasher.update(&u64::MAX.to_le_bytes());
                }
            };
        };
        field(Some(&self.title));
        field(self.description.as_deref());
        field(self.link.as_deref());
        field(self.published_at.map(|v| v.to_string()).as_deref());
        field(self.duration.map(|v| v.to_string()).as_deref());
        field(enclosure);
        field(self.enclosure_type.as_deref());
        field(self.image_url.as_deref());
        field(self.season.map(|v| v.to_string()).as_deref());
        field(self.episode_no.map(|v| v.to_string()).as_deref());
        hasher.finalize().to_hex().to_string()
    }
}

fn ns_matches(actual: Option<&str>, wanted: Option<&str>) -> bool {
    match (actual, wanted) {
        (None, None) => true,
        (Some(a), Some(w)) => a.eq_ignore_ascii_case(w),
        _ => false,
    }
}

fn child<'a, 'i>(node: Node<'a, 'i>, ns: Option<&str>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| {
        c.is_element() && c.tag_name().name() == name && ns_matches(c.tag_name().namespace(), ns)
    })
}

fn text_of(node: Node) -> Option<String> {
    let text: String = node
        .descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn child_text(node: Node, ns: Option<&str>, name: &str) -> Option<String> {
    child(node, ns, name).and_then(text_of)
}

fn attr(node: Node, name: &str) -> Option<String> {
    node.attribute(name)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Parses RFC 2822 dates (the RSS standard) and, as a fallback, RFC 3339.
///
/// The optional weekday is dropped first: real feeds contain dates whose weekday does not match
/// the date, and a strict parser would reject them entirely.
pub fn parse_date(input: &str) -> Option<i64> {
    let input = input.trim();
    let input = match input.split_once(',') {
        Some((weekday, rest)) if weekday.chars().all(char::is_alphabetic) => rest.trim_start(),
        _ => input,
    };
    crate::util::parse_rfc2822(input).or_else(|| crate::util::parse_rfc3339(input))
}

/// Parses `HH:MM:SS`, `MM:SS` or plain seconds.
pub fn parse_duration(input: &str) -> Option<i64> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let mut total = 0i64;
    for part in input.split(':') {
        let value: f64 = part.trim().parse().ok()?;
        if !(0.0..1e9).contains(&value) {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(value as i64)?;
    }
    Some(total)
}

pub fn parse(bytes: &[u8]) -> Result<ParsedFeed> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(text, options).context("the feed is not valid XML")?;
    let root = doc.root_element();
    match root.tag_name().name() {
        "rss" => {}
        "feed" => bail!("Atom feeds are not supported yet"),
        "html" => bail!("this URL points to a web page, not to a podcast feed"),
        other => bail!("not an RSS feed (root element <{other}>)"),
    }
    let channel = child(root, None, "channel").context("the feed has no <channel>")?;

    let image_url = child(channel, Some(NS_ITUNES), "image")
        .and_then(|n| attr(n, "href"))
        .or_else(|| child(channel, None, "image").and_then(|n| child_text(n, None, "url")));

    let mut feed = ParsedFeed {
        title: child_text(channel, None, "title").unwrap_or_else(|| "Untitled podcast".into()),
        author: child_text(channel, Some(NS_ITUNES), "author"),
        description: child_text(channel, None, "description")
            .or_else(|| child_text(channel, Some(NS_ITUNES), "summary")),
        link: child_text(channel, None, "link"),
        image_url,
        language: child_text(channel, None, "language"),
        podcast_guid: child_text(channel, Some(NS_PODCAST), "guid"),
        new_feed_url: child_text(channel, Some(NS_ITUNES), "new-feed-url"),
        namespaces: root
            .namespaces()
            .filter(|ns| ns.name() != Some("xml"))
            .map(|ns| (ns.name().unwrap_or("").to_string(), ns.uri().to_string()))
            .collect(),
        items: Vec::new(),
    };

    for item in channel.children().filter(|c| {
        c.is_element() && c.tag_name().name() == "item" && c.tag_name().namespace().is_none()
    }) {
        feed.items.push(parse_item(item, text));
    }
    Ok(feed)
}

fn parse_item(item: Node, source: &str) -> ParsedItem {
    let enclosure = child(item, None, "enclosure");
    let enclosure_url = enclosure.and_then(|n| attr(n, "url"));
    let link = child_text(item, None, "link");
    let title = child_text(item, None, "title")
        .or_else(|| child_text(item, Some(NS_ITUNES), "title"))
        .unwrap_or_else(|| "Untitled episode".into());
    let published_raw = child_text(item, None, "pubDate");

    // Feeds without <guid> exist. Fall back to something that stays stable between fetches.
    let guid = child_text(item, None, "guid")
        .or_else(|| {
            enclosure_url
                .as_deref()
                .map(|u| u.split(['?', '#']).next().unwrap_or(u).to_string())
        })
        .or_else(|| link.clone())
        .unwrap_or_else(|| format!("{title}|{}", published_raw.as_deref().unwrap_or("")));

    ParsedItem {
        guid,
        title,
        description: child_text(item, Some(NS_CONTENT), "encoded")
            .or_else(|| child_text(item, None, "description"))
            .or_else(|| child_text(item, Some(NS_ITUNES), "summary")),
        link,
        published_at: published_raw.as_deref().and_then(parse_date),
        duration: child_text(item, Some(NS_ITUNES), "duration")
            .as_deref()
            .and_then(parse_duration),
        enclosure_url,
        enclosure_type: enclosure.and_then(|n| attr(n, "type")),
        enclosure_length: enclosure
            .and_then(|n| attr(n, "length"))
            .and_then(|v| v.parse().ok())
            .filter(|v| *v > 0),
        image_url: child(item, Some(NS_ITUNES), "image").and_then(|n| attr(n, "href")),
        season: child_text(item, Some(NS_ITUNES), "season").and_then(|v| v.parse().ok()),
        episode_no: child_text(item, Some(NS_ITUNES), "episode").and_then(|v| v.parse().ok()),
        raw_xml: source[item.range()].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"
     xmlns:podcast="https://podcastindex.org/namespace/1.0"
     xmlns:content="http://purl.org/rss/1.0/modules/content/"
     xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>Bird Songs &amp; Stories</title>
    <atom:link href="https://example.org/feed.xml" rel="self"/>
    <link>https://example.org</link>
    <description><![CDATA[All about <b>warblers</b>.]]></description>
    <language>en</language>
    <itunes:author>A. Warbler</itunes:author>
    <itunes:image href="https://example.org/cover.jpg"/>
    <podcast:guid>917393e3-1b1e-5cef-ace4-edaa54e1f810</podcast:guid>
    <item>
      <title>Episode 2</title>
      <guid isPermaLink="false">ep-2</guid>
      <pubDate>Tue, 02 Jan 2024 10:00:00 +0000</pubDate>
      <itunes:duration>01:02:03</itunes:duration>
      <itunes:season>1</itunes:season>
      <itunes:episode>2</itunes:episode>
      <content:encoded><![CDATA[<p>Long notes</p>]]></content:encoded>
      <description>Short notes</description>
      <enclosure url="https://cdn.example.org/ep2.mp3?token=abc" length="1234" type="audio/mpeg"/>
    </item>
    <item>
      <title>Episode 1</title>
      <pubDate>Mon, 01 Jan 2024 10:00:00 GMT</pubDate>
      <itunes:duration>125</itunes:duration>
      <enclosure url="https://cdn.example.org/ep1.mp3?token=abc" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_channel_and_items() {
        let feed = parse(FEED.as_bytes()).unwrap();
        assert_eq!(feed.title, "Bird Songs & Stories");
        assert_eq!(feed.link.as_deref(), Some("https://example.org"));
        assert_eq!(
            feed.description.as_deref(),
            Some("All about <b>warblers</b>.")
        );
        assert_eq!(feed.author.as_deref(), Some("A. Warbler"));
        assert_eq!(
            feed.image_url.as_deref(),
            Some("https://example.org/cover.jpg")
        );
        assert_eq!(
            feed.podcast_guid.as_deref(),
            Some("917393e3-1b1e-5cef-ace4-edaa54e1f810")
        );
        assert_eq!(feed.items.len(), 2);
        assert!(feed.namespaces.contains(&(
            "podcast".to_string(),
            "https://podcastindex.org/namespace/1.0".to_string()
        )));
        assert_eq!(feed.namespaces.len(), 4);

        let two = &feed.items[0];
        assert_eq!(two.guid, "ep-2");
        assert_eq!(two.published_at, Some(1_704_189_600));
        assert_eq!(two.duration, Some(3723));
        assert_eq!(two.description.as_deref(), Some("<p>Long notes</p>"));
        assert_eq!(two.enclosure_length, Some(1234));
        assert_eq!((two.season, two.episode_no), (Some(1), Some(2)));
        assert!(two.raw_xml.starts_with("<item>") && two.raw_xml.ends_with("</item>"));

        let one = &feed.items[1];
        assert_eq!(one.guid, "https://cdn.example.org/ep1.mp3");
        assert_eq!(one.published_at, Some(1_704_103_200));
        assert_eq!(one.duration, Some(125));
    }

    #[test]
    fn meta_hash_ignores_enclosure_query() {
        let feed = parse(FEED.as_bytes()).unwrap();
        let mut item = feed.items[0].clone();
        let before = item.meta_hash();
        item.enclosure_url = Some("https://cdn.example.org/ep2.mp3?token=other".into());
        assert_eq!(before, item.meta_hash());
        item.title.push('!');
        assert_ne!(before, item.meta_hash());
    }

    #[test]
    fn rejects_non_feeds() {
        assert!(parse(b"<html><body>hi</body></html>").is_err());
        assert!(parse(b"<feed xmlns=\"http://www.w3.org/2005/Atom\"/>").is_err());
        assert!(parse(b"not xml").is_err());
    }

    #[test]
    fn dates() {
        assert_eq!(
            parse_date("Tue, 02 Jan 2024 10:00:00 +0000"),
            Some(1_704_189_600)
        );
        // Wrong weekday, as found in the wild.
        assert_eq!(
            parse_date("Fri, 02 Jan 2024 10:00:00 +0000"),
            Some(1_704_189_600)
        );
        assert_eq!(
            parse_date("02 Jan 2024 11:00:00 +0100"),
            Some(1_704_189_600)
        );
        assert_eq!(parse_date("2024-01-02T10:00:00Z"), Some(1_704_189_600));
        assert_eq!(parse_date("yesterday"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("02:03"), Some(123));
        assert_eq!(parse_duration("90.5"), Some(90));
        assert_eq!(parse_duration("abc"), None);
    }
}
