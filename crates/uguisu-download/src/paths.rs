//! The identifier layout (ADR 0020): `<podcast id>/<episode id>.<ext>`,
//! with the `.part` always under `<podcast id>/.uguisu-tmp/<job id>.part`,
//! relative to the media directory with POSIX separators. A target is the
//! rendered template path the engine's `DestinationResolver` answers at
//! enqueue (ADR 0022); this layout is the fallback when it answers nothing
//! usable.
//!
//! Every component here is a ULID or a whitelisted extension, so no
//! user-controlled string reaches the file system.

use std::path::{Component, Path, PathBuf};

use uguisu_core::ids::{EpisodeId, JobId, PodcastId};
use url::Url;

/// Name of the per-podcast directory holding `.part` files.
pub const TMP_DIR: &str = ".uguisu-tmp";
/// Extension of a partial download.
pub const PART_EXT: &str = "part";
/// Fallback extension when neither MIME type nor URL yields a known one.
pub const FALLBACK_EXT: &str = "bin";

/// A relative path that failed validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsafe media path `{path}`: {reason}")]
pub struct PathError {
    /// The offending relative path.
    pub path: String,
    /// Why.
    pub reason: &'static str,
}

/// Target and `.part` locations of one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// Final path relative to the media directory.
    pub target: String,
    /// `.part` path relative to the media directory.
    pub part: String,
}

impl Destination {
    /// The identifier layout for a job.
    #[must_use]
    pub fn for_job(podcast_id: PodcastId, episode_id: EpisodeId, job_id: JobId, ext: &str) -> Self {
        Self {
            target: target_relative(podcast_id, episode_id, ext),
            part: part_relative(podcast_id, job_id),
        }
    }
}

/// `<podcast id>/<episode id>.<ext>`.
#[must_use]
pub fn target_relative(podcast_id: PodcastId, episode_id: EpisodeId, ext: &str) -> String {
    format!("{podcast_id}/{episode_id}.{ext}")
}

/// `<podcast id>/.uguisu-tmp/<job id>.part`.
#[must_use]
pub fn part_relative(podcast_id: PodcastId, job_id: JobId) -> String {
    format!("{podcast_id}/{TMP_DIR}/{job_id}.{PART_EXT}")
}

/// Picks a file extension: from the declared MIME type when it is a known
/// media type, else from the URL's path when it is short and alphanumeric,
/// else [`FALLBACK_EXT`].
#[must_use]
pub fn extension_for(mime: Option<&str>, url: &Url) -> String {
    if let Some(ext) = mime.and_then(mime_extension) {
        return ext.to_owned();
    }
    let path = url.path();
    let name = path.rsplit('/').next().unwrap_or_default();
    if let Some((_, ext)) = name.rsplit_once('.') {
        let ext = ext.to_ascii_lowercase();
        if (1..=5).contains(&ext.len())
            && ext
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return ext;
        }
    }
    FALLBACK_EXT.to_owned()
}

/// Known media MIME types and their extensions.
#[must_use]
pub fn mime_extension(mime: &str) -> Option<&'static str> {
    let essence = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    Some(match essence.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg-3" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/aac" | "audio/aacp" | "audio/x-aac" => "aac",
        "audio/ogg" | "application/ogg" | "audio/vorbis" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
        "audio/webm" => "weba",
        "video/mp4" => "mp4",
        "video/x-m4v" => "m4v",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "video/x-matroska" => "mkv",
        _ => return None,
    })
}

/// Joins a stored relative path onto the media directory after checking
/// that it is made of plain components only (no `..`, no root, no drive,
/// no separators inside a component) and stays inside `media_dir`.
pub fn resolve(media_dir: &Path, relative: &str) -> Result<PathBuf, PathError> {
    let err = |reason| PathError {
        path: relative.to_owned(),
        reason,
    };
    if relative.is_empty() {
        return Err(err("empty"));
    }
    if relative.contains('\\') {
        return Err(err("backslash"));
    }
    let mut out = media_dir.to_path_buf();
    for seg in relative.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(err("dot or empty segment"));
        }
        if seg.contains(':') || seg.contains('\0') {
            return Err(err("reserved character"));
        }
        let p = Path::new(seg);
        let mut comps = p.components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(_)), None) => {}
            _ => return Err(err("not a plain component")),
        }
        out.push(seg);
    }
    if !out.starts_with(media_dir) {
        return Err(err("escapes the media directory"));
    }
    Ok(out)
}

