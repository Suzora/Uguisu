//! Builds the feed Uguisu publishes for an archived podcast.
//!
//! Episodes are published as the original `<item>` elements, with only the enclosure pointing at
//! the archived file. GUIDs, show notes and everything a publisher adds through namespaces
//! (chapters, transcripts, people, ...) therefore reach the podcast app unchanged. If an item
//! cannot be reused, an equivalent one is generated from the indexed fields.

use std::fmt::Write;

use roxmltree::Document;

use crate::feed::without_query;
use crate::model::{Attachment, FeedEpisode, Podcast};
use crate::util::xml_escape;

fn rfc2822(timestamp: i64) -> Option<String> {
    crate::util::rfc2822(timestamp)
}

fn url_segment(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// Public URL of an archived media file.
pub fn media_url(base_url: &str, token: &str, episode_id: i64, file_name: &str) -> String {
    format!(
        "{}/media/{token}/{episode_id}/{}",
        base_url.trim_end_matches('/'),
        url_segment(file_name)
    )
}

pub fn feed_url(base_url: &str, token: &str) -> String {
    format!("{}/feed/{token}.xml", base_url.trim_end_matches('/'))
}

const NS_ITUNES: &str = "http://www.itunes.com/dtds/podcast-1.0.dtd";
const NS_CONTENT: &str = "http://purl.org/rss/1.0/modules/content/";

fn is_prefix(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.eq_ignore_ascii_case("xml")
        && !name.eq_ignore_ascii_case("xmlns")
}

/// Namespace declarations for the root element, and whether original items can be reused.
///
/// The source feed's prefixes are kept, because the original items use them. A source feed with
/// a default namespace is not reused: its unprefixed elements would change meaning here.
fn namespaces(podcast: &Podcast) -> (String, bool) {
    let stored: Vec<(String, String)> = podcast
        .namespaces
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let reusable =
        podcast.namespaces.is_some() && stored.iter().all(|(prefix, _)| !prefix.is_empty());
    let mut declared: Vec<(String, String)> = stored
        .into_iter()
        .filter(|(prefix, _)| is_prefix(prefix))
        .collect();
    for (prefix, uri) in [("itunes", NS_ITUNES), ("content", NS_CONTENT)] {
        if !declared.iter().any(|(p, _)| p == prefix) {
            declared.push((prefix.to_string(), uri.to_string()));
        }
    }
    let attributes = declared
        .iter()
        .map(|(prefix, uri)| format!("xmlns:{prefix}=\"{}\"", xml_escape(uri)))
        .collect::<Vec<_>>()
        .join(" ");
    (attributes, reusable)
}

/// Public URL of an archived attachment.
pub fn attachment_url(base_url: &str, token: &str, attachment: &Attachment) -> String {
    format!(
        "{}/media/{token}/{}/a/{}/{}",
        base_url.trim_end_matches('/'),
        attachment.episode_id,
        attachment.id,
        url_segment(attachment.file_name())
    )
}

/// What can follow an address inside HTML that is itself inside XML: a quote, a bracket or
/// white space, written directly or as an entity, escaped once or twice.
const ADDRESS_ENDS: [&str; 12] = [
    "\"",
    "'",
    "<",
    ">",
    "&quot;",
    "&#34;",
    "&#39;",
    "&apos;",
    "&lt;",
    "&gt;",
    "&amp;quot;",
    "&amp;#",
];

/// Replaces the addresses of archived files wherever they occur in `text`, which is how images
/// in show notes are pointed at the archive. An address is matched together with its query
/// string, and only as a whole: `a.png` is not replaced inside `a.png.webp`.
fn rewrite_text(text: &str, links: &[(&str, String)]) -> String {
    let mut out = text.to_string();
    for (source, archived) in links {
        if source.is_empty() || !out.contains(source) {
            continue;
        }
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(position) = rest.find(source) {
            let after = &rest[position + source.len()..];
            // The end of the address: directly after it, or after its query string.
            let end = after
                .char_indices()
                .find(|(index, c)| {
                    c.is_whitespace() || ADDRESS_ENDS.iter().any(|e| after[*index..].starts_with(e))
                })
                .map_or(after.len(), |(index, _)| index);
            let tail = &after[..end];
            result.push_str(&rest[..position]);
            if tail.is_empty() || tail.starts_with('?') || tail.starts_with('#') {
                result.push_str(archived);
            } else {
                result.push_str(source);
                result.push_str(tail);
            }
            rest = &after[end..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// Returns the original `<item>` with its `<enclosure>` replaced, or `None` if it cannot be
/// reused (not well-formed on its own, no enclosure, or more than one element).
///
/// `links` maps source addresses (without query string) of archived attachments to their
/// address in the archive; `url` and `href` attributes pointing at one of them are rewritten,
/// and so are the addresses of images in the show notes.
fn reuse_item(
    raw: &str,
    namespace_attributes: &str,
    enclosure: &str,
    links: &[(&str, String)],
) -> Option<String> {
    let open = format!("<r {namespace_attributes}>");
    let text = format!("{open}{raw}</r>");
    let doc = Document::parse(&text).ok()?;
    let mut elements = doc.root_element().children().filter(|n| n.is_element());
    let item = elements.next()?;
    if elements.next().is_some() || item.tag_name().name() != "item" {
        return None;
    }
    let original = item.children().find(|n| {
        n.is_element() && n.tag_name().name() == "enclosure" && n.tag_name().namespace().is_none()
    })?;

    // Replacements as (range in `text`, new content), applied from left to right.
    let mut edits: Vec<(std::ops::Range<usize>, String)> =
        vec![(original.range(), enclosure.to_string())];
    if !links.is_empty() {
        for node in item
            .descendants()
            .filter(|n| n.is_element() && *n != original)
        {
            for attribute in node.attributes() {
                if !matches!(attribute.name(), "url" | "href") {
                    continue;
                }
                let key = without_query(attribute.value());
                if let Some((_, archived)) = links.iter().find(|(source, _)| *source == key) {
                    edits.push((attribute.range_value(), xml_escape(archived)));
                }
            }
        }
        edits.sort_by_key(|(range, _)| range.start);
    }

    let mut out = String::with_capacity(raw.len() + 128);
    let mut position = open.len();
    for (range, replacement) in edits {
        if range.start < position {
            return None;
        }
        out.push_str(&text[position..range.start]);
        out.push_str(&replacement);
        position = range.end;
    }
    out.push_str(&text[position..text.len() - "</r>".len()]);
    Some(rewrite_text(&out, links))
}

/// Renders an RSS 2.0 document. Episodes without an archived file are left out, as are episodes
/// that disappeared from the source feed when the podcast is configured that way.
pub fn build_feed(
    podcast: &Podcast,
    episodes: &[FeedEpisode],
    attachments: &[Attachment],
    base_url: &str,
) -> String {
    let base = base_url.trim_end_matches('/');
    let token = &podcast.feed_token;
    let mut x = String::with_capacity(4096 + episodes.len() * 1024);
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let (namespace_attributes, reusable) = namespaces(podcast);
    let _ = writeln!(x, "<rss version=\"2.0\" {namespace_attributes}>\n<channel>");
    let _ = writeln!(x, "<title>{}</title>", xml_escape(&podcast.title));
    let source = crate::http::redact(&podcast.original_feed_url);
    let link = podcast.link.as_deref().unwrap_or(&source);
    let _ = writeln!(x, "<link>{}</link>", xml_escape(link));
    let _ = writeln!(
        x,
        "<description>{}</description>",
        xml_escape(podcast.description.as_deref().unwrap_or(""))
    );
    if let Some(language) = &podcast.language {
        let _ = writeln!(x, "<language>{}</language>", xml_escape(language));
    }
    if let Some(author) = &podcast.author {
        let _ = writeln!(x, "<itunes:author>{}</itunes:author>", xml_escape(author));
    }
    let image = match &podcast.cover_file {
        Some(file) => Some(format!("{base}/media/{token}/{}", url_segment(file))),
        None => podcast.image_url.clone(),
    };
    if let Some(image) = image {
        let _ = writeln!(x, "<itunes:image href=\"{}\"/>", xml_escape(&image));
        let _ = writeln!(
            x,
            "<image><url>{}</url><title>{}</title><link>{}</link></image>",
            xml_escape(&image),
            xml_escape(&podcast.title),
            xml_escape(link)
        );
    }
    // A private archive must not end up in public podcast directories.
    x.push_str("<itunes:block>Yes</itunes:block>\n");
    x.push_str("<generator>Uguisu</generator>\n");

    for episode in episodes {
        let view = &episode.view;
        let e = &view.episode;
        let (Some(file_name), Some(size), Some(mime)) = (
            view.media_file_name(),
            view.media_size,
            view.media_mime.as_deref(),
        ) else {
            continue;
        };
        if e.removed_at.is_some() && !podcast.include_removed {
            continue;
        }
        let enclosure = format!(
            "<enclosure url=\"{}\" length=\"{size}\" type=\"{}\"/>",
            xml_escape(&media_url(base, token, e.id, file_name)),
            xml_escape(mime)
        );
        let links: Vec<(&str, String)> = attachments
            .iter()
            .filter(|a| a.episode_id == e.id)
            .map(|a| (a.source_key.as_str(), attachment_url(base, token, a)))
            .collect();
        if reusable
            && let Some(raw) = &episode.raw_xml
            && let Some(item) = reuse_item(raw, &namespace_attributes, &enclosure, &links)
        {
            x.push_str(&item);
            x.push('\n');
            continue;
        }
        x.push_str("<item>\n");
        let _ = writeln!(x, "<title>{}</title>", xml_escape(&e.title));
        let _ = writeln!(
            x,
            "<guid isPermaLink=\"false\">{}</guid>",
            xml_escape(&e.guid)
        );
        if let Some(date) = e.published_at.and_then(rfc2822) {
            let _ = writeln!(x, "<pubDate>{date}</pubDate>");
        }
        if let Some(link) = &e.link {
            let _ = writeln!(x, "<link>{}</link>", xml_escape(link));
        }
        if let Some(description) = &e.description {
            let escaped = xml_escape(&rewrite_text(description, &links));
            let _ = writeln!(x, "<description>{escaped}</description>");
            let _ = writeln!(x, "<content:encoded>{escaped}</content:encoded>");
        }
        if let Some(duration) = e.duration {
            let _ = writeln!(x, "<itunes:duration>{duration}</itunes:duration>");
        }
        if let Some(season) = e.season {
            let _ = writeln!(x, "<itunes:season>{season}</itunes:season>");
        }
        if let Some(number) = e.episode_no {
            let _ = writeln!(x, "<itunes:episode>{number}</itunes:episode>");
        }
        x.push_str(&enclosure);
        x.push_str("\n</item>\n");
    }
    x.push_str("</channel>\n</rss>\n");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMESPACES: &str = "xmlns:podcast=\"https://podcastindex.org/namespace/1.0\"";
    const ENCLOSURE: &str =
        "<enclosure url=\"https://archive.example/a.mp3\" length=\"1\" type=\"audio/mpeg\"/>";

    #[test]
    fn rewrites_addresses_in_show_notes() {
        let links = [("https://cdn.example/a.png", "ARCHIVED".to_string())];
        let rewrite = |text: &str| rewrite_text(text, &links);
        // In CDATA, with and without a query string, and in every quoting style.
        assert_eq!(
            rewrite(
                r#"<img src="https://cdn.example/a.png"> <img src='https://cdn.example/a.png?w=1&amp;h=2'>"#
            ),
            r#"<img src="ARCHIVED"> <img src='ARCHIVED'>"#
        );
        assert_eq!(rewrite("src=https://cdn.example/a.png>"), "src=ARCHIVED>");
        // Escaped once more, as in a <description> without CDATA.
        assert_eq!(
            rewrite("&lt;img src=&quot;https://cdn.example/a.png?w=1&amp;amp;h=2&quot;&gt;"),
            "&lt;img src=&quot;ARCHIVED&quot;&gt;"
        );
        // Other addresses that merely start the same way stay as they are.
        let other = r#"<img src="https://cdn.example/a.png.webp"> https://cdn.example/a.pngx"#;
        assert_eq!(rewrite(other), other);
        assert_eq!(rewrite("see https://cdn.example/a.png"), "see ARCHIVED");
    }

    #[test]
    fn reuses_the_original_item() {
        let raw = "<item><title>A &amp; B</title><guid>1</guid>\
                   <enclosure url=\"https://cdn.example/a.mp3?x=1&amp;y=2\" type=\"audio/mpeg\"></enclosure>\
                   <podcast:transcript url=\"https://cdn.example/a.vtt\" type=\"text/vtt\"/>\
                   <description><![CDATA[<p>Notes</p>]]></description></item>";
        let item = reuse_item(raw, NAMESPACES, ENCLOSURE, &[]).unwrap();
        assert_eq!(
            item,
            format!(
                "<item><title>A &amp; B</title><guid>1</guid>{ENCLOSURE}\
                 <podcast:transcript url=\"https://cdn.example/a.vtt\" type=\"text/vtt\"/>\
                 <description><![CDATA[<p>Notes</p>]]></description></item>"
            )
        );
    }

    #[test]
    fn refuses_items_it_cannot_reuse() {
        // Prefix that is not declared, no enclosure, trailing content, broken markup.
        assert!(
            reuse_item(
                "<item><x:y/><enclosure url=\"u\"/></item>",
                NAMESPACES,
                ENCLOSURE,
                &[]
            )
            .is_none()
        );
        assert!(reuse_item("<item><title>t</title></item>", NAMESPACES, ENCLOSURE, &[]).is_none());
        assert!(
            reuse_item(
                "<item><enclosure url=\"u\"/></item><item><enclosure url=\"v\"/></item>",
                NAMESPACES,
                ENCLOSURE,
                &[]
            )
            .is_none()
        );
        assert!(
            reuse_item(
                "<item><enclosure url=\"u\"></item>",
                NAMESPACES,
                ENCLOSURE,
                &[]
            )
            .is_none()
        );
    }

    #[test]
    fn rewrites_links_to_archived_attachments() {
        let raw = "<item><guid>1</guid>\
                   <podcast:transcript url=\"https://cdn.example/a.vtt?t=9\" type=\"text/vtt\"/>\
                   <enclosure url=\"https://cdn.example/a.mp3\"/>\
                   <podcast:chapters type=\"application/json\" url='https://cdn.example/a.json'/>\
                   <podcast:transcript url=\"https://cdn.example/not-archived.srt\"/></item>";
        let links = [
            (
                "https://cdn.example/a.vtt",
                "https://archive.example/t?x=1&y=2".to_string(),
            ),
            (
                "https://cdn.example/a.json",
                "https://archive.example/c".to_string(),
            ),
        ];
        let item = reuse_item(raw, NAMESPACES, ENCLOSURE, &links).unwrap();
        assert_eq!(
            item,
            format!(
                "<item><guid>1</guid>\
                 <podcast:transcript url=\"https://archive.example/t?x=1&amp;y=2\" type=\"text/vtt\"/>\
                 {ENCLOSURE}\
                 <podcast:chapters type=\"application/json\" url='https://archive.example/c'/>\
                 <podcast:transcript url=\"https://cdn.example/not-archived.srt\"/></item>"
            )
        );
    }

    #[test]
    fn prefixes() {
        assert!(is_prefix("podcast") && is_prefix("_a-b.c"));
        assert!(!is_prefix("") && !is_prefix("1a") && !is_prefix("a b") && !is_prefix("xmlns"));
    }
}
