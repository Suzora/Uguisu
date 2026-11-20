//! Builds the feed Uguisu publishes for an archived podcast.
//!
//! Episode GUIDs are passed through unchanged, so a podcast app can recognise the episodes as the
//! ones it already knows from the original feed. Enclosures point at the archived files.

use std::fmt::Write;

use crate::model::{EpisodeView, Podcast};
use crate::util::xml_escape;

fn rfc2822(timestamp: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(timestamp, 0).map(|d| d.to_rfc2822())
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

/// Renders an RSS 2.0 document. Episodes without an archived file are left out, as are episodes
/// that disappeared from the source feed when the podcast is configured that way.
pub fn build_feed(podcast: &Podcast, episodes: &[EpisodeView], base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let token = &podcast.feed_token;
    let mut x = String::with_capacity(4096 + episodes.len() * 1024);
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.push_str(
        "<rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\" \
         xmlns:content=\"http://purl.org/rss/1.0/modules/content/\">\n<channel>\n",
    );
    let _ = writeln!(x, "<title>{}</title>", xml_escape(&podcast.title));
    let link = podcast
        .link
        .as_deref()
        .unwrap_or(&podcast.original_feed_url);
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

    for view in episodes {
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
            let escaped = xml_escape(description);
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
        let _ = writeln!(
            x,
            "<enclosure url=\"{}\" length=\"{size}\" type=\"{}\"/>",
            xml_escape(&media_url(base, token, e.id, file_name)),
            xml_escape(mime)
        );
        x.push_str("</item>\n");
    }
    x.push_str("</channel>\n</rss>\n");
    x
}
