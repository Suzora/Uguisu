//! Full podcast feed parser (ADR 0008): RSS 2.0, Atom 1.0 and RSS 1.0/RDF
//! with the iTunes and Podcasting 2.0 namespaces, streaming over
//! `quick-xml` with the same hardening as the probe (no entity expansion,
//! depth and size limits, lossy decoding with warnings).
//!
//! The output is *parsed*, not normalized: dates and durations are kept as
//! written, descriptions keep their markup, and unmodelled elements are
//! preserved in `raw_extensions`. `crate::normalize` and `crate::identity`
//! turn a [`ParsedItem`] into an episode.
//!
//! Error isolation (ADR 0017): limit violations truncate and warn; a
//! mismatched end tag inside an item marks that item malformed and parsing
//! continues; an XML syntax error, the depth limit or the item limit stop
//! parsing with `truncated = true` and every completed item kept.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::{Deserialize, Serialize};
use uguisu_core::config::FeedLimits;
use uguisu_core::model::{
    ChaptersRef, FeedKind, Funding, License, Location, Person, RawExtension, Soundbite,
    TranscriptRef, Txt,
};

use crate::text::{decode_bytes, resolve_reference};

/// Why a body could not be parsed as a feed at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The body is not XML.
    #[error("not an XML document{}", if *.looks_like_html { " (looks like HTML)" } else { "" })]
    NotXml {
        /// Whether the body appears to be an HTML page.
        looks_like_html: bool,
    },
    /// Broken XML before any item could be read.
    #[error("malformed xml: {0}")]
    Malformed(String),
    /// The declared encoding is not supported.
    #[error("unsupported encoding `{0}`")]
    UnsupportedEncoding(String),
    /// The root element is not RSS, Atom or RDF.
    #[error("unknown root element `{0}`")]
    UnknownRoot(String),
    /// Nesting deeper than the limit before any item could be read.
    #[error("document nested deeper than {0} levels")]
    TooDeep(usize),
    /// The body exceeds the byte limit.
    #[error("body of {size} bytes exceeds the limit of {limit} bytes")]
    TooLarge {
        /// Body size.
        size: usize,
        /// Limit.
        limit: u64,
    },
    /// Empty body.
    #[error("empty body")]
    Empty,
}

/// Where an enclosure was declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnclosureOrigin {
    /// RSS `<enclosure>`.
    Enclosure,
    /// Atom `<link rel="enclosure">`.
    AtomLink,
    /// RSS 1.0 `<enc:enclosure>`.
    RdfEnc,
    /// `<media:content>` (fallback when nothing else declares media).
    MediaContent,
    /// `<podcast:alternateEnclosure>`.
    AlternateEnclosure,
}

/// A media reference as written in the feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedEnclosure {
    /// URL as written (first `podcast:source` for alternate enclosures).
    pub url: String,
    /// Declared MIME type.
    pub mime_type: Option<String>,
    /// Declared length as written.
    pub length: Option<String>,
    /// Where it came from.
    pub origin: EnclosureOrigin,
    /// `bitrate` attribute.
    pub bitrate: Option<String>,
    /// `height` attribute.
    pub height: Option<String>,
    /// `codecs` attribute.
    pub codecs: Option<String>,
    /// `lang` attribute.
    pub lang: Option<String>,
    /// `title` attribute.
    pub title: Option<String>,
    /// `default="true"` on an alternate enclosure.
    pub default: bool,
    /// `podcast:integrity@type`.
    pub integrity_type: Option<String>,
    /// `podcast:integrity@value`.
    pub integrity_value: Option<String>,
    /// Additional `podcast:source` URIs.
    pub sources: Vec<String>,
}

/// A `podcast:value` block, kept structurally.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ParsedValue {
    /// `type` attribute.
    pub kind: Option<String>,
    /// `method` attribute.
    pub method: Option<String>,
    /// `suggested` attribute.
    pub suggested: Option<String>,
    /// Recipients with all their attributes.
    pub recipients: Vec<HashMap<String, String>>,
}

/// One feed item as written.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ParsedItem {
    /// Zero-based position in the document.
    pub index: usize,
    /// `guid` / Atom `id`.
    pub guid: Option<String>,
    /// `guid@isPermaLink`.
    pub guid_is_permalink: Option<bool>,
    /// Title (`title`, else `itunes:title`).
    pub title: Option<String>,
    /// `itunes:title` when it differs from `title`.
    pub itunes_title: Option<String>,
    /// `itunes:subtitle`.
    pub subtitle: Option<String>,
    /// `description` / Atom `summary`, markup kept.
    pub description: Option<String>,
    /// `content:encoded` / Atom `content`, markup kept.
    pub content: Option<String>,
    /// `itunes:summary`.
    pub summary: Option<String>,
    /// `link` / Atom `link rel="alternate"`.
    pub link: Option<String>,
    /// `pubDate` / `dc:date` / Atom `published`, as written.
    pub pub_date: Option<String>,
    /// Atom `updated`, as written.
    pub updated: Option<String>,
    /// `itunes:duration`, as written.
    pub duration: Option<String>,
    /// `itunes:season` / `podcast:season`.
    pub season: Option<String>,
    /// `itunes:episode` / `podcast:episode`.
    pub episode: Option<String>,
    /// `itunes:episodeType`.
    pub episode_type: Option<String>,
    /// `itunes:explicit`, as written.
    pub explicit: Option<String>,
    /// `itunes:image@href`.
    pub image: Option<String>,
    /// `itunes:author` / `dc:creator` / Atom author name / `author`.
    pub author: Option<String>,
    /// Media references in document order (primary candidates first).
    pub enclosures: Vec<ParsedEnclosure>,
    /// `podcast:chapters`.
    pub chapters: Vec<ChaptersRef>,
    /// `podcast:transcript`.
    pub transcripts: Vec<TranscriptRef>,
    /// `podcast:person`.
    pub persons: Vec<Person>,
    /// `podcast:location`.
    pub location: Option<Location>,
    /// `podcast:soundbite`.
    pub soundbites: Vec<Soundbite>,
    /// `podcast:value`.
    pub value: Option<ParsedValue>,
    /// `podcast:license`.
    pub license: Option<License>,
    /// `podcast:txt`.
    pub txt: Vec<Txt>,
    /// Elements Uguisu does not model (direct children of the item).
    pub raw_extensions: Vec<RawExtension>,
    /// Problems noticed in this item.
    pub warnings: Vec<String>,
}

