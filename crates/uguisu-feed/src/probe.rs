//! Streaming feed probe: identity and podcast-likeness without an episode model.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::Serialize;
use time::OffsetDateTime;
use url::Url;

use crate::dates::parse_date;
use crate::text::{decode_bytes, resolve_reference};

/// Maximum element nesting accepted before the document is rejected.
const MAX_DEPTH: usize = 64;
/// Longest text captured for any single field.
const MAX_TEXT: usize = 4096;

pub use uguisu_core::model::FeedKind;

/// What the probe learned about a feed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FeedProbe {
    /// Syntax family (`None` only in the `Default` value).
    pub kind: Option<FeedKind>,
    /// Channel/feed title.
    pub title: Option<String>,
    /// Channel description or Atom subtitle, truncated.
    pub description: Option<String>,
    /// Channel website link.
    pub link: Option<Url>,
    /// Language tag as published.
    pub language: Option<String>,
    /// `itunes:author` or Atom author name.
    pub author: Option<String>,
    /// Artwork (`itunes:image href`, `image/url`, Atom `logo`).
    pub image: Option<Url>,
    /// `atom:link rel="self"`.
    pub self_link: Option<Url>,
    /// `itunes:new-feed-url`.
    pub new_feed_url: Option<Url>,
    /// `podcast:guid`.
    pub podcast_guid: Option<String>,
    /// `podcast:locked` (`yes`/`no`).
    pub locked: Option<bool>,
    /// `itunes:explicit`.
    pub explicit: Option<bool>,
    /// Completed items/entries.
    pub item_count: usize,
    /// Items with an `enclosure`, Atom `link rel="enclosure"`, RSS 1.0
    /// `enc:enclosure` or `podcast:alternateEnclosure`.
    pub items_with_enclosure: usize,
    /// Newest item timestamp found (`pubDate`, `published`, `updated`, `dc:date`).
    #[serde(with = "time::serde::rfc3339::option")]
    pub newest_item: Option<OffsetDateTime>,
    /// Encoding the body was decoded with.
    pub encoding: String,
    /// Non-fatal problems noticed while probing.
    pub warnings: Vec<String>,
}

impl FeedProbe {
    /// A feed is treated as a podcast when at least one item carries media.
    pub const fn looks_like_podcast(&self) -> bool {
        self.items_with_enclosure >= 1
    }
}

/// Why a body could not be probed as a feed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    /// The body is not XML at all.
    #[error("not an XML document{}", if *.looks_like_html { " (looks like HTML)" } else { "" })]
    NotXml {
        /// Whether the body appears to be an HTML page.
        looks_like_html: bool,
    },
    /// The XML is broken before any item could be read.
    #[error("malformed xml: {0}")]
    Malformed(String),
    /// The declared encoding is not supported.
    #[error("unsupported encoding `{0}`")]
    UnsupportedEncoding(String),
    /// The root element is not RSS, Atom or RDF.
    #[error("unknown root element `{0}`")]
    UnknownRoot(String),
    /// Nesting deeper than the limit.
    #[error("document nested deeper than {MAX_DEPTH} levels")]
    TooDeep,
    /// The body is empty.
    #[error("empty body")]
    Empty,
}

