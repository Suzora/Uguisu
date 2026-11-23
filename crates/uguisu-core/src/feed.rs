//! Tolerant parser for podcast feeds: RSS 2.0 (and its 0.9x predecessors), Atom and RSS 1.0.
//!
//! Only the fields Uguisu indexes are extracted. Nothing is lost by that: the complete feed is
//! stored byte for byte as a snapshot, and the raw `<item>` XML is kept with every revision.

use anyhow::{Context, Result, bail};
use roxmltree::{Document, Node, ParsingOptions};

const NS_ITUNES: &str = "http://www.itunes.com/dtds/podcast-1.0.dtd";
const NS_PODCAST: &str = "https://podcastindex.org/namespace/1.0";
const NS_CONTENT: &str = "http://purl.org/rss/1.0/modules/content/";
const NS_ATOM: &str = "http://www.w3.org/2005/Atom";
const NS_RSS1: &str = "http://purl.org/rss/1.0/";
const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
const NS_ENC: &str = "http://purl.oclc.org/net/rss_2.0/enc#";
const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";
const NS_MEDIA: &str = "http://search.yahoo.com/mrss/";

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
    /// Address of the page with the next older episodes, if the feed is split into pages
    /// (RFC 5005). May be relative to the address of this page.
    pub next_page: Option<String>,
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
    /// Files the item links to besides its enclosure.
    pub links: Vec<ItemLink>,
    /// Other files of the same episode the feed offers instead of the enclosure: Podcasting
    /// 2.0 `podcast:alternateEnclosure`, Media RSS `media:content`, further Atom enclosures.
    pub alternatives: Vec<MediaOption>,
    /// Hash over the whole item, see [`content_hash`].
    pub hash: String,
    pub raw_xml: String,
}

/// One file of an episode the feed offers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaOption {
    pub url: String,
    pub mime: Option<String>,
    pub length: Option<i64>,
    /// Bits per second.
    pub bitrate: Option<f64>,
}

impl MediaOption {
    /// A playlist or stream description rather than the episode itself.
    fn is_playlist(&self) -> bool {
        let mime = self
            .mime
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let path = without_query(&self.url).to_ascii_lowercase();
        mime.contains("mpegurl")
            || mime.contains("scpls")
            || [".m3u", ".m3u8", ".pls"]
                .iter()
                .any(|ext| path.ends_with(ext))
    }

    /// `audio`, `video` or whatever else the media type or the address says.
    fn kind(&self) -> Option<String> {
        let mime = self.mime.clone().or_else(|| {
            crate::archive::extension_from_url(&self.url)
                .and_then(crate::archive::mime_from_extension)
                .map(str::to_string)
        })?;
        let kind = mime.split('/').next()?.trim().to_ascii_lowercase();
        (!kind.is_empty()).then_some(kind)
    }

    /// Whether this file is better than `other`: a higher bitrate when both state one, else a
    /// larger file when both state their size. Without a way to tell, it is not.
    fn is_better_than(&self, other: &MediaOption) -> bool {
        match (self.bitrate, other.bitrate) {
            (Some(a), Some(b)) if a != b => return a > b,
            _ => {}
        }
        matches!((self.length, other.length), (Some(a), Some(b)) if a > b)
    }
}