/// An item that could not be parsed reliably.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MalformedItem {
    /// Zero-based position in the document.
    pub index: usize,
    /// What went wrong.
    pub reason: String,
    /// Title collected before the problem, if any.
    pub partial_title: Option<String>,
    /// GUID collected before the problem, if any.
    pub partial_guid: Option<String>,
}

/// Channel / feed level data as written.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ParsedChannel {
    /// Title.
    pub title: Option<String>,
    /// `itunes:subtitle`.
    pub subtitle: Option<String>,
    /// `description` / Atom `subtitle`, markup kept.
    pub description: Option<String>,
    /// `itunes:summary`.
    pub summary: Option<String>,
    /// Website (`link` / Atom `link rel="alternate"`).
    pub link: Option<String>,
    /// `language` / `dc:language` / `xml:lang`.
    pub language: Option<String>,
    /// `itunes:author` / Atom author name.
    pub author: Option<String>,
    /// `managingEditor` / `dc:publisher`.
    pub publisher: Option<String>,
    /// `itunes:owner/itunes:name`.
    pub owner_name: Option<String>,
    /// `itunes:owner/itunes:email`.
    pub owner_email: Option<String>,
    /// `copyright` / Atom `rights`.
    pub copyright: Option<String>,
    /// Artwork (`itunes:image@href`, `image/url`, Atom `logo`).
    pub image: Option<String>,
    /// Categories (`itunes:category` flattened as `Parent / Child`, then `category`).
    pub categories: Vec<String>,
    /// `itunes:explicit`, as written.
    pub explicit: Option<String>,
    /// `itunes:type`.
    pub podcast_type: Option<String>,
    /// `itunes:new-feed-url`.
    pub new_feed_url: Option<String>,
    /// `podcast:guid`.
    pub podcast_guid: Option<String>,
    /// `podcast:locked`, as written.
    pub locked: Option<String>,
    /// `podcast:medium`.
    pub medium: Option<String>,
    /// `podcast:funding`.
    pub funding: Vec<Funding>,
    /// `atom:link rel="self"`.
    pub self_link: Option<String>,
    /// `lastBuildDate` / `pubDate` / Atom `updated`, as written.
    pub updated: Option<String>,
    /// `generator`.
    pub generator: Option<String>,
    /// Elements Uguisu does not model.
    pub raw_extensions: Vec<RawExtension>,
}

/// Counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ParseStats {
    /// Items completed (well-formed).
    pub items: usize,
    /// Items with at least one media reference.
    pub items_with_enclosure: usize,
    /// Items marked malformed.
    pub malformed_items: usize,
    /// Entity references that were not expanded.
    pub unknown_refs: usize,
    /// Input size in bytes.
    pub bytes: usize,
}

/// A parsed feed document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedFeed {
    /// Syntax family.
    pub kind: FeedKind,
    /// Encoding the body was decoded with.
    pub encoding: String,
    /// Channel-level data.
    pub channel: ParsedChannel,
    /// Items in document order.
    pub items: Vec<ParsedItem>,
    /// Items that could not be parsed.
    pub malformed_items: Vec<MalformedItem>,
    /// The document was cut short (XML error after items, depth or item limit).
    pub truncated: bool,
    /// Document-level warnings.
    pub warnings: Vec<String>,
    /// Counters.
    pub stats: ParseStats,
}

impl ParsedFeed {
    /// A feed is a podcast feed when at least one item carries media.
    #[must_use]
    pub const fn looks_like_podcast(&self) -> bool {
        self.stats.items_with_enclosure >= 1
    }

    /// Some items were malformed.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        !self.malformed_items.is_empty()
    }
}

