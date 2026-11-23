//! Layout of the archive directory (format 5, see `docs/ARCHIVE.md`).
//!
//! ```text
//! archive/
//!   <author> - <title>/                      one directory per podcast
//!     podcast.json, uguisu.json, cover.<ext>
//!     feeds/<timestamp>-<hash>.xml           every distinct version of the source feed
//!     episodes/<episode>.<ext>               the media files, and nothing else
//!     metadata/<episode>/                    everything else about an episode
//!       episode.json, item.xml, media.<ext>.blake3, linked files
//!       versions/v<n>.<ext>                  earlier media files the source replaced
//! ```
//!
//! `<episode>` is the publication time and the title, e.g. `2024-10-23 22.07 - Trailer`. Names
//! are for people; nothing depends on parsing them. The directory is meant to stay
//! understandable without Uguisu and without the database.

use std::path::Path;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;

/// Name of a podcast's directory: `<author> - <title>`, or the title alone when there is no
/// author or the author is the podcast itself.
pub fn podcast_name(author: Option<&str>, title: &str) -> String {
    let author = author
        .map(str::trim)
        .filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case(title.trim()));
    let full = match author {
        Some(author) => format!("{author} - {title}"),
        None => title.to_string(),
    };
    crate::util::file_name_from_title(&full, "Podcast")
}

/// Name of an episode's media file (without extension) and of its metadata directory: the
/// publication time in UTC, so that the names sort in order, and the title.
pub fn episode_name(published_at: Option<i64>, title: &str) -> String {
    let when = published_at
        .and_then(|t| crate::util::format_utc(t, "%Y-%m-%d %H.%M"))
        .unwrap_or_else(|| "undated".to_string());
    format!(
        "{when} - {}",
        crate::util::file_name_from_title(title, "Untitled")
    )
}

/// `<podcast>/metadata/<episode>`, relative to the archive.
pub fn metadata_dir(podcast_dir: &str, episode: &str) -> String {
    format!("{podcast_dir}/metadata/{episode}")
}

/// `<podcast>/episodes/<episode>.<ext>`, relative to the archive: the current media file.
pub fn media_path(podcast_dir: &str, episode: &str, ext: &str) -> String {
    format!("{podcast_dir}/episodes/{episode}.{ext}")
}

/// Where the checksum of a media file is kept, relative to the archive, and the path to the
/// media file as written into it (relative to the checksum file, as `b3sum -c` expects).
pub fn checksum_location(media_path: &str) -> (String, String) {
    let (podcast_dir, rest) = media_path.split_once('/').unwrap_or(("", media_path));
    match rest.strip_prefix("episodes/") {
        Some(file) => {
            let episode = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
            let ext = file.rsplit_once('.').map_or("bin", |(_, ext)| ext);
            (
                format!("{}/media.{ext}.blake3", metadata_dir(podcast_dir, episode)),
                format!("../../episodes/{file}"),
            )
        }
        // Earlier versions, and archives in older formats: next to the file.
        None => {
            let name = media_path.rsplit('/').next().unwrap_or(media_path);
            (format!("{media_path}.blake3"), name.to_string())
        }
    }
}

/// Name of a media file in published addresses: `media.<ext>`. Addresses do not follow the
/// names on disk, so they stay the same when the archive is reorganised.
pub fn published_media_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, ext)) => format!("media.{ext}"),
        None => "media".to_string(),
    }
}

/// Writes a file atomically: the content is flushed to a temporary sibling and then renamed.
pub async fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("cannot create {}", parent.display()))?;
    let mut tmp_name = path
        .file_name()
        .context("path has no file name")?
        .to_os_string();
    tmp_name.push(".part");
    let tmp = parent.join(tmp_name);
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .with_context(|| format!("cannot create {}", tmp.display()))?;
    file.write_all(content).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&tmp, path)
        .await
        .with_context(|| format!("cannot move {} into place", tmp.display()))?;
    Ok(())
}

/// Hashes a file with BLAKE3. Returns the hex digest and the size in bytes.
pub fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

const MEDIA_TYPES: &[(&str, &str)] = &[
    ("mp3", "audio/mpeg"),
    ("m4a", "audio/mp4"),
    ("m4b", "audio/mp4"),
    ("aac", "audio/aac"),
    ("ogg", "audio/ogg"),
    ("oga", "audio/ogg"),
    ("opus", "audio/opus"),
    ("flac", "audio/flac"),
    ("wav", "audio/wav"),
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mov", "video/quicktime"),
    ("webm", "video/webm"),
    ("mkv", "video/x-matroska"),
    ("pdf", "application/pdf"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("vtt", "text/vtt"),
    ("srt", "application/x-subrip"),
    ("json", "application/json"),
    ("txt", "text/plain"),
    ("html", "text/html"),
];

/// Known extension at the end of a URL path, lowercased.
pub fn extension_from_url(url: &str) -> Option<&'static str> {
    let parsed = url::Url::parse(url).ok()?;
    let ext = parsed
        .path()
        .rsplit('/')
        .next()?
        .rsplit_once('.')?
        .1
        .to_ascii_lowercase();
    MEDIA_TYPES.iter().find(|(e, _)| *e == ext).map(|(e, _)| *e)
}

pub fn extension_from_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next()?.trim().to_ascii_lowercase();
    match mime.as_str() {
        "audio/mp3" | "audio/mpeg3" => return Some("mp3"),
        "audio/x-m4a" | "audio/m4a" => return Some("m4a"),
        "application/srt" | "text/srt" => return Some("srt"),
        "application/json+chapters" => return Some("json"),
        _ => {}
    }
    MEDIA_TYPES
        .iter()
        .find(|(_, m)| *m == mime)
        .map(|(e, _)| *e)
}

pub fn mime_from_extension(ext: &str) -> Option<&'static str> {
    MEDIA_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn podcast_names() {
        assert_eq!(
            podcast_name(Some("funk - von ARD und ZDF"), "TeleKaddi"),
            "funk - von ARD und ZDF - TeleKaddi"
        );
        assert_eq!(
            podcast_name(Some("Distractible"), "Distractible"),
            "Distractible"
        );
        assert_eq!(podcast_name(Some(" "), "Show"), "Show");
        assert_eq!(podcast_name(None, "Show: Live"), "Show Live");
    }

    #[test]
    fn episode_names() {
        assert_eq!(
            episode_name(Some(1_729_721_220), "Kinderserien: Trailer"),
            "2024-10-23 22.07 - Kinderserien Trailer"
        );
        assert_eq!(episode_name(None, "?"), "undated - Untitled");
    }

    #[test]
    fn extensions() {
        assert_eq!(
            extension_from_url("https://x.test/a/b/Ep1.MP3?token=1"),
            Some("mp3")
        );
        assert_eq!(extension_from_url("https://x.test/download"), None);
        assert_eq!(extension_from_mime("audio/mpeg; charset=x"), Some("mp3"));
        assert_eq!(extension_from_mime("audio/x-m4a"), Some("m4a"));
        assert_eq!(mime_from_extension("opus"), Some("audio/opus"));
    }
}