impl ParsedItem {
    /// The file to archive when it is not the enclosure itself: among the alternatives of the
    /// same kind (audio stays audio, video stays video), the one with the highest quality.
    /// `None` when the enclosure is as good as any.
    pub fn best_media(&self) -> Option<MediaOption> {
        let main = MediaOption {
            url: self.enclosure_url.clone()?,
            mime: self.enclosure_type.clone(),
            length: self.enclosure_length,
            bitrate: None,
        };
        let kind = main.kind()?;
        // An alternative that is the enclosure itself tells its bitrate.
        let main = self
            .alternatives
            .iter()
            .find(|a| without_query(&a.url) == without_query(&main.url))
            .map(|a| MediaOption {
                bitrate: a.bitrate,
                length: main.length.or(a.length),
                ..main.clone()
            })
            .unwrap_or(main);
        let mut best = &main;
        for option in &self.alternatives {
            let usable = matches!(
                url::Url::parse(&option.url)
                    .map(|u| u.scheme().to_string())
                    .as_deref(),
                Ok("http" | "https")
            );
            if usable
                && !option.is_playlist()
                && option.kind().as_deref() == Some(kind.as_str())
                && option.is_better_than(best)
            {
                best = option;
            }
        }
        (without_query(&best.url) != without_query(&main.url)).then(|| best.clone())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemLink {
    /// `transcript`, `chapters`, `image` or `notes-image` (an image shown in the show notes).
    pub kind: &'static str,
    pub url: String,
}

impl ParsedItem {
    /// Identifies this state of the item; a different value means a new revision.
    pub fn meta_hash(&self) -> String {
        self.hash.clone()
    }
}

/// Addresses are compared without query string and fragment: many hosts put per-request
/// tracking tokens there, which would otherwise look like a change at every fetch.
pub fn without_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// Hash over everything in the item: element names, attributes and text. Any change a publisher
/// makes, also to tags Uguisu does not index, therefore produces a new revision. Only the query
/// strings of addresses in attributes are left out.
fn content_hash(item: Node) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut put = |text: &str| {
        hasher.update(&(text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    };
    for node in item.descendants() {
        if node.is_element() {
            put("<");
            put(node.tag_name().namespace().unwrap_or(""));
            put(node.tag_name().name());
            for attribute in node.attributes() {
                put("@");
                put(attribute.namespace().unwrap_or(""));
                put(attribute.name());
                let value = attribute.value();
                let is_address = value.starts_with("http://") || value.starts_with("https://");
                put(if is_address {
                    without_query(value)
                } else {
                    value
                });
            }
        } else if node.is_text() {
            let text = node.text().unwrap_or("").trim();
            if !text.is_empty() {
                put("#");
                put(text);
            }
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn item_links(item: Node) -> Vec<ItemLink> {
    let mut links: Vec<ItemLink> = Vec::new();
    for node in item.children().filter(|n| n.is_element()) {
        let namespace = node.tag_name().namespace();
        let found = match node.tag_name().name() {
            "transcript" if ns_matches(namespace, Some(NS_PODCAST)) => {
                attr(node, "url").map(|url| ("transcript", url))
            }
            "chapters" if ns_matches(namespace, Some(NS_PODCAST)) => {
                attr(node, "url").map(|url| ("chapters", url))
            }
            "image" if ns_matches(namespace, Some(NS_ITUNES)) => {
                attr(node, "href").map(|url| ("image", url))
            }
            _ => None,
        };
        if let Some((kind, url)) = found
            && (url.starts_with("http://") || url.starts_with("https://"))
            && !links
                .iter()
                .any(|l| without_query(&l.url) == without_query(&url))
        {
            links.push(ItemLink { kind, url });
        }
    }
    // Images embedded in the show notes. The notes are HTML inside these elements.
    for node in item.children().filter(|n| {
        n.is_element() && matches!(n.tag_name().name(), "encoded" | "description" | "content")
    }) {
        let Some(html) = text_of(node) else { continue };
        for url in html_images(&html) {
            if links.len() >= MAX_ITEM_LINKS {
                return links;
            }
            if !links
                .iter()
                .any(|l| without_query(&l.url) == without_query(&url))
            {
                links.push(ItemLink {
                    kind: "notes-image",
                    url,
                });
            }
        }
    }
    links
}

/// Upper bound for the files archived per episode besides its media file.
const MAX_ITEM_LINKS: usize = 40;

/// Addresses of the images an HTML fragment shows with `<img src>`.
fn html_images(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(position) = lower[from..].find("<img") {
        let start = from + position + "<img".len();
        let end = start + lower[start..].find('>').unwrap_or(lower.len() - start);
        from = end;
        if let Some(url) = html_attribute(&html[start..end], "src")
            && (url.starts_with("http://") || url.starts_with("https://"))
        {
            found.push(url);
        }
    }
    found
}

/// A paged feed links its next page with `rel="next"`, an archived feed its previous archive
/// document with `rel="prev-archive"`. Both lead to older episodes.
fn next_page(channel: Node) -> Option<String> {
    let link = |rel: &str| {
        channel
            .children()
            .filter(|c| {
                c.is_element()
                    && c.tag_name().name() == "link"
                    && ns_matches(c.tag_name().namespace(), Some(NS_ATOM))
            })
            .find(|c| {
                c.attribute("rel")
                    .is_some_and(|r| r.trim().eq_ignore_ascii_case(rel))
            })
            .and_then(|c| attr(c, "href"))
    };
    link("next").or_else(|| link("prev-archive"))
}

fn ns_matches(actual: Option<&str>, wanted: Option<&str>) -> bool {
    match (actual, wanted) {
        (None, None) => true,
        (Some(a), Some(w)) => a.eq_ignore_ascii_case(w),
        _ => false,
    }
}

/// `podcast:alternateEnclosure` (bitrate in bits per second, the address in `podcast:source`)
/// and `media:content`, also inside `media:group` (bitrate in kilobits per second).
fn alternatives(item: Node) -> Vec<MediaOption> {
    let number = |node: Node, name: &str| attr(node, name).and_then(|v| v.parse::<f64>().ok());
    let mut options = Vec::new();
    for node in item.children().filter(|c| {
        c.is_element()
            && c.tag_name().name() == "alternateEnclosure"
            && c.tag_name().namespace() == Some(NS_PODCAST)
    }) {
        let Some(url) = node
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "source")
            .filter_map(|c| attr(c, "uri"))
            .find(|u| u.starts_with("http://") || u.starts_with("https://"))
        else {
            continue;
        };
        options.push(MediaOption {
            url,
            mime: attr(node, "type"),
            length: number(node, "length").map(|v| v as i64).filter(|v| *v > 0),
            bitrate: number(node, "bitrate").filter(|v| *v > 0.0),
        });
    }
    let is_media = |c: &Node, name: &str| {
        c.is_element() && c.tag_name().name() == name && c.tag_name().namespace() == Some(NS_MEDIA)
    };
    let mut contents: Vec<Node> = item.children().filter(|c| is_media(c, "content")).collect();
    for group in item.children().filter(|c| is_media(c, "group")) {
        contents.extend(group.children().filter(|c| is_media(c, "content")));
    }
    for node in contents {
        let Some(url) = attr(node, "url") else {
            continue;
        };
        options.push(MediaOption {
            url,
            mime: attr(node, "type"),
            length: number(node, "fileSize")
                .map(|v| v as i64)
                .filter(|v| *v > 0),
            bitrate: number(node, "bitrate")
                .filter(|v| *v > 0.0)
                .map(|kbps| kbps * 1000.0),
        });
    }
    options
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

fn ns_attr(node: Node, namespace: &str, name: &str) -> Option<String> {
    node.attribute((namespace, name))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Value of an attribute inside the text of one HTML start tag, with `&amp;` decoded.
fn html_attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let start = from + found;
        from = start + name.len();
        let boundary = lower[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_whitespace() || c == '"' || c == '\'');
        let rest = tag[from..].trim_start();
        if !boundary || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let value = match rest.chars().next()? {
            quote @ ('"' | '\'') => rest[1..].split(quote).next()?,
            _ => rest
                .split(|c: char| c.is_ascii_whitespace() || c == '>')
                .next()?,
        };
        return Some(value.trim().replace("&amp;", "&"));
    }
    None
}

pub fn looks_like_html(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_ascii_lowercase();
    head.contains("<!doctype html") || head.contains("<html")
}

/// Finds the feeds a web page announces with `<link rel="alternate">`, in the order of the
/// page, RSS before Atom. Empty for anything that is not an HTML page. Relative addresses are
/// resolved against `base`.
///
/// Pages often announce several feeds (blog posts, comments, one per audio format), so the
/// caller has to look for the one that carries episodes.
pub fn discover(bytes: &[u8], base: &url::Url) -> Vec<url::Url> {
    if !looks_like_html(bytes) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    let (mut rss, mut atom) = (Vec::new(), Vec::new());
    let mut from = 0;
    while let Some(found) = lower[from..].find("<link") {
        let start = from + found;
        let Some(length) = lower[start..].find('>') else {
            break;
        };
        let end = start + length;
        from = end;
        let tag = &text[start + "<link".len()..end];
        let is_alternate = html_attribute(tag, "rel").is_some_and(|rel| {
            rel.split_ascii_whitespace()
                .any(|r| r.eq_ignore_ascii_case("alternate"))
        });
        let Some(kind) = html_attribute(tag, "type").filter(|_| is_alternate) else {
            continue;
        };
        let Some(target) = html_attribute(tag, "href").and_then(|href| base.join(&href).ok())
        else {
            continue;
        };
        if !matches!(target.scheme(), "http" | "https") {
            continue;
        }
        let list = match kind.to_ascii_lowercase().as_str() {
            "application/rss+xml" => &mut rss,
            "application/atom+xml" => &mut atom,
            _ => continue,
        };
        if !list.contains(&target) {
            list.push(target);
        }
    }
    rss.append(&mut atom);
    rss
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

/// How much text the entities a document declares itself may add when they are expanded.
const MAX_ENTITY_EXPANSION: usize = 16 * 1024 * 1024;

/// Refuses documents whose own entity declarations would blow them up when parsed.
///
/// A document may declare entities (`<!ENTITY a "…">`) and refer to them as often as it likes.
/// The parser stops entities that refer to each other in depth, but not one long entity that is
/// referred to many thousand times: a few hundred kilobytes then expand to gigabytes. The
/// estimate here is deliberately coarse and errs on the side of refusing: every reference is
/// counted as if it expanded to the whole declaration block.
pub fn check_entities(text: &str) -> Result<()> {
    const DECLARATION: &str = "<!ENTITY";
    let (Some(first), Some(last)) = (text.find(DECLARATION), text.rfind(DECLARATION)) else {
        return Ok(());
    };
    // The block of declarations ends with the value of the last one: the text between the
    // first quotation mark after it and the next mark of the same kind.
    let after = last + DECLARATION.len();
    let end = text[after..]
        .char_indices()
        .find(|(_, c)| matches!(c, '"' | '\''))
        .and_then(|(open, quote)| {
            let value = after + open + 1;
            text[value..].find(quote).map(|close| value + close)
        })
        .unwrap_or(text.len());
    let declarations = &text[first..end];
    if own_references(declarations) > 0 {
        bail!("the document declares entities that refer to other entities");
    }
    let references = own_references(&text[end..]);
    if references.saturating_mul(declarations.len()) > MAX_ENTITY_EXPANSION {
        bail!("the document expands its own entities too often");
    }
    Ok(())
}

/// Counts references to entities other than the five every XML document knows and character
/// references such as `&#160;`.
fn own_references(text: &str) -> usize {
    text.match_indices('&')
        .filter(|(at, _)| {
            let rest = &text[at + 1..];
            !(rest.starts_with('#')
                || ["amp;", "lt;", "gt;", "quot;", "apos;"]
                    .iter()
                    .any(|known| rest.starts_with(known)))
        })
        .count()
}

pub fn parse(bytes: &[u8]) -> Result<ParsedFeed> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    check_entities(text)?;
    let doc = Document::parse_with_options(text, options).context("the feed is not valid XML")?;
    let root = doc.root_element();
    let namespace = root.tag_name().namespace();
    let mut feed = match root.tag_name().name() {
        "rss" => {
            let channel = child(root, None, "channel").context("the feed has no <channel>")?;
            parse_rss(channel, channel, None, text)
        }
        "RDF" if ns_matches(namespace, Some(NS_RDF)) => {
            // RSS 1.0: the items are siblings of the channel, not its children.
            let channel =
                child(root, Some(NS_RSS1), "channel").context("the feed has no <channel>")?;
            parse_rss(channel, root, Some(NS_RSS1), text)
        }
        "feed" if ns_matches(namespace, Some(NS_ATOM)) => parse_atom(root, text),
        "html" => bail!("this URL points to a web page, not to a podcast feed"),
        other => bail!("not a podcast feed (root element <{other}>)"),
    };
    feed.namespaces = root
        .namespaces()
        .filter(|ns| ns.name() != Some("xml"))
        .map(|ns| (ns.name().unwrap_or("").to_string(), ns.uri().to_string()))
        .collect();
    Ok(feed)
}

/// RSS 2.0 and RSS 1.0. `core` is the namespace of the format's own elements: none for RSS 2.0,
/// [`NS_RSS1`] for RSS 1.0. `items_in` is the element whose children are the items.
fn parse_rss(channel: Node, items_in: Node, core: Option<&str>, source: &str) -> ParsedFeed {
    let image_url = child(channel, Some(NS_ITUNES), "image")
        .and_then(|n| attr(n, "href"))
        .or_else(|| child(channel, core, "image").and_then(|n| child_text(n, core, "url")))
        .or_else(|| child(items_in, core, "image").and_then(|n| child_text(n, core, "url")));

    ParsedFeed {
        title: child_text(channel, core, "title").unwrap_or_else(|| "Untitled podcast".into()),
        author: child_text(channel, Some(NS_ITUNES), "author")
            .or_else(|| child_text(channel, Some(NS_DC), "creator")),
        description: child_text(channel, core, "description")
            .or_else(|| child_text(channel, Some(NS_ITUNES), "summary")),
        link: child_text(channel, core, "link"),
        image_url,
        language: child_text(channel, core, "language")
            .or_else(|| child_text(channel, Some(NS_DC), "language")),
        podcast_guid: child_text(channel, Some(NS_PODCAST), "guid"),
        new_feed_url: child_text(channel, Some(NS_ITUNES), "new-feed-url"),
        next_page: next_page(channel),
        namespaces: Vec::new(),
        items: items_in
            .children()
            .filter(|c| {
                c.is_element()
                    && c.tag_name().name() == "item"
                    && ns_matches(c.tag_name().namespace(), core)
            })
            .map(|item| parse_item(item, core, source))
            .collect(),
    }
}

fn parse_item(item: Node, core: Option<&str>, source: &str) -> ParsedItem {
    // RSS 1.0 has no enclosure of its own; feeds use the RSS 2.0 element or mod_enclosure.
    let enclosure = child(item, core, "enclosure").or_else(|| child(item, None, "enclosure"));
    let rdf_enclosure = child(item, Some(NS_ENC), "enclosure");
    let enclosure_url = enclosure
        .and_then(|n| attr(n, "url"))
        .or_else(|| rdf_enclosure.and_then(|n| ns_attr(n, NS_RDF, "resource")));
    let link = child_text(item, core, "link");
    let title = child_text(item, core, "title")
        .or_else(|| child_text(item, Some(NS_ITUNES), "title"))
        .unwrap_or_else(|| "Untitled episode".into());
    let published_raw =
        child_text(item, core, "pubDate").or_else(|| child_text(item, Some(NS_DC), "date"));

    // Feeds without <guid> exist. Fall back to something that stays stable between fetches.
    let guid = child_text(item, core, "guid")
        .or_else(|| ns_attr(item, NS_RDF, "about"))
        .or_else(|| {
            enclosure_url
                .as_deref()
                .map(|u| without_query(u).to_string())
        })
        .or_else(|| link.clone())
        .unwrap_or_else(|| format!("{title}|{}", published_raw.as_deref().unwrap_or("")));

    ParsedItem {
        guid,
        title,
        description: child_text(item, Some(NS_CONTENT), "encoded")
            .or_else(|| child_text(item, core, "description"))
            .or_else(|| child_text(item, Some(NS_ITUNES), "summary")),
        link,
        published_at: published_raw.as_deref().and_then(parse_date),
        enclosure_type: enclosure
            .and_then(|n| attr(n, "type"))
            .or_else(|| rdf_enclosure.and_then(|n| ns_attr(n, NS_ENC, "type"))),
        enclosure_length: enclosure
            .and_then(|n| attr(n, "length"))
            .or_else(|| rdf_enclosure.and_then(|n| ns_attr(n, NS_ENC, "length")))
            .and_then(|v| v.parse().ok())
            .filter(|v| *v > 0),
        enclosure_url,
        ..common_fields(item, source)
    }
}

/// The fields that are read the same way in every format: the iTunes and Podcasting 2.0 tags,
/// the hash and the raw XML.
fn common_fields(item: Node, source: &str) -> ParsedItem {
    ParsedItem {
        duration: child_text(item, Some(NS_ITUNES), "duration")
            .as_deref()
            .and_then(parse_duration),
        image_url: child(item, Some(NS_ITUNES), "image").and_then(|n| attr(n, "href")),
        season: child_text(item, Some(NS_ITUNES), "season").and_then(|v| v.parse().ok()),
        episode_no: child_text(item, Some(NS_ITUNES), "episode").and_then(|v| v.parse().ok()),
        links: item_links(item),
        alternatives: alternatives(item),
        hash: content_hash(item),
        raw_xml: source[item.range()].to_string(),
        ..ParsedItem::default()
    }
}

/// Atom links of a feed or entry with the given relation. A link without `rel` is an
/// `alternate` link.
fn atom_links<'a, 'i: 'a>(node: Node<'a, 'i>, rel: &'a str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(move |c| {
        c.is_element()
            && c.tag_name().name() == "link"
            && ns_matches(c.tag_name().namespace(), Some(NS_ATOM))
            && c.attribute("rel")
                .unwrap_or("alternate")
                .trim()
                .eq_ignore_ascii_case(rel)
    })
}

/// Text of an Atom text construct. For `type="xhtml"` the markup inside is kept as written.
fn atom_text(node: Node, source: &str) -> Option<String> {
    if node.attribute("type") == Some("xhtml")
        && let Some(div) = node.children().find(|c| c.is_element())
        && let (Some(first), Some(last)) = (div.first_child(), div.last_child())
    {
        let inner = source[first.range().start..last.range().end].trim();
        return (!inner.is_empty()).then(|| inner.to_string());
    }
    text_of(node)
}

fn parse_atom(root: Node, source: &str) -> ParsedFeed {
    let atom = Some(NS_ATOM);
    ParsedFeed {
        title: child_text(root, atom, "title").unwrap_or_else(|| "Untitled podcast".into()),
        author: child_text(root, Some(NS_ITUNES), "author")
            .or_else(|| child(root, atom, "author").and_then(|a| child_text(a, atom, "name"))),
        description: child(root, atom, "subtitle")
            .and_then(|n| atom_text(n, source))
            .or_else(|| child_text(root, Some(NS_ITUNES), "summary")),
        link: atom_links(root, "alternate")
            .next()
            .and_then(|n| attr(n, "href")),
        image_url: child(root, Some(NS_ITUNES), "image")
            .and_then(|n| attr(n, "href"))
            .or_else(|| child_text(root, atom, "logo"))
            .or_else(|| child_text(root, atom, "icon")),
        language: ns_attr(root, NS_XML, "lang"),
        podcast_guid: child_text(root, Some(NS_PODCAST), "guid"),
        new_feed_url: child_text(root, Some(NS_ITUNES), "new-feed-url"),
        next_page: next_page(root),
        namespaces: Vec::new(),
        items: root
            .children()
            .filter(|c| {
                c.is_element()
                    && c.tag_name().name() == "entry"
                    && ns_matches(c.tag_name().namespace(), atom)
            })
            .map(|entry| parse_atom_entry(entry, source))
            .collect(),
    }
}

fn parse_atom_entry(entry: Node, source: &str) -> ParsedItem {
    let atom = Some(NS_ATOM);
    let enclosure = atom_links(entry, "enclosure").next();
    let enclosure_url = enclosure.and_then(|n| attr(n, "href"));
    let link = atom_links(entry, "alternate")
        .next()
        .and_then(|n| attr(n, "href"));
    let title = child(entry, atom, "title")
        .and_then(text_of)
        .unwrap_or_else(|| "Untitled episode".into());
    let published_raw =
        child_text(entry, atom, "published").or_else(|| child_text(entry, atom, "updated"));
    let guid = child_text(entry, atom, "id")
        .or_else(|| {
            enclosure_url
                .as_deref()
                .map(|u| without_query(u).to_string())
        })
        .or_else(|| link.clone())
        .unwrap_or_else(|| format!("{title}|{}", published_raw.as_deref().unwrap_or("")));

    let mut item = ParsedItem {
        guid,
        title,
        description: child(entry, atom, "content")
            .and_then(|n| atom_text(n, source))
            .or_else(|| child(entry, atom, "summary").and_then(|n| atom_text(n, source)))
            .or_else(|| child_text(entry, Some(NS_ITUNES), "summary")),
        link,
        published_at: published_raw.as_deref().and_then(parse_date),
        enclosure_type: enclosure.and_then(|n| attr(n, "type")),
        enclosure_length: enclosure
            .and_then(|n| attr(n, "length"))
            .and_then(|v| v.parse().ok())
            .filter(|v| *v > 0),
        enclosure_url,
        ..common_fields(entry, source)
    };
    item.alternatives
        .extend(atom_links(entry, "enclosure").skip(1).filter_map(|n| {
            Some(MediaOption {
                url: attr(n, "href")?,
                mime: attr(n, "type"),
                length: attr(n, "length")
                    .and_then(|v| v.parse().ok())
                    .filter(|v| *v > 0),
                bitrate: None,
            })
        }));
    item
}

#[cfg(test)]
mod tests {
    #[test]
    fn entities_that_would_blow_up_are_refused() {
        let feed = |declarations: &str, title: &str| {
            format!(
                "<?xml version=\"1.0\"?><!DOCTYPE rss [{declarations}]>\
                 <rss><channel><title>{title}</title></channel></rss>"
            )
        };
        // Ordinary use: a few named characters.
        let fine = feed(
            "<!ENTITY nbsp \"&#160;\"><!ENTITY auml \"&#228;\">",
            "M&auml;rchen&nbsp;&amp; mehr",
        );
        assert_eq!(
            super::parse(fine.as_bytes()).unwrap().title,
            "Märchen\u{a0}& mehr"
        );

        // One long entity, referred to thousands of times: 300 KB that would become 300 MB.
        let long = format!("<!ENTITY a \"{}\">", "x".repeat(100_000));
        let many = feed(&long, &"&a;".repeat(3_000));
        let error = super::parse(many.as_bytes()).unwrap_err();
        assert!(format!("{error:#}").contains("too often"), "{error:#}");

        // Entities built from entities.
        let nested = feed("<!ENTITY a \"ha\"><!ENTITY b \"&a;&a;\">", "&b;");
        assert!(super::parse(nested.as_bytes()).is_err());

        // A second declaration block hidden behind text that looks like the end of the first.
        let hidden = feed(
            &format!("<!ENTITY z \"]>\"><!ENTITY a \"{}\">", "x".repeat(100_000)),
            &"&a;".repeat(3_000),
        );
        assert!(super::parse(hidden.as_bytes()).is_err());

        // Without declarations nothing is counted, however many references there are.
        let plain = format!(
            "<rss><channel><title>{}</title></channel></rss>",
            "&amp;".repeat(50_000)
        );
        assert!(super::parse(plain.as_bytes()).is_ok());
    }

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

    /// One RSS item with the given extra elements, parsed.
    fn item_with(extra: &str) -> super::ParsedItem {
        let xml = format!(
            r#"<rss version="2.0" xmlns:podcast="https://podcastindex.org/namespace/1.0"
                 xmlns:media="http://search.yahoo.com/mrss/"><channel><title>T</title>
               <item><title>E</title><guid>g</guid>
                 <enclosure url="https://cdn.example/e-64.mp3" type="audio/mpeg" length="1000"/>
                 {extra}</item></channel></rss>"#
        );
        super::parse(xml.as_bytes()).unwrap().items.remove(0)
    }

    #[test]
    fn the_best_file_of_an_episode_is_chosen() {
        // Podcasting 2.0: the enclosure is listed as the default with its bitrate.
        let item = item_with(
            r#"<podcast:alternateEnclosure type="audio/mpeg" bitrate="64000" default="true">
                 <podcast:source uri="https://cdn.example/e-64.mp3"/></podcast:alternateEnclosure>
               <podcast:alternateEnclosure type="audio/mpeg" bitrate="320000" length="5000">
                 <podcast:source uri="ipfs://Qm"/>
                 <podcast:source uri="https://cdn.example/e-320.mp3"/></podcast:alternateEnclosure>
               <podcast:alternateEnclosure type="audio/opus" bitrate="128000">
                 <podcast:source uri="https://cdn.example/e-128.opus"/></podcast:alternateEnclosure>"#,
        );
        let best = item.best_media().unwrap();
        assert_eq!(best.url, "https://cdn.example/e-320.mp3");
        assert_eq!(best.mime.as_deref(), Some("audio/mpeg"));
        assert_eq!(best.bitrate, Some(320_000.0));

        // Media RSS states the bitrate in kilobits per second.
        let item = item_with(
            r#"<media:group>
                 <media:content url="https://cdn.example/e-64.mp3" type="audio/mpeg" bitrate="64"/>
                 <media:content url="https://cdn.example/e-192.mp3" type="audio/mpeg" bitrate="192"/>
               </media:group>"#,
        );
        assert_eq!(
            item.best_media().unwrap().url,
            "https://cdn.example/e-192.mp3"
        );

        // Audio stays audio, however large a video is.
        let item = item_with(
            r#"<media:content url="https://cdn.example/e.mp4" type="video/mp4" fileSize="99999"/>"#,
        );
        assert_eq!(item.best_media(), None);

        // Without a way to compare, the enclosure stays: no bitrate on either side, no size on
        // the alternative; and a smaller file is not better.
        let item =
            item_with(r#"<media:content url="https://cdn.example/other.mp3" type="audio/mpeg"/>"#);
        assert_eq!(item.best_media(), None);
        let item = item_with(
            r#"<media:content url="https://cdn.example/small.mp3" type="audio/mpeg" fileSize="10"/>"#,
        );
        assert_eq!(item.best_media(), None);
        assert_eq!(item_with("").best_media(), None);
        // A playlist is not the episode, however its type starts.
        let item = item_with(
            r#"<media:content url="https://cdn.example/live.m3u8" type="audio/mpegurl" fileSize="99999"/>"#,
        );
        assert_eq!(item.best_media(), None);
    }

    #[test]
    fn further_atom_enclosures_are_alternatives() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>T</title>
            <entry><title>E</title><id>e</id>
              <link rel="enclosure" href="https://cdn.example/low.m4a" type="audio/mp4" length="100"/>
              <link rel="enclosure" href="https://cdn.example/high.m4a" type="audio/mp4" length="900"/>
            </entry></feed>"#;
        let item = super::parse(xml.as_bytes()).unwrap().items.remove(0);
        assert_eq!(
            item.enclosure_url.as_deref(),
            Some("https://cdn.example/low.m4a")
        );
        assert_eq!(
            item.best_media().unwrap().url,
            "https://cdn.example/high.m4a"
        );
    }

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
    fn hash_covers_the_whole_item_but_not_query_strings() {
        let hash = |text: &str| parse(text.as_bytes()).unwrap().items[0].meta_hash();
        let original = hash(FEED);
        assert_eq!(
            original,
            hash(&FEED.replace("ep2.mp3?token=abc", "ep2.mp3?token=other"))
        );
        assert_ne!(original, hash(&FEED.replace("Episode 2", "Episode 2!")));
        // A tag Uguisu does not index still makes a new revision.
        assert_ne!(
            original,
            hash(&FEED.replace(
                "<itunes:season>1</itunes:season>",
                "<itunes:season>1</itunes:season><podcast:person>Someone</podcast:person>"
            ))
        );
    }

    #[test]
    fn finds_linked_files() {
        let feed = FEED.replace(
            "<itunes:season>1</itunes:season>",
            r#"<podcast:transcript url="https://cdn.example.org/2.vtt?t=1" type="text/vtt"/>
               <podcast:transcript url="https://cdn.example.org/2.vtt?t=2" type="text/vtt"/>
               <podcast:transcript url="https://cdn.example.org/2.srt" type="application/srt"/>
               <podcast:chapters url="https://cdn.example.org/2.json" type="application/json+chapters"/>
               <itunes:image href="https://cdn.example.org/2.jpg"/>
               <podcast:transcript url="/relative.vtt"/>"#,
        );
        let item = &parse(feed.as_bytes()).unwrap().items[0];
        let found: Vec<(&str, &str)> = item
            .links
            .iter()
            .map(|l| (l.kind, l.url.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("transcript", "https://cdn.example.org/2.vtt?t=1"),
                ("transcript", "https://cdn.example.org/2.srt"),
                ("chapters", "https://cdn.example.org/2.json"),
                ("image", "https://cdn.example.org/2.jpg"),
            ]
        );
    }

    #[test]
    fn finds_the_next_page() {
        let page = |links: &str| {
            let xml = format!(
                r#"<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom"><channel>
<title>Paged</title><link>https://example.org</link>{links}</channel></rss>"#
            );
            parse(xml.as_bytes()).unwrap().next_page
        };
        assert_eq!(page(r#"<atom:link rel="self" href="/feed"/>"#), None);
        assert_eq!(
            page(
                r#"<atom:link rel="self" href="/feed"/><atom:link rel="next" href="/feed?page=2"/>"#
            ),
            Some("/feed?page=2".into())
        );
        assert_eq!(
            page(r#"<atom:link rel="prev-archive" href="https://example.org/2023.xml"/>"#),
            Some("https://example.org/2023.xml".into())
        );
    }

    #[test]
    fn parses_atom() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xml:lang="de"
      xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <title>Atom Cast</title>
  <subtitle>Spoken &amp; sung</subtitle>
  <link rel="self" href="https://example.org/atom.xml"/>
  <link href="https://example.org/"/>
  <link rel="next" href="https://example.org/atom.xml?page=2"/>
  <author><name>A. Warbler</name></author>
  <logo>https://example.org/logo.png</logo>
  <entry>
    <id>urn:uuid:1</id>
    <title type="html">One &amp;amp; only</title>
    <published>2024-01-02T10:00:00Z</published>
    <updated>2024-02-01T00:00:00Z</updated>
    <link rel="alternate" href="https://example.org/1"/>
    <link rel="enclosure" href="https://cdn.example.org/1.mp3?t=1" type="audio/mpeg" length="42"/>
    <content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><p>Hello <b>you</b></p></div></content>
    <itunes:duration>61</itunes:duration>
  </entry>
  <entry>
    <title>No id</title>
    <updated>2024-01-01T10:00:00+01:00</updated>
    <summary>Short</summary>
    <link rel="enclosure" href="https://cdn.example.org/0.mp3"/>
  </entry>
</feed>"#;
        let feed = parse(xml.as_bytes()).unwrap();
        assert_eq!(feed.title, "Atom Cast");
        assert_eq!(feed.description.as_deref(), Some("Spoken & sung"));
        assert_eq!(feed.link.as_deref(), Some("https://example.org/"));
        assert_eq!(feed.author.as_deref(), Some("A. Warbler"));
        assert_eq!(feed.language.as_deref(), Some("de"));
        assert_eq!(
            feed.image_url.as_deref(),
            Some("https://example.org/logo.png")
        );
        assert_eq!(
            feed.next_page.as_deref(),
            Some("https://example.org/atom.xml?page=2")
        );
        assert_eq!(feed.items.len(), 2);
        let first = &feed.items[0];
        assert_eq!(first.guid, "urn:uuid:1");
        assert_eq!(first.title, "One &amp; only");
        assert_eq!(first.published_at, Some(1_704_189_600));
        assert_eq!(first.link.as_deref(), Some("https://example.org/1"));
        assert_eq!(
            first.enclosure_url.as_deref(),
            Some("https://cdn.example.org/1.mp3?t=1")
        );
        assert_eq!(first.enclosure_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(first.enclosure_length, Some(42));
        assert_eq!(
            first.description.as_deref(),
            Some("<p>Hello <b>you</b></p>")
        );
        assert_eq!(first.duration, Some(61));
        assert!(first.raw_xml.starts_with("<entry>"));
        let second = &feed.items[1];
        assert_eq!(second.guid, "https://cdn.example.org/0.mp3");
        assert_eq!(second.published_at, Some(1_704_099_600));
        assert_eq!(second.description.as_deref(), Some("Short"));
    }

    #[test]
    fn parses_rss_1() {
        let xml = r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns="http://purl.org/rss/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/"
         xmlns:enc="http://purl.oclc.org/net/rss_2.0/enc#">
  <channel rdf:about="https://example.org/rss">
    <title>Old Cast</title>
    <link>https://example.org</link>
    <description>From 2003</description>
    <dc:language>en</dc:language>
  </channel>
  <image><url>https://example.org/old.png</url></image>
  <item rdf:about="https://example.org/ep/1">
    <title>First</title>
    <link>https://example.org/ep/1</link>
    <description>Notes</description>
    <dc:date>2003-05-01T12:00:00Z</dc:date>
    <enc:enclosure rdf:resource="https://example.org/1.mp3" enc:type="audio/mpeg" enc:length="7"/>
  </item>
</rdf:RDF>"#;
        let feed = parse(xml.as_bytes()).unwrap();
        assert_eq!(feed.title, "Old Cast");
        assert_eq!(feed.language.as_deref(), Some("en"));
        assert_eq!(
            feed.image_url.as_deref(),
            Some("https://example.org/old.png")
        );
        assert_eq!(feed.items.len(), 1);
        let item = &feed.items[0];
        assert_eq!(item.guid, "https://example.org/ep/1");
        assert_eq!(item.title, "First");
        assert_eq!(item.published_at, Some(1_051_790_400));
        assert_eq!(
            item.enclosure_url.as_deref(),
            Some("https://example.org/1.mp3")
        );
        assert_eq!(item.enclosure_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(item.enclosure_length, Some(7));
    }

    #[test]
    fn discovers_the_feeds_of_a_web_page() {
        let base = url::Url::parse("https://example.org/show/").unwrap();
        let find = |html: &str| -> Vec<String> {
            discover(html.as_bytes(), &base)
                .into_iter()
                .map(String::from)
                .collect()
        };
        let page = r#"<!DOCTYPE html><html><head>
<link rel="stylesheet" href="/style.css">
<LINK REL='alternate' TYPE='application/atom+xml' HREF='/atom.xml'>
<link href="feed.xml?a=1&amp;b=2" type="application/rss+xml" title="Podcast" rel="alternate">
<link rel=alternate type=application/rss+xml href=/comments>
</head><body><p>broken <br> html</body></html>"#;
        assert_eq!(
            find(page),
            [
                "https://example.org/show/feed.xml?a=1&b=2",
                "https://example.org/comments",
                "https://example.org/atom.xml"
            ]
        );
        assert!(find("<html><head><title>No feed</title></head></html>").is_empty());
        // A feed is never mistaken for a page, whatever links it contains.
        assert!(
            find(r#"<rss><channel><link rel="alternate" type="application/rss+xml" href="/x"/></channel></rss>"#)
                .is_empty()
        );
    }

    #[test]
    fn finds_images_in_show_notes() {
        let xml = r#"<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel><title>T</title>
<item><guid>1</guid>
<description>&lt;p&gt;&lt;img src="https://example.org/a.png?w=1&amp;amp;h=2"&gt;&lt;/p&gt;</description>
<content:encoded><![CDATA[<p><IMG alt="x" data-src="https://example.org/lazy.png"
 SRC='https://example.org/a.png?w=9'> <img srcset="https://example.org/set.png 2x"
 src=https://example.org/b.jpg> <img src="/relative.png"><img src="data:image/gif;base64,AA"></p>]]></content:encoded>
</item></channel></rss>"#;
        let feed = parse(xml.as_bytes()).unwrap();
        let links: Vec<(&str, &str)> = feed.items[0]
            .links
            .iter()
            .map(|l| (l.kind, l.url.as_str()))
            .collect();
        assert_eq!(
            links,
            [
                // Found first in <description>; the same picture in another size is not
                // stored twice.
                ("notes-image", "https://example.org/a.png?w=1&h=2"),
                ("notes-image", "https://example.org/b.jpg"),
            ]
        );
    }

    #[test]
    fn rejects_non_feeds() {
        assert!(parse(b"<html><body>hi</body></html>").is_err());
        assert!(parse(b"<opml version=\"2.0\"><body/></opml>").is_err());
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