/// Parses a feed body within the given limits.
pub fn parse(bytes: &[u8], limits: &FeedLimits) -> Result<ParsedFeed, ParseError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(ParseError::Empty);
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_bytes {
        return Err(ParseError::TooLarge {
            size: bytes.len(),
            limit: limits.max_bytes,
        });
    }
    let decoded = decode_bytes(bytes).map_err(|e| ParseError::UnsupportedEncoding(e.0))?;
    let text = decoded
        .text
        .trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    if !text.starts_with('<') {
        return Err(ParseError::NotXml {
            looks_like_html: looks_like_html(text),
        });
    }
    if looks_like_html(text) {
        return Err(ParseError::NotXml {
            looks_like_html: true,
        });
    }

    let mut reader = Reader::from_str(text);
    let cfg = reader.config_mut();
    // End tags are matched by this parser's own stack so a mismatched tag
    // isolates one item instead of aborting the document.
    cfg.check_end_names = false;
    cfg.expand_empty_elements = false;
    cfg.trim_text_start = false;
    cfg.trim_text_end = false;

    let mut p = Parser::new(limits, decoded.encoding, decoded.warnings, bytes.len());
    loop {
        if p.stop {
            break;
        }
        let event = match reader.read_event() {
            Ok(ev) => ev,
            Err(err) => {
                // Syntax errors are fatal for the reader; keep what we have.
                if p.stats.items > 0 || !p.malformed.is_empty() {
                    p.warnings.push(format!(
                        "stopped at malformed xml after {} items: {err}",
                        p.stats.items
                    ));
                    p.truncated = true;
                    break;
                }
                return Err(ParseError::Malformed(err.to_string()));
            }
        };
        match event {
            Event::Start(e) => p.start(&e)?,
            Event::Empty(e) => {
                p.start(&e)?;
                p.end(None);
            }
            Event::End(e) => {
                let local = e.name().local_name().as_ref().to_ascii_lowercase();
                p.end(Some(&local));
            }
            Event::Text(t) => p.text(&t.xml10_content()),
            Event::CData(c) => p.text(&c.into_inner()),
            Event::GeneralRef(r) => {
                if let Some(resolved) = resolve_reference(&r) {
                    p.text(&resolved);
                } else {
                    p.stats.unknown_refs += 1;
                }
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    p.finish()
}

pub(crate) fn looks_like_html(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(512)
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html") || head.contains("<html")
}

/// Canonical namespace prefixes by URI.
fn canonical_prefix(uri: &str) -> Option<&'static str> {
    let u = uri.trim().trim_end_matches('/').to_ascii_lowercase();
    Some(match u.as_str() {
        "http://www.itunes.com/dtds/podcast-1.0.dtd" => "itunes",
        "https://podcastindex.org/namespace/1.0"
        | "http://podcastindex.org/namespace/1.0"
        | "https://github.com/podcastindex-org/podcast-namespace/blob/main/docs/1.0.md" => {
            "podcast"
        }
        "http://www.w3.org/2005/atom" => "atom",
        "http://purl.org/rss/1.0/modules/content" => "content",
        "http://search.yahoo.com/mrss" | "http://video.search.yahoo.com/mrss" => "media",
        "http://purl.org/dc/elements/1.1" | "http://purl.org/dc/terms" => "dc",
        "http://purl.oclc.org/net/rss_2.0/enc#" => "enc",
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#" => "rdf",
        "http://purl.org/rss/1.0" => "",
        _ => return None,
    })
}

#[derive(Debug)]
struct Frame {
    prefix: String,
    local: String,
    attrs: Vec<(String, String)>,
    text: String,
    overflow: bool,
    /// Namespace prefixes this element declared, with the mapping they
    /// shadowed (restored when the element closes).
    ns_shadowed: Vec<(String, Option<String>)>,
}

impl Frame {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name) || k.rsplit(':').next() == Some(name))
            .map(|(_, v)| v.as_str())
    }
}

struct Parser<'l> {
    limits: &'l FeedLimits,
    kind: Option<FeedKind>,
    channel: ParsedChannel,
    items: Vec<ParsedItem>,
    malformed: Vec<MalformedItem>,
    warnings: Vec<String>,
    stats: ParseStats,
    truncated: bool,
    stop: bool,
    encoding: String,
    stack: Vec<Frame>,
    /// Declared prefix → canonical prefix.
    ns: HashMap<String, String>,
    /// Depth (stack length) of the open item, if any.
    item_depth: Option<usize>,
    current: ParsedItem,
    current_error: Option<String>,
    item_index: usize,
    /// Pending alternate enclosure while its children are read.
    alt: Option<ParsedEnclosure>,
    /// Pending `podcast:value`.
    value: Option<ParsedValue>,
    /// Nested `itunes:category` names.
    category_path: Vec<String>,
    channel_seen: bool,
}

impl<'l> Parser<'l> {
    fn new(limits: &'l FeedLimits, encoding: &str, warnings: Vec<String>, bytes: usize) -> Self {
        Self {
            limits,
            kind: None,
            channel: ParsedChannel::default(),
            items: Vec::new(),
            malformed: Vec::new(),
            warnings,
            stats: ParseStats {
                bytes,
                ..ParseStats::default()
            },
            truncated: false,
            stop: false,
            encoding: encoding.to_owned(),
            stack: Vec::new(),
            ns: HashMap::new(),
            item_depth: None,
            current: ParsedItem::default(),
            current_error: None,
            item_index: 0,
            alt: None,
            value: None,
            category_path: Vec::new(),
            channel_seen: false,
        }
    }

    fn depth(&self) -> usize {
        self.stack.len()
    }

    fn in_item(&self) -> bool {
        self.item_depth.is_some()
    }

    fn canon(&self, prefix: &str) -> String {
        self.ns
            .get(prefix)
            .cloned()
            .unwrap_or_else(|| prefix.to_owned())
    }

