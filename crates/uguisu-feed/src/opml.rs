//! OPML subscription lists (ADR 0049).
//!
//! [`parse()`] reads every outline that carries an `xmlUrl`, at any depth,
//! with the feed parser's hardening: no DTD, no custom entities, and caps
//! on size, depth and count. [`write()`] produces a flat OPML 2.0 list.

use std::borrow::Cow;
use std::fmt::Write as _;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::parser::looks_like_html;
use crate::text::resolve_reference;

/// Largest document read, in bytes.
pub const MAX_BYTES: usize = 1 << 20;
/// Deepest element nesting read.
pub const MAX_DEPTH: usize = 64;
/// Most feed outlines one document may carry.
pub const MAX_OUTLINES: usize = 10_000;

/// One feed outline, as written in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outline {
    /// `title`, else `text`, with whitespace collapsed.
    pub title: Option<String>,
    /// `xmlUrl`, unchecked: callers decide whether it is a usable URL.
    pub xml_url: String,
    /// `htmlUrl`.
    pub html_url: Option<String>,
}

/// Why a document was not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpmlError {
    /// Nothing but whitespace, or no element at all.
    #[error("the document is empty")]
    Empty,
    /// Larger than [`MAX_BYTES`].
    #[error("the document is {size} bytes, more than the {limit} allowed")]
    TooLarge {
        /// Size of the document.
        size: usize,
        /// The cap.
        limit: usize,
    },
    /// Not XML at all.
    #[error("{}", if *looks_like_html { "this is an HTML page, not an OPML file" } else { "this is not XML" })]
    NotXml {
        /// Whether it looks like an HTML page.
        looks_like_html: bool,
    },
    /// XML whose root is not `<opml>`.
    #[error("this is XML with a <{root}> root, not an OPML file")]
    NotOpml {
        /// The root element's local name.
        root: String,
    },
    /// XML the reader could not get through.
    #[error("the OPML is malformed: {0}")]
    Malformed(String),
    /// Nested deeper than [`MAX_DEPTH`].
    #[error("the OPML nests deeper than {0} levels")]
    TooDeep(usize),
    /// More than [`MAX_OUTLINES`] feeds.
    #[error("the OPML lists more than {0} feeds")]
    TooMany(usize),
}

/// Reads every feed outline of an OPML document, folders flattened.
///
/// A declared encoding is ignored: `text` is already decoded.
pub fn parse(text: &str) -> Result<Vec<Outline>, OpmlError> {
    if text.len() > MAX_BYTES {
        return Err(OpmlError::TooLarge {
            size: text.len(),
            limit: MAX_BYTES,
        });
    }
    let text = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    if text.trim_end().is_empty() {
        return Err(OpmlError::Empty);
    }
    if !text.starts_with('<') || looks_like_html(text) {
        return Err(OpmlError::NotXml {
            looks_like_html: looks_like_html(text),
        });
    }

    let mut reader = Reader::from_str(text);
    let cfg = reader.config_mut();
    cfg.check_end_names = false;
    cfg.expand_empty_elements = false;

    let mut outlines = Vec::new();
    let mut depth = 0_usize;
    let mut root_seen = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| OpmlError::Malformed(e.to_string()))?;
        let (element, opens) = match event {
            Event::Start(e) => (e, true),
            Event::Empty(e) => (e, false),
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let local = element.local_name().as_ref().to_ascii_lowercase();
        if !root_seen {
            if local != "opml" {
                return Err(OpmlError::NotOpml { root: local });
            }
            root_seen = true;
        }
        if opens {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(OpmlError::TooDeep(MAX_DEPTH));
            }
        }
        if local == "outline"
            && let Some(outline) = outline(&element)
        {
            if outlines.len() == MAX_OUTLINES {
                return Err(OpmlError::TooMany(MAX_OUTLINES));
            }
            outlines.push(outline);
        }
    }
    if !root_seen {
        return Err(OpmlError::Empty);
    }
    Ok(outlines)
}

fn outline(element: &BytesStart<'_>) -> Option<Outline> {
    let (mut xml_url, mut html_url, mut title, mut text) = (None, None, None, None);
    for attribute in element.attributes().with_checks(false).flatten() {
        let key = attribute.key.local_name().as_ref().to_ascii_lowercase();
        // Predefined and the feed parser's HTML entities only. A value that
        // still does not decode (exporters write a bare `&` in URLs) is kept
        // as written: nothing is added from it without being fetched first.
        let value = attribute
            .normalized_value_with(quick_xml::XmlVersion::Implicit1_0, 1, |name| {
                match resolve_reference(name) {
                    Some(Cow::Borrowed(s)) => Some(s),
                    _ => None,
                }
            })
            .map_or_else(|_| attribute.value.clone().into_owned(), Cow::into_owned);
        match key.as_str() {
            "xmlurl" => xml_url = Some(value),
            "htmlurl" => html_url = Some(value),
            "title" => title = Some(value),
            "text" => text = Some(value),
            _ => {}
        }
    }
    let xml_url = xml_url
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())?;
    Some(Outline {
        title: title
            .and_then(|t| collapse(&t))
            .or_else(|| text.and_then(|t| collapse(&t))),
        xml_url,
        html_url: html_url
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty()),
    })
}

fn collapse(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

/// A flat OPML 2.0 document listing `outlines` in the order given.
///
/// Characters XML 1.0 cannot carry are left out; nothing else is changed.
#[must_use]
pub fn write(title: &str, outlines: &[Outline]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head>\n",
    );
    let _ = writeln!(out, "    <title>{}</title>", attr(title));
    out.push_str("  </head>\n  <body>\n");
    for o in outlines {
        let text = o.title.as_deref().unwrap_or(&o.xml_url);
        let _ = write!(out, "    <outline type=\"rss\" text=\"{}\"", attr(text));
        if let Some(title) = &o.title {
            let _ = write!(out, " title=\"{}\"", attr(title));
        }
        let _ = write!(out, " xmlUrl=\"{}\"", attr(&o.xml_url));
        if let Some(html) = &o.html_url {
            let _ = write!(out, " htmlUrl=\"{}\"", attr(html));
        }
        out.push_str("/>\n");
    }
    out.push_str("  </body>\n</opml>\n");
    out
}

fn attr(value: &str) -> String {
    let allowed: String = value
        .chars()
        .filter(|&c| {
            matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{fffe}' && c != '\u{ffff}')
        })
        .collect();
    quick_xml::escape::escape(allowed.as_str()).into_owned()
}