/// Probes a feed body. `content_type` is used only for diagnostics.
pub fn probe(bytes: &[u8]) -> Result<FeedProbe, ProbeError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(ProbeError::Empty);
    }
    let decoded = decode_bytes(bytes).map_err(|e| ProbeError::UnsupportedEncoding(e.0))?;
    let text = decoded
        .text
        .trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    if !text.starts_with('<') {
        return Err(ProbeError::NotXml {
            looks_like_html: looks_like_html(text),
        });
    }
    if looks_like_html(text) {
        return Err(ProbeError::NotXml {
            looks_like_html: true,
        });
    }

    let mut reader = Reader::from_str(text);
    let cfg = reader.config_mut();
    cfg.check_end_names = true;
    cfg.expand_empty_elements = false;
    cfg.trim_text_start = false;
    cfg.trim_text_end = false;

    let mut state = State {
        probe: FeedProbe {
            encoding: decoded.encoding.to_owned(),
            warnings: decoded.warnings,
            ..FeedProbe::default()
        },
        ..State::default()
    };

    loop {
        let event = match reader.read_event() {
            Ok(ev) => ev,
            Err(err) => {
                if state.probe.item_count > 0 {
                    state.probe.warnings.push(format!(
                        "stopped at malformed xml after {} items: {err}",
                        state.probe.item_count
                    ));
                    break;
                }
                return Err(ProbeError::Malformed(err.to_string()));
            }
        };
        match event {
            Event::Start(e) => {
                state.start(&e)?;
            }
            Event::Empty(e) => {
                state.start(&e)?;
                state.end();
            }
            Event::End(_) => state.end(),
            Event::Text(t) => state.text(&t.xml10_content()),
            Event::CData(c) => state.text(&c.into_inner()),
            Event::GeneralRef(r) => {
                if let Some(resolved) = resolve_reference(&r) {
                    state.text(&resolved);
                } else {
                    state.unknown_refs += 1;
                }
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if state.probe.kind.is_none() {
        return Err(ProbeError::NotXml {
            looks_like_html: false,
        });
    }
    if state.unknown_refs > 0 {
        state.probe.warnings.push(format!(
            "{} entity references were not expanded",
            state.unknown_refs
        ));
    }
    if state.depth_open > 0 {
        state
            .probe
            .warnings
            .push("document ended with open elements".to_owned());
    }
    Ok(state.probe)
}

fn looks_like_html(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(512)
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html") || head.contains("<html")
}

#[derive(Debug, Default)]
struct State {
    probe: FeedProbe,
    /// Stack of (prefix, local name) for open elements.
    stack: Vec<(String, String)>,
    depth_open: usize,
    text: String,
    text_overflow: bool,
    in_item: bool,
    item_has_enclosure: bool,
    unknown_refs: usize,
}

impl State {
    fn parent(&self) -> Option<&(String, String)> {
        self.stack.iter().rev().nth(1)
    }

    fn depth(&self) -> usize {
        self.stack.len()
    }

    fn start(&mut self, e: &BytesStart<'_>) -> Result<(), ProbeError> {
        let qname = e.name();
        let local = qname.local_name().as_ref().to_ascii_lowercase();
        let prefix = qname
            .prefix()
            .map(|p| p.as_ref().to_ascii_lowercase())
            .unwrap_or_default();
        if self.depth() >= MAX_DEPTH {
            return Err(ProbeError::TooDeep);
        }
        if self.stack.is_empty() {
            self.probe.kind = Some(match local.as_str() {
                "rss" => FeedKind::Rss2,
                "feed" => FeedKind::Atom,
                "rdf" => FeedKind::Rss1,
                other => return Err(ProbeError::UnknownRoot(other.to_owned())),
            });
        }
        self.stack.push((prefix.clone(), local.clone()));
        self.depth_open += 1;
        self.text.clear();
        self.text_overflow = false;

        let kind = self.probe.kind.unwrap_or(FeedKind::Rss2);
        let is_item = match kind {
            FeedKind::Rss2 => local == "item" && self.depth() == 3,
            FeedKind::Atom => local == "entry" && self.depth() == 2,
            FeedKind::Rss1 => local == "item" && self.depth() == 2,
        };
        if is_item {
            self.in_item = true;
            self.item_has_enclosure = false;
            return Ok(());
        }

        // Attribute-carrying elements.
        let attr = |name: &str| attribute(e, name);
        if self.in_item {
            match (prefix.as_str(), local.as_str()) {
                ("", "enclosure") if attr("url").is_some() => self.item_has_enclosure = true,
                ("enc", "enclosure") | ("podcast", "alternateenclosure") => {
                    self.item_has_enclosure = true;
                }
                ("", "link")
                    if kind == FeedKind::Atom && attr("rel").as_deref() == Some("enclosure") =>
                {
                    self.item_has_enclosure = true;
                }
                _ => {}
            }
            return Ok(());
        }

        let at_channel = self.at_channel_level(kind);
        if !at_channel {
            return Ok(());
        }
        match (prefix.as_str(), local.as_str()) {
            ("itunes", "image") => {
                if self.probe.image.is_none() {
                    self.probe.image = attr("href").and_then(|v| Url::parse(&v).ok());
                }
            }
            ("atom" | "", "link") => {
                let rel = attr("rel").unwrap_or_else(|| "alternate".to_owned());
                let href = attr("href").and_then(|v| Url::parse(&v).ok());
                if rel == "self" && self.probe.self_link.is_none() {
                    self.probe.self_link = href;
                } else if rel == "alternate" && kind == FeedKind::Atom && self.probe.link.is_none()
                {
                    self.probe.link = href;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn at_channel_level(&self, kind: FeedKind) -> bool {
        match kind {
            FeedKind::Rss2 => self.depth() == 3 && self.parent().is_some_and(|p| p.1 == "channel"),
            FeedKind::Atom => self.depth() == 2,
            FeedKind::Rss1 => self.depth() == 3 && self.parent().is_some_and(|p| p.1 == "channel"),
        }
    }

    fn text(&mut self, s: &str) {
        if self.text.len() >= MAX_TEXT {
            self.text_overflow = true;
            return;
        }
        let room = MAX_TEXT - self.text.len();
        if s.len() > room {
            let cut = s
                .char_indices()
                .take_while(|(i, _)| *i < room)
                .last()
                .map_or(0, |(i, c)| i + c.len_utf8());
            self.text.push_str(&s[..cut]);
            self.text_overflow = true;
        } else {
            self.text.push_str(s);
        }
    }

    #[allow(clippy::too_many_lines)] // one pass over the closing element keeps the state machine readable
    fn end(&mut self) {
        let Some((prefix, local)) = self.stack.pop() else {
            return;
        };
        self.depth_open = self.depth_open.saturating_sub(1);
        let kind = self.probe.kind.unwrap_or(FeedKind::Rss2);
        let depth_after = self.depth();
        let text = std::mem::take(&mut self.text);
        let value = text.trim().to_owned();

        let closes_item = match kind {
            FeedKind::Rss2 => local == "item" && depth_after == 2,
            FeedKind::Atom => local == "entry" && depth_after == 1,
            FeedKind::Rss1 => local == "item" && depth_after == 1,
        };
        if closes_item && self.in_item {
            self.in_item = false;
            self.probe.item_count += 1;
            if self.item_has_enclosure {
                self.probe.items_with_enclosure += 1;
            }
            return;
        }
        if self.in_item {
            // Item-level dates only; the full parser reads the rest.
            let is_date = matches!(
                (prefix.as_str(), local.as_str()),
                ("", "pubdate" | "published" | "updated") | ("dc", "date")
            );
            if is_date && !value.is_empty() {
                if let Some(dt) = parse_date(&value) {
                    if self.probe.newest_item.is_none_or(|n| dt > n) {
                        self.probe.newest_item = Some(dt);
                    }
                } else {
                    self.probe
                        .warnings
                        .push(format!("unparseable item date `{}`", truncate(&value, 40)));
                }
            }
            return;
        }

        // Channel-level text fields: parent must be channel/feed.
        let at_channel = match kind {
            FeedKind::Rss2 | FeedKind::Rss1 => {
                depth_after == 2 && self.stack.last().is_some_and(|p| p.1 == "channel")
            }
            FeedKind::Atom => depth_after == 1,
        };
        let image_url = kind != FeedKind::Atom
            && depth_after == 3
            && local == "url"
            && self.stack.last().is_some_and(|p| p.1 == "image");
        let atom_author_name = kind == FeedKind::Atom
            && depth_after == 2
            && local == "name"
            && self.stack.last().is_some_and(|p| p.1 == "author");
        if image_url && self.probe.image.is_none() {
            self.probe.image = Url::parse(&value).ok();
            return;
        }
        if atom_author_name && self.probe.author.is_none() {
            self.probe.author = non_empty(value);
            return;
        }
        if !at_channel || value.is_empty() {
            return;
        }
        match (prefix.as_str(), local.as_str()) {
            ("", "title") if self.probe.title.is_none() => {
                self.probe.title = Some(collapse_ws(&value));
            }
            ("", "description" | "subtitle") if self.probe.description.is_none() => {
                let mut d = collapse_ws(&strip_tags(&value));
                if d.chars().count() > 500 {
                    d = d.chars().take(500).collect::<String>() + "…";
                }
                self.probe.description = Some(d);
            }
            ("", "link") if kind != FeedKind::Atom && self.probe.link.is_none() => {
                self.probe.link = Url::parse(&value).ok();
            }
            ("" | "dc", "language") => self.probe.language = Some(value),
            ("itunes", "author") if self.probe.author.is_none() => {
                self.probe.author = Some(collapse_ws(&value));
            }
            ("", "logo") if kind == FeedKind::Atom && self.probe.image.is_none() => {
                self.probe.image = Url::parse(&value).ok();
            }
            ("itunes", "new-feed-url") => self.probe.new_feed_url = Url::parse(&value).ok(),
            ("podcast", "guid") => self.probe.podcast_guid = Some(value),
            ("podcast", "locked") => {
                self.probe.locked = Some(matches!(
                    value.to_ascii_lowercase().as_str(),
                    "yes" | "true"
                ));
            }
            ("itunes", "explicit") => {
                self.probe.explicit = Some(matches!(
                    value.to_ascii_lowercase().as_str(),
                    "yes" | "true" | "explicit"
                ));
            }
            _ => {}
        }
        if self.text_overflow {
            self.probe.warnings.push(format!(
                "text of <{local}> was truncated to {MAX_TEXT} bytes"
            ));
        }
    }
}

fn attribute(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes().with_checks(false).flatten().find_map(|a| {
        let key = a.key.local_name();
        if key.as_ref().eq_ignore_ascii_case(name) || a.key.as_ref().eq_ignore_ascii_case(name) {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.trim().to_owned())
        } else {
            None
        }
    })
}

fn non_empty(s: String) -> Option<String> {
    (!s.trim().is_empty()).then_some(s)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_bytes!(concat!("../../../tests/fixtures/feeds/probe/", $name))
        };
    }

    #[test]
    fn rss_with_itunes_and_podcast_namespace() {
        let p = probe(fixture!("rss_itunes_podcast.xml")).unwrap();
        assert_eq!(p.kind, Some(FeedKind::Rss2));
        assert_eq!(p.title.as_deref(), Some("Example Diaries"));
        assert_eq!(p.author.as_deref(), Some("Jane Example"));
        assert_eq!(p.language.as_deref(), Some("en-us"));
        assert_eq!(
            p.link.as_ref().map(Url::as_str),
            Some("https://example-diaries.test/")
        );
        assert_eq!(
            p.image.as_ref().map(Url::as_str),
            Some("https://cdn.example-diaries.test/cover.jpg"),
            "itunes:image wins over image/url"
        );
        assert_eq!(
            p.self_link.as_ref().map(Url::as_str),
            Some("https://feeds.example-diaries.test/rss")
        );
        assert_eq!(
            p.new_feed_url.as_ref().map(Url::as_str),
            Some("https://feeds.example-diaries.test/v2/rss")
        );
        assert_eq!(
            p.podcast_guid.as_deref(),
            Some("917393e3-1b1e-5cef-ace4-edaa54e1f810")
        );
        assert_eq!(p.locked, Some(true));
        assert_eq!(p.explicit, Some(false));
        assert_eq!((p.item_count, p.items_with_enclosure), (3, 3));
        assert_eq!(
            p.newest_item.map(OffsetDateTime::unix_timestamp),
            Some(1_756_807_200)
        );
        assert!(p.looks_like_podcast());
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert!(p.description.unwrap().starts_with("True stories"));
    }

    #[test]
    fn atom_feed_with_enclosure_links() {
        let p = probe(fixture!("atom_with_enclosures.xml")).unwrap();
        assert_eq!(p.kind, Some(FeedKind::Atom));
        assert_eq!(p.title.as_deref(), Some("Atom Cast"));
        assert_eq!(p.description.as_deref(), Some("An Atom-based podcast"));
        assert_eq!(p.author.as_deref(), Some("Atom Author"));
        assert_eq!(
            p.link.as_ref().map(Url::as_str),
            Some("https://atomcast.test/")
        );
        assert_eq!(
            p.self_link.as_ref().map(Url::as_str),
            Some("https://atomcast.test/feed.atom")
        );
        assert_eq!(
            p.image.as_ref().map(Url::as_str),
            Some("https://atomcast.test/logo.png")
        );
        assert_eq!((p.item_count, p.items_with_enclosure), (2, 1));
        assert!(p.looks_like_podcast());
        assert_eq!(
            p.newest_item.map(OffsetDateTime::unix_timestamp),
            Some(1_757_289_600)
        );
    }

    #[test]
    fn blog_feed_is_not_a_podcast() {
        let p = probe(fixture!("rss_no_enclosures.xml")).unwrap();
        assert_eq!((p.item_count, p.items_with_enclosure), (2, 0));
        assert!(!p.looks_like_podcast());
        assert_eq!(p.title.as_deref(), Some("Just a Blog"));
    }

    #[test]
    fn alternate_enclosure_counts_as_media() {
        let p = probe(fixture!("rss_alternate_enclosure_only.xml")).unwrap();
        assert_eq!((p.item_count, p.items_with_enclosure), (1, 1));
    }

    #[test]
    fn latin1_body_is_decoded() {
        let p = probe(fixture!("rss_latin1.xml")).unwrap();
        assert_eq!(p.encoding, "iso-8859-1");
        assert_eq!(p.title.as_deref(), Some("Gemütlicher Podcast"));
        assert_eq!(p.description.as_deref(), Some("Über alles."));
        assert!(p.looks_like_podcast());
    }

    #[test]
    fn entities_and_cdata_are_resolved() {
        let p = probe(fixture!("rss_entities_and_cdata.xml")).unwrap();
        assert_eq!(p.title.as_deref(), Some("Tom & Jerry’s Show — Live"));
        assert_eq!(
            p.description.as_deref(),
            Some("Cats &amp; mice"),
            "CDATA is literal, tags stripped"
        );
        assert_eq!(p.author.as_deref(), Some("T &amp; J"));
        assert!(p.looks_like_podcast());
    }

    #[test]
    fn malformed_xml_is_an_error() {
        let err = probe(fixture!("malformed.xml")).unwrap_err();
        assert!(matches!(err, ProbeError::Malformed(_)), "{err:?}");
    }

    #[test]
    fn entity_bomb_is_not_expanded() {
        let started = std::time::Instant::now();
        let p = probe(fixture!("entity_bomb.xml")).unwrap();
        assert!(started.elapsed().as_millis() < 500);
        assert_eq!(
            p.title.as_deref(),
            None,
            "unknown entities are dropped, not expanded"
        );
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("entity references were not expanded"))
        );
        assert!(p.looks_like_podcast());
    }

    #[test]
    fn html_is_rejected() {
        let err = probe(fixture!("html_page.html")).unwrap_err();
        assert_eq!(
            err,
            ProbeError::NotXml {
                looks_like_html: true
            }
        );
        assert_eq!(
            probe(b"just text").unwrap_err(),
            ProbeError::NotXml {
                looks_like_html: false
            }
        );
        assert_eq!(probe(b"   ").unwrap_err(), ProbeError::Empty);
    }

    #[test]
    fn rdf_feed_with_enc_enclosure() {
        let p = probe(fixture!("rss_rdf.xml")).unwrap();
        assert_eq!(p.kind, Some(FeedKind::Rss1));
        assert_eq!(p.title.as_deref(), Some("RDF Cast"));
        assert_eq!((p.item_count, p.items_with_enclosure), (1, 1));
    }

    #[test]
    fn unknown_root_and_depth_limit() {
        assert!(matches!(
            probe(b"<html5><x/></html5>").unwrap_err(),
            ProbeError::NotXml { .. } | ProbeError::UnknownRoot(_)
        ));
        assert_eq!(
            probe(b"<svg/>").unwrap_err(),
            ProbeError::UnknownRoot("svg".into())
        );
        let deep = format!(
            "<rss><channel>{}{}</channel></rss>",
            "<a>".repeat(70),
            "</a>".repeat(70)
        );
        assert_eq!(probe(deep.as_bytes()).unwrap_err(), ProbeError::TooDeep);
    }

    #[test]
    fn truncated_after_items_is_tolerated() {
        let body = b"<rss><channel><title>T</title><item><enclosure url=\"https://x.test/1.mp3\"/></item><item><title>oops";
        let p = probe(body).unwrap();
        assert_eq!((p.item_count, p.items_with_enclosure), (1, 1));
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("malformed") || w.contains("open elements")),
            "{:?}",
            p.warnings
        );
    }
}