    #[allow(clippy::too_many_lines)] // element dispatch reads best as one pass
    fn start(&mut self, e: &BytesStart<'_>) -> Result<(), ParseError> {
        if self.depth() >= self.limits.max_depth {
            if self.stats.items > 0 || !self.malformed.is_empty() {
                self.warnings.push(format!(
                    "stopped: nesting deeper than {} levels after {} items",
                    self.limits.max_depth, self.stats.items
                ));
                self.truncated = true;
                self.stop = true;
                return Ok(());
            }
            return Err(ParseError::TooDeep(self.limits.max_depth));
        }
        let qname = e.name();
        let local = qname.local_name().as_ref().to_ascii_lowercase();
        let raw_prefix = qname
            .prefix()
            .map(|p| p.as_ref().to_ascii_lowercase())
            .unwrap_or_default();

        // Collect attributes and namespace declarations (scoped to this element).
        let mut attrs = Vec::new();
        let mut ns_shadowed = Vec::new();
        for a in e.attributes().with_checks(false).flatten() {
            let key = a.key.as_ref().to_ascii_lowercase();
            let Ok(value) = a.normalized_value(quick_xml::XmlVersion::Implicit1_0) else {
                continue;
            };
            let value = value.trim().to_owned();
            let declared = if key == "xmlns" {
                Some(String::new())
            } else {
                key.strip_prefix("xmlns:").map(str::to_owned)
            };
            if let Some(declared) = declared {
                // A declaration binds the prefix to the canonical name of a
                // known namespace, or to the prefix itself for unknown ones
                // (so a default namespace on a foreign element does not turn
                // unprefixed children into feed elements).
                let mapped = canonical_prefix(&value).map_or_else(
                    || {
                        if declared.is_empty() {
                            String::new()
                        } else {
                            declared.clone()
                        }
                    },
                    str::to_owned,
                );
                let previous = self.ns.insert(declared.clone(), mapped);
                ns_shadowed.push((declared, previous));
            } else {
                attrs.push((key, cap(&value, self.limits.max_field_bytes)));
            }
        }
        let prefix = self.canon(&raw_prefix);

        if self.stack.is_empty() {
            self.kind = Some(match local.as_str() {
                "rss" => FeedKind::Rss2,
                "feed" => FeedKind::Atom,
                "rdf" => FeedKind::Rss1,
                other => return Err(ParseError::UnknownRoot(other.to_owned())),
            });
            if let Some(lang) = attrs.iter().find(|(k, _)| k == "xml:lang") {
                self.channel.language = Some(lang.1.clone());
            }
        }
        let kind = self.kind.unwrap_or(FeedKind::Rss2);
        self.stack.push(Frame {
            prefix: prefix.clone(),
            local: local.clone(),
            attrs,
            text: String::new(),
            overflow: false,
            ns_shadowed,
        });

        let is_item = match kind {
            FeedKind::Rss2 => local == "item" && self.depth() == 3,
            FeedKind::Atom => local == "entry" && self.depth() == 2,
            FeedKind::Rss1 => local == "item" && self.depth() == 2,
        };
        if is_item && !self.in_item() {
            self.item_depth = Some(self.depth());
            self.current = ParsedItem {
                index: self.item_index,
                ..ParsedItem::default()
            };
            self.current_error = None;
            return Ok(());
        }
        if local == "channel" && !self.in_item() {
            self.channel_seen = true;
        }
        if self.in_item() {
            match (prefix.as_str(), local.as_str()) {
                ("podcast", "alternateenclosure") => {
                    let f = self.top();
                    self.alt = Some(ParsedEnclosure {
                        url: String::new(),
                        mime_type: f.attr("type").map(str::to_owned),
                        length: f.attr("length").map(str::to_owned),
                        origin: EnclosureOrigin::AlternateEnclosure,
                        bitrate: f.attr("bitrate").map(str::to_owned),
                        height: f.attr("height").map(str::to_owned),
                        codecs: f.attr("codecs").map(str::to_owned),
                        lang: f.attr("lang").map(str::to_owned),
                        title: f.attr("title").map(str::to_owned),
                        default: f
                            .attr("default")
                            .is_some_and(|v| v.eq_ignore_ascii_case("true")),
                        integrity_type: None,
                        integrity_value: None,
                        sources: Vec::new(),
                    });
                }
                ("podcast", "value") => {
                    let f = self.top();
                    self.value = Some(ParsedValue {
                        kind: f.attr("type").map(str::to_owned),
                        method: f.attr("method").map(str::to_owned),
                        suggested: f.attr("suggested").map(str::to_owned),
                        recipients: Vec::new(),
                    });
                }
                _ => {}
            }
        } else if prefix == "itunes"
            && local == "category"
            && let Some(text) = self.top().attr("text")
        {
            self.category_path.push(text.to_owned());
            let flat = self.category_path.join(" / ");
            if !self.channel.categories.contains(&flat) {
                self.channel.categories.push(flat);
            }
        }
        Ok(())
    }

    fn top(&self) -> &Frame {
        self.stack
            .last()
            .unwrap_or_else(|| unreachable!("frame pushed before use"))
    }