/// Moves `from` to `to`, and never over an existing file, also not over one
/// whose name differs only in case on a case-insensitive file system.
///
/// `std::fs::rename` replaces a target, and a check before it leaves a window
/// and compares one spelling. A hard link fails when the name is taken in any
/// case the file system folds, so it is made first and the old name removed
/// after. Where linking fails for another reason (FAT, some network shares)
/// this falls back to check, then rename, and a caller's retry sees the
/// rename's own error.
///
/// When the old name cannot be removed after a link (a scanner holds it on
/// Windows), the new name is taken back and that error is returned, so a retry
/// starts over. If the new name cannot be taken back either, the move has
/// happened: the target is complete and the old name is a second name for the
/// same bytes, which the orphan scans report and nothing removes. A crash
/// between the link and the removal leaves the same state.
///
/// # Errors
/// `AlreadyExists` when `to` is taken; the removal's or the fallback rename's
/// error otherwise.
pub fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => match std::fs::remove_file(from) {
            Ok(()) => Ok(()),
            Err(e) => std::fs::remove_file(to).map_or(Ok(()), |()| Err(e)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            if to.try_exists()? {
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
            }
            std::fs::rename(from, to)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::many_single_char_names)]

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn layout_and_extensions() {
        let p = PodcastId::new();
        let e = EpisodeId::new();
        let j = JobId::new();
        let d = Destination::for_job(p, e, j, "mp3");
        assert_eq!(d.target, format!("{p}/{e}.mp3"));
        assert_eq!(d.part, format!("{p}/.uguisu-tmp/{j}.part"));
        let u = |s: &str| Url::parse(s).unwrap();
        assert_eq!(extension_for(Some("audio/mpeg"), &u("https://x/a")), "mp3");
        assert_eq!(
            extension_for(Some("audio/mpeg; charset=binary"), &u("https://x/a.ogg")),
            "mp3"
        );
        assert_eq!(
            extension_for(Some("application/octet-stream"), &u("https://x/a.M4A?x=1")),
            "m4a"
        );
        assert_eq!(extension_for(None, &u("https://x/a.mp3/")), "bin");
        assert_eq!(extension_for(None, &u("https://x/a.mpeg-audio")), "bin");
        assert_eq!(extension_for(None, &u("https://x/a")), "bin");
        assert_eq!(extension_for(Some("video/mp4"), &u("https://x/a")), "mp4");
        assert_eq!(mime_extension("Audio/OGG"), Some("ogg"));
    }

    #[test]
    fn resolve_accepts_only_the_layout() {
        let root = Path::new("/media");
        let p = PodcastId::new();
        let e = EpisodeId::new();
        let ok = resolve(root, &target_relative(p, e, "mp3")).unwrap();
        assert!(ok.starts_with(root));
        assert!(ok.ends_with(format!("{e}.mp3")));
        for bad in [
            "",
            "../x.mp3",
            "a/../x.mp3",
            "/etc/passwd",
            "a//b",
            "a/./b",
            "a\\b",
            "C:/x",
            "a\0b",
        ] {
            assert!(resolve(root, bad).is_err(), "{bad:?}");
        }
    }

    proptest! {
        #[test]
        fn generated_paths_never_escape(mime in prop::option::of("[a-z]{3,8}/[a-z0-9.+-]{2,12}"), url_ext in "[a-zA-Z0-9._-]{0,8}") {
            let url = Url::parse(&format!("https://cdn.example/media/file.{url_ext}")).unwrap();
            let ext = extension_for(mime.as_deref(), &url);
            prop_assert!((1..=5).contains(&ext.len()));
            prop_assert!(ext.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
            let d = Destination::for_job(PodcastId::new(), EpisodeId::new(), JobId::new(), &ext);
            let root = Path::new("/srv/media");
            let t = resolve(root, &d.target).unwrap();
            let part = resolve(root, &d.part).unwrap();
            prop_assert!(t.starts_with(root) && part.starts_with(root));
            prop_assert_eq!(t.parent().unwrap(), part.parent().unwrap().parent().unwrap());
        }
    }

    #[test]
    fn rename_new_moves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a.part"), dir.path().join("a.mp3"));
        std::fs::write(&from, b"audio").unwrap();
        rename_new(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"audio");
    }

    #[test]
    fn rename_new_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a.part"), dir.path().join("a.mp3"));
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"theirs").unwrap();
        let e = rename_new(&from, &to).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&to).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&from).unwrap(), b"new");
    }

    #[test]
    fn case_variant_target_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("probe"), b"").unwrap();
        let folds_case = dir.path().join("PROBE").exists();
        let (from, theirs) = (dir.path().join("a.part"), dir.path().join("Ep.mp3"));
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&theirs, b"theirs").unwrap();
        let result = rename_new(&from, &dir.path().join("EP.mp3"));
        assert_eq!(
            std::fs::read(&theirs).unwrap(),
            b"theirs",
            "folds case: {folds_case}"
        );
        if folds_case {
            assert_eq!(
                result.unwrap_err().kind(),
                std::io::ErrorKind::AlreadyExists
            );
        } else {
            result.unwrap();
        }
    }
}
