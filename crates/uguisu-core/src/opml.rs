//! OPML import and export, the exchange format for subscription lists.

use std::fmt::Write;

use anyhow::{Context, Result};
use roxmltree::{Document, ParsingOptions};

use crate::model::Podcast;
use crate::rss::feed_url;
use crate::util::xml_escape;

/// Extracts all feed URLs (`xmlUrl` attributes) from an OPML document, in document order.
pub fn parse(bytes: &[u8]) -> Result<Vec<String>> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(text, options).context("the file is not valid XML")?;
    let mut urls: Vec<String> = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("outline")) {
        let url = node
            .attributes()
            .find(|a| a.name().eq_ignore_ascii_case("xmlurl"))
            .map(|a| a.value().trim().to_string());
        if let Some(url) = url
            && !url.is_empty()
            && !urls.contains(&url)
        {
            urls.push(url);
        }
    }
    Ok(urls)
}

/// OPML listing the feeds Uguisu publishes, ready to import into a podcast app.
pub fn export(podcasts: &[Podcast], base_url: &str) -> String {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n");
    x.push_str("<head><title>Uguisu archive</title></head>\n<body>\n");
    for p in podcasts {
        let _ = writeln!(
            x,
            "<outline type=\"rss\" text=\"{title}\" title=\"{title}\" xmlUrl=\"{url}\"{html}/>",
            title = xml_escape(&p.title),
            url = xml_escape(&feed_url(base_url, &p.feed_token)),
            html = p
                .link
                .as_deref()
                .map(|l| format!(" htmlUrl=\"{}\"", xml_escape(l)))
                .unwrap_or_default(),
        );
    }
    x.push_str("</body>\n</opml>\n");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_outlines() {
        let opml = br#"<?xml version="1.0"?>
<opml version="1.0"><body>
  <outline text="Group">
    <outline type="rss" text="A" xmlUrl="https://a.test/feed"/>
    <outline type="rss" text="B" xmlurl=" https://b.test/feed "/>
    <outline type="rss" text="A again" xmlUrl="https://a.test/feed"/>
  </outline>
  <outline text="no url"/>
</body></opml>"#;
        assert_eq!(
            parse(opml).unwrap(),
            vec!["https://a.test/feed", "https://b.test/feed"]
        );
    }
}