    fn text(&mut self, s: &str) {
        let max = self.limits.max_text_bytes;
        let Some(f) = self.stack.last_mut() else {
            return;
        };
        if f.text.len() >= max {
            f.overflow = true;
            return;
        }
        let room = max - f.text.len();
        if s.len() > room {
            let cut = s
                .char_indices()
                .take_while(|(i, _)| *i < room)
                .last()
                .map_or(0, |(i, c)| i + c.len_utf8());
            f.text.push_str(&s[..cut]);
            f.overflow = true;
        } else {
            f.text.push_str(s);
        }
    }

    /// Handles an end event. `found` is the local name of the end tag
    /// (`None` for empty elements). A name that does not match the open
    /// element closes it anyway and marks the current item malformed.
    fn end(&mut self, found: Option<&str>) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        for (declared, previous) in frame.ns_shadowed.iter().rev() {
            match previous {
                Some(p) => {
                    self.ns.insert(declared.clone(), p.clone());
                }
                None => {
                    self.ns.remove(declared);
                }
            }
        }
        if let Some(found) = found
            && found != frame.local
        {
            let reason = format!(
                "mismatched end tag: expected </{}>, found </{found}>",
                frame.local
            );
            if self.in_item() {
                self.current_error.get_or_insert(reason);
            } else {
                self.warnings.push(reason);
            }
        }
        if frame.overflow {
            let msg = format!(
                "text of <{}> was truncated to {} bytes",
                frame.local, self.limits.max_text_bytes
            );
            if self.in_item() {
                self.current.warnings.push(msg);
            } else {
                self.warnings.push(msg);
            }
        }
        let depth_after = self.depth();
        if let Some(item_depth) = self.item_depth
            && depth_after + 1 == item_depth
        {
            // The item element itself closed.
            self.finish_item();
            return;
        }
        if self.in_item() {
            self.item_element(&frame, depth_after);
        } else {
            self.channel_element(&frame, depth_after);
        }
    }

    fn finish_item(&mut self) {
        self.item_depth = None;
        let item = std::mem::take(&mut self.current);
        self.alt = None;
        self.value = None;
        if let Some(reason) = self.current_error.take() {
            self.malformed.push(MalformedItem {
                index: item.index,
                reason,
                partial_title: item.title,
                partial_guid: item.guid,
            });
            self.stats.malformed_items += 1;
            if self.malformed.len() >= self.limits.max_malformed_items {
                self.warnings.push(format!(
                    "stopped after {} malformed items",
                    self.malformed.len()
                ));
                self.truncated = true;
                self.stop = true;
            }
        } else {
            if !item.enclosures.is_empty() {
                self.stats.items_with_enclosure += 1;
            }
            self.stats.items += 1;
            self.items.push(item);
            if self.items.len() >= self.limits.max_items {
                self.warnings
                    .push(format!("stopped after {} items (limit)", self.items.len()));
                self.truncated = true;
                self.stop = true;
            }
        }
        self.item_index += 1;
    }

    #[allow(clippy::too_many_lines)] // one match arm per element keeps the mapping readable
    fn item_element(&mut self, f: &Frame, depth_after: usize) {
        let kind = self.kind.unwrap_or(FeedKind::Rss2);
        let item_depth = self.item_depth.unwrap_or(0);
        let direct = depth_after == item_depth;
        let value = f.text.trim().to_owned();
        let short = |s: &str| cap(s, self.limits.max_field_bytes);
        let parent_local = self
            .stack
            .last()
            .map(|p| (p.prefix.clone(), p.local.clone()));

        // Children of alternateEnclosure / value.
        if let Some(alt) = self.alt.as_mut()
            && parent_local
                .as_ref()
                .is_some_and(|p| p.0 == "podcast" && p.1 == "alternateenclosure")
        {
            match (f.prefix.as_str(), f.local.as_str()) {
                ("podcast", "source") => {
                    if let Some(uri) = f.attr("uri") {
                        if alt.url.is_empty() {
                            uri.clone_into(&mut alt.url);
                            if alt.mime_type.is_none() {
                                alt.mime_type = f.attr("contenttype").map(str::to_owned);
                            }
                        } else {
                            alt.sources.push(uri.to_owned());
                        }
                    }
                }
                ("podcast", "integrity") => {
                    alt.integrity_type = f.attr("type").map(str::to_owned);
                    alt.integrity_value = f.attr("value").map(str::to_owned);
                }
                _ => {}
            }
            return;
        }
        if let Some(v) = self.value.as_mut()
            && f.prefix == "podcast"
            && f.local == "valuerecipient"
        {
            v.recipients.push(f.attrs.iter().cloned().collect());
            return;
        }
        if !direct {
            // Atom author/name, RDF nested, etc.
            if kind == FeedKind::Atom
                && f.local == "name"
                && parent_local.as_ref().is_some_and(|p| p.1 == "author")
                && self.current.author.is_none()
            {
                self.current.author = non_empty(short(&value));
            }
            return;
        }
        let item = &mut self.current;
        match (f.prefix.as_str(), f.local.as_str()) {
            ("" | "atom", "title") => {
                if item.title.is_none() {
                    item.title = non_empty(short(&value));
                }
            }
            ("itunes", "title") => {
                let t = non_empty(short(&value));
                if item.title.is_none() {
                    item.title = t.clone();
                } else if t.is_some() && t != item.title {
                    item.itunes_title = t;
                }
            }
            ("", "guid") | ("atom", "id") => {
                if item.guid.is_none() {
                    item.guid = non_empty(short(&value));
                    item.guid_is_permalink = f
                        .attr("ispermalink")
                        .map(|v| !v.eq_ignore_ascii_case("false"));
                }
            }
            ("itunes", "subtitle") => item.subtitle = non_empty(short(&value)),
            ("itunes", "summary") => item.summary = non_empty(value),
            ("", "description") | ("atom", "summary") => {
                if item.description.is_none() {
                    item.description = non_empty(value);
                }
            }
            ("content", "encoded") | ("atom", "content") => {
                if item.content.is_none() {
                    item.content = non_empty(value);
                }
            }
            ("" | "atom", "link") => {
                if kind == FeedKind::Atom || f.attr("href").is_some() {
                    let rel = f.attr("rel").unwrap_or("alternate");
                    match rel {
                        "enclosure" => {
                            if let Some(href) = f.attr("href") {
                                item.enclosures.push(ParsedEnclosure {
                                    url: href.to_owned(),
                                    mime_type: f.attr("type").map(str::to_owned),
                                    length: f.attr("length").map(str::to_owned),
                                    origin: EnclosureOrigin::AtomLink,
                                    bitrate: None,
                                    height: None,
                                    codecs: None,
                                    lang: None,
                                    title: f.attr("title").map(str::to_owned),
                                    default: false,
                                    integrity_type: None,
                                    integrity_value: None,
                                    sources: Vec::new(),
                                });
                            }
                        }
                        "alternate" if item.link.is_none() => {
                            item.link = f.attr("href").map(str::to_owned);
                        }
                        _ => {}
                    }
                } else if item.link.is_none() {
                    item.link = non_empty(short(&value));
                }
            }
            ("", "pubdate") | ("dc", "date") | ("atom", "published") => {
                if item.pub_date.is_none() {
                    item.pub_date = non_empty(short(&value));
                }
            }
            ("" | "atom", "updated") | ("dc", "modified") => {
                item.updated = non_empty(short(&value));
            }
            ("itunes", "duration") => item.duration = non_empty(short(&value)),
            ("itunes" | "podcast", "season") => {
                if item.season.is_none() {
                    item.season = non_empty(short(&value));
                }
            }
            ("itunes" | "podcast", "episode") => {
                if item.episode.is_none() {
                    item.episode = non_empty(short(&value));
                }
            }
            ("itunes", "episodetype") => item.episode_type = non_empty(short(&value)),
            ("itunes", "explicit") => item.explicit = non_empty(short(&value)),
            ("itunes", "image") => {
                if let Some(href) = f.attr("href") {
                    item.image = Some(href.to_owned());
                } else if item.image.is_none() {
                    item.image = non_empty(short(&value));
                }
            }
            ("itunes", "author") | ("dc", "creator") => {
                if item.author.is_none() || f.prefix == "itunes" {
                    item.author = non_empty(short(&value));
                }
            }
            ("", "author") => {
                if item.author.is_none() {
                    item.author = non_empty(short(&value));
                }
            }
            ("", "enclosure") => {
                if let Some(url) = f.attr("url") {
                    item.enclosures.push(ParsedEnclosure {
                        url: url.to_owned(),
                        mime_type: f.attr("type").map(str::to_owned),
                        length: f.attr("length").map(str::to_owned),
                        origin: EnclosureOrigin::Enclosure,
                        bitrate: None,
                        height: None,
                        codecs: None,
                        lang: None,
                        title: None,
                        default: false,
                        integrity_type: None,
                        integrity_value: None,
                        sources: Vec::new(),
                    });
                } else {
                    item.warnings
                        .push("enclosure without url attribute".to_owned());
                }
            }
            ("enc", "enclosure") => {
                if let Some(url) = f.attr("rdf:resource").or_else(|| f.attr("resource")) {
                    item.enclosures.push(ParsedEnclosure {
                        url: url.to_owned(),
                        mime_type: f
                            .attr("enc:type")
                            .or_else(|| f.attr("type"))
                            .map(str::to_owned),
                        length: f
                            .attr("enc:length")
                            .or_else(|| f.attr("length"))
                            .map(str::to_owned),
                        origin: EnclosureOrigin::RdfEnc,
                        bitrate: None,
                        height: None,
                        codecs: None,
                        lang: None,
                        title: None,
                        default: false,
                        integrity_type: None,
                        integrity_value: None,
                        sources: Vec::new(),
                    });
                }
            }
            ("media", "content") => {
                if let Some(url) = f.attr("url") {
                    item.enclosures.push(ParsedEnclosure {
                        url: url.to_owned(),
                        mime_type: f.attr("type").map(str::to_owned),
                        length: f.attr("filesize").map(str::to_owned),
                        origin: EnclosureOrigin::MediaContent,
                        bitrate: f.attr("bitrate").map(str::to_owned),
                        height: f.attr("height").map(str::to_owned),
                        codecs: None,
                        lang: f.attr("lang").map(str::to_owned),
                        title: None,
                        default: false,
                        integrity_type: None,
                        integrity_value: None,
                        sources: Vec::new(),
                    });
                }
            }
            ("podcast", "alternateenclosure") => {
                if let Some(alt) = self.alt.take() {
                    if alt.url.is_empty() {
                        item.warnings
                            .push("alternateEnclosure without a source uri".to_owned());
                    } else {
                        item.enclosures.push(alt);
                    }
                }
            }
            ("podcast", "chapters") => {
                if let Some(url) = f.attr("url") {
                    item.chapters.push(ChaptersRef {
                        url: url.to_owned(),
                        mime_type: f.attr("type").map(str::to_owned),
                    });
                }
            }
            ("podcast", "transcript") => {
                if let Some(url) = f.attr("url") {
                    item.transcripts.push(TranscriptRef {
                        url: url.to_owned(),
                        mime_type: f.attr("type").map(str::to_owned),
                        language: f.attr("language").map(str::to_owned),
                        rel: f.attr("rel").map(str::to_owned),
                    });
                }
            }
            ("podcast", "person") => {
                if let Some(name) = non_empty(short(&value)) {
                    item.persons.push(Person {
                        name,
                        role: f.attr("role").map(str::to_owned),
                        group: f.attr("group").map(str::to_owned),
                        img: f.attr("img").map(str::to_owned),
                        href: f.attr("href").map(str::to_owned),
                    });
                }
            }
            ("podcast", "location") => {
                if let Some(name) = non_empty(short(&value)) {
                    item.location = Some(Location {
                        name,
                        geo: f.attr("geo").map(str::to_owned),
                        osm: f.attr("osm").map(str::to_owned),
                    });
                }
            }
            ("podcast", "soundbite") => {
                let start = f.attr("starttime").and_then(|v| v.parse::<f64>().ok());
                let dur = f.attr("duration").and_then(|v| v.parse::<f64>().ok());
                if let (Some(start_time), Some(duration)) = (start, dur) {
                    item.soundbites.push(Soundbite {
                        start_time,
                        duration,
                        title: non_empty(short(&value)),
                    });
                }
            }
            ("podcast", "value") => item.value = self.value.take(),
            ("podcast", "license") => {
                if let Some(text) = non_empty(short(&value)) {
                    item.license = Some(License {
                        text,
                        url: f.attr("url").map(str::to_owned),
                    });
                }
            }
            ("podcast", "txt") => {
                if let Some(v) = non_empty(short(&value)) {
                    item.txt.push(Txt {
                        purpose: f.attr("purpose").map(str::to_owned),
                        value: v,
                    });
                }
            }
            ("itunes", "keywords" | "block" | "order" | "isclosedcaptioned")
            | ("", "category" | "comments" | "source")
            | (
                "media",
                "thumbnail" | "keywords" | "credit" | "rating" | "title" | "description"
                | "category",
            ) => {}
            _ => {
                if item.raw_extensions.len() < self.limits.max_raw_extensions_per_item {
                    item.raw_extensions.push(RawExtension {
                        name: if f.prefix.is_empty() {
                            f.local.clone()
                        } else {
                            format!("{}:{}", f.prefix, f.local)
                        },
                        attributes: f.attrs.iter().cloned().collect(),
                        text: non_empty(short(&value)),
                    });
                }
            }
        }
        if item.enclosures.len() > self.limits.max_enclosures_per_item {
            item.enclosures
                .truncate(self.limits.max_enclosures_per_item);
            item.warnings.push(format!(
                "enclosures truncated to {}",
                self.limits.max_enclosures_per_item
            ));
        }
    }

    #[allow(clippy::too_many_lines)] // one match arm per element keeps the mapping readable
    fn channel_element(&mut self, f: &Frame, depth_after: usize) {
        let kind = self.kind.unwrap_or(FeedKind::Rss2);
        let value = f.text.trim().to_owned();
        let short = |s: &str| cap(s, self.limits.max_field_bytes);
        let parent = self
            .stack
            .last()
            .map(|p| (p.prefix.clone(), p.local.clone()));
        let at_channel = match kind {
            FeedKind::Rss2 | FeedKind::Rss1 => {
                depth_after == 2 && parent.as_ref().is_some_and(|p| p.1 == "channel")
            }
            FeedKind::Atom => depth_after == 1,
        };
        // Nested channel children.
        if !at_channel {
            let grand = self.stack.iter().rev().nth(1).map(|p| p.local.clone());
            let under_channel = match kind {
                FeedKind::Rss2 | FeedKind::Rss1 => {
                    depth_after == 3 && grand.as_deref() == Some("channel")
                }
                FeedKind::Atom => depth_after == 2,
            };
            if under_channel {
                match (
                    parent.as_ref().map(|p| p.1.as_str()),
                    f.prefix.as_str(),
                    f.local.as_str(),
                ) {
                    (Some("image"), "", "url") => {
                        if self.channel.image.is_none() {
                            self.channel.image = non_empty(short(&value));
                        }
                    }
                    (Some("owner"), "itunes", "name") => {
                        self.channel.owner_name = non_empty(short(&value));
                    }
                    (Some("owner"), "itunes", "email") => {
                        self.channel.owner_email = non_empty(short(&value));
                    }
                    (Some("author"), _, "name")
                        if kind == FeedKind::Atom && self.channel.author.is_none() =>
                    {
                        self.channel.author = non_empty(short(&value));
                    }
                    _ => {}
                }
            }
            if f.prefix == "itunes" && f.local == "category" {
                self.category_path.pop();
            }
            return;
        }
        if f.prefix == "itunes" && f.local == "category" {
            self.category_path.pop();
            return;
        }
        let ch = &mut self.channel;
        match (f.prefix.as_str(), f.local.as_str()) {
            ("" | "atom", "title") => {
                if ch.title.is_none() {
                    ch.title = non_empty(collapse_ws(&short(&value)));
                }
            }
            ("itunes", "subtitle") => ch.subtitle = non_empty(short(&value)),
            ("", "description") | ("atom", "subtitle") => {
                if ch.description.is_none() {
                    ch.description = non_empty(value);
                }
            }
            ("itunes", "summary") => ch.summary = non_empty(value),
            ("" | "atom", "link") => {
                if kind == FeedKind::Atom || f.attr("href").is_some() {
                    let rel = f.attr("rel").unwrap_or("alternate");
                    if rel == "self" && ch.self_link.is_none() {
                        ch.self_link = f.attr("href").map(str::to_owned);
                    } else if rel == "alternate" && ch.link.is_none() {
                        ch.link = f.attr("href").map(str::to_owned);
                    }
                } else if ch.link.is_none() {
                    ch.link = non_empty(short(&value));
                }
            }
            ("" | "dc", "language") => ch.language = non_empty(short(&value)),
            ("itunes", "author") => ch.author = non_empty(collapse_ws(&short(&value))),
            ("", "managingeditor") | ("dc", "publisher" | "creator") => {
                if ch.publisher.is_none() {
                    ch.publisher = non_empty(short(&value));
                }
            }
            ("", "copyright") | ("atom" | "dc", "rights") => {
                ch.copyright = non_empty(short(&value));
            }
            ("itunes", "image") => {
                if let Some(href) = f.attr("href") {
                    ch.image = Some(href.to_owned());
                }
            }
            ("", "logo") if kind == FeedKind::Atom => {
                if ch.image.is_none() {
                    ch.image = non_empty(short(&value));
                }
            }
            ("", "category") => {
                if let Some(c) = non_empty(short(&value))
                    && !ch.categories.contains(&c)
                {
                    ch.categories.push(c);
                }
            }
            ("itunes", "explicit") => ch.explicit = non_empty(short(&value)),
            ("itunes", "type") => ch.podcast_type = non_empty(short(&value)),
            ("itunes", "new-feed-url") => ch.new_feed_url = non_empty(short(&value)),
            ("podcast", "guid") => ch.podcast_guid = non_empty(short(&value)),
            ("podcast", "locked") => ch.locked = non_empty(short(&value)),
            ("podcast", "medium") => ch.medium = non_empty(short(&value)),
            ("podcast", "funding") => {
                if let Some(url) = f.attr("url") {
                    ch.funding.push(Funding {
                        url: url.to_owned(),
                        text: non_empty(short(&value)),
                    });
                }
            }
            ("", "lastbuilddate" | "pubdate") | ("atom", "updated") => {
                if ch.updated.is_none() || f.local == "lastbuilddate" {
                    ch.updated = non_empty(short(&value));
                }
            }
            ("", "generator") => ch.generator = non_empty(short(&value)),
            (
                "",
                "image" | "docs" | "ttl" | "webmaster" | "cloud" | "rating" | "skiphours"
                | "skipdays" | "textinput" | "items",
            )
            | ("itunes", "owner" | "keywords" | "block" | "complete")
            | ("atom", "id" | "author" | "contributor" | "icon")
            | ("rdf", _) => {}
            _ => {
                if ch.raw_extensions.len() < self.limits.max_raw_extensions_per_item {
                    ch.raw_extensions.push(RawExtension {
                        name: if f.prefix.is_empty() {
                            f.local.clone()
                        } else {
                            format!("{}:{}", f.prefix, f.local)
                        },
                        attributes: f.attrs.iter().cloned().collect(),
                        text: non_empty(short(&value)),
                    });
                }
            }
        }
    }

    fn finish(mut self) -> Result<ParsedFeed, ParseError> {
        let Some(kind) = self.kind else {
            return Err(ParseError::NotXml {
                looks_like_html: false,
            });
        };
        if self.in_item() {
            // Document ended inside an item: keep it as malformed.
            self.current_error
                .get_or_insert_with(|| "document ended inside the item".to_owned());
            self.finish_item();
            self.truncated = true;
        }
        if self.items.is_empty() && !self.stack.is_empty() {
            // Nothing usable and the document never closed: treat it like
            // the probe does and refuse it outright.
            let reason = self.malformed.first().map_or_else(
                || "document ended with open elements".to_owned(),
                |m| m.reason.clone(),
            );
            return Err(ParseError::Malformed(reason));
        }
        if self.stats.unknown_refs > 0 {
            self.warnings.push(format!(
                "{} entity references were not expanded",
                self.stats.unknown_refs
            ));
        }
        if !self.stack.is_empty() {
            self.warnings
                .push("document ended with open elements".to_owned());
        }
        if kind != FeedKind::Atom && !self.channel_seen {
            self.warnings.push("no <channel> element".to_owned());
        }
        Ok(ParsedFeed {
            kind,
            encoding: self.encoding,
            channel: self.channel,
            items: self.items,
            malformed_items: self.malformed,
            truncated: self.truncated,
            warnings: self.warnings,
            stats: self.stats,
        })
    }
}

fn non_empty(s: String) -> Option<String> {
    (!s.trim().is_empty()).then_some(s)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncates on a char boundary.
fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}
