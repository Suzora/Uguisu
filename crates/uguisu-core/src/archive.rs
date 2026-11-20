//! Layout of the archive directory.
//!
//! ```text
//! archive/
//!   <podcast-slug>-<hash>/
//!     podcast.json                     current podcast metadata
//!     cover.<ext>
//!     feeds/<timestamp>-<hash>.xml     every distinct version of the source feed, byte for byte
//!     episodes/<date>-<slug>-<hash>/
//!       episode.json                   current episode metadata
//!       item.xml                       raw <item> element as last seen
//!       media.<ext>                    the enclosure
//! ```
//!
//! The directory is meant to stay understandable without Uguisu and without the database.

use std::path::Path;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;

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
