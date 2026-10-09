//! What lives where under the media root, and which paths are Uguisu's own
//! bookkeeping rather than a user's media (ADR 0024).
//!
//! Exactly one top-level name is reserved: [`CONTROL_DIR`]. Manifests,
//! artwork and scratch files live under it, anchored at the media
//! root rather than inside a podcast's directory — because there is no
//! such thing as "the podcast's directory". Where a podcast's files land
//! is decided entirely by the path template, which a user may change to
//! `{episode.year}/{podcast.title}/…` or to a flat layout with no podcast
//! directory at all. A manifest under the template's output would be
//! stranded by the next template change; under the media root it only ever
//! needs marking stale.
//!
//! Every enumerator — the rebuild scan, the import scan, collision
//! occupancy, relocation — consults [`is_control_path`] and skips what it
//! names. Episode sidecars deliberately do **not** live here: a sidecar
//! sits next to its media file so that copying one episode out of the
//! archive takes its metadata along.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use uguisu_core::archive::ArtworkFormat;
use uguisu_core::ids::PodcastId;

use crate::path::RelativePath;

/// Uguisu's reserved directory at the media root.
pub const CONTROL_DIR: &str = ".uguisu";

/// The per-podcast scratch directory `.part` files are written into.
/// Restated here rather than imported: `uguisu-archive` must not depend on
/// `uguisu-download` (`docs/ARCHITECTURE.md` layering).
pub const DOWNLOAD_TMP_DIR: &str = ".uguisu-tmp";

/// Extension of a portable episode sidecar, appended to the media name.
pub const SIDECAR_EXT: &str = "json";

/// File name of a podcast's manifest.
pub const MANIFEST_FILE: &str = "manifest.sha256";

/// Whether one path component is Uguisu's bookkeeping rather than media.
///
/// Matches anything starting with `.uguisu`, so a directory added in a
/// later addition is covered without another rule — and so that the escape in
/// [`crate::sanitize::segment`] stays stable, since `._uguisu` no longer
/// matches.
#[must_use]
pub fn is_control_name(name: &str) -> bool {
    let name = name.trim();
    name.len() >= CONTROL_DIR.len()
        && name
            .get(..CONTROL_DIR.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(CONTROL_DIR))
}

/// Whether any component of a path is Uguisu's bookkeeping.
#[must_use]
pub fn is_control_path(relative: &RelativePath) -> bool {
    relative.components().any(is_control_name)
}

/// Whether a path names a portable sidecar.
#[must_use]
pub fn is_sidecar_path(relative: &RelativePath) -> bool {
    relative
        .file_name()
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case(SIDECAR_EXT))
}

/// The sidecar that belongs beside a media file.
///
/// Derived from the final media name and never templated on its own, so
/// the two can only ever be moved together.
#[must_use]
pub fn sidecar_of(media: &RelativePath) -> RelativePath {
    RelativePath::from_trusted(format!("{}.{SIDECAR_EXT}", media.as_str()))
}

/// The media file a sidecar belongs to, if the name has that shape.
#[must_use]
pub fn media_of(sidecar: &RelativePath) -> Option<RelativePath> {
    if !is_sidecar_path(sidecar) {
        return None;
    }
    let raw = sidecar.as_str();
    let cut = raw.len().checked_sub(SIDECAR_EXT.len() + 1)?;
    RelativePath::parse(raw.get(..cut)?).ok()
}

/// Where a podcast's manifest lives.
#[must_use]
pub fn manifest_path(podcast: PodcastId) -> RelativePath {
    RelativePath::from_trusted(format!("{CONTROL_DIR}/manifests/{podcast}/{MANIFEST_FILE}"))
}

/// Where one artwork image lives.
///
/// Content-addressed: the file is named after its hash, so storing a
/// replacement cannot overwrite the image before it. `hash` must be the
/// hex digest; it is lowercased here so the name is stable.
#[must_use]
pub fn artwork_path(podcast: PodcastId, hash: &str, format: ArtworkFormat) -> RelativePath {
    RelativePath::from_trusted(format!(
        "{CONTROL_DIR}/artwork/{podcast}/{}.{}",
        hash.to_ascii_lowercase(),
        format.extension()
    ))
}

/// A suffix no two concurrent writers can share.
///
/// Every atomic write here goes through a temporary file in the target's
/// own directory, and a *fixed* name made those writes race: two writers
/// of one manifest would take turns renaming each other's file away, and
/// the loser failed with "no such file". The process id keeps it unique
/// between processes, the counter within one.
///
/// The cost is that a crash can leave more than one leftover per target.
/// They are reported by `archive orphans` and never removed, which is the
/// same rule as every other leftover: a scratch file silently deleted is a
/// copy of somebody's media that nobody got to look at.
#[must_use]
pub fn tmp_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Where the archive engine puts a file it is still writing.
///
/// Under the media root, so the rename that publishes it stays inside one
/// file system even when the source was on another disk. Deliberately not
/// the per-podcast [`DOWNLOAD_TMP_DIR`]: mixing the two would make each
/// orphan scan report the other's work in progress.
#[must_use]
pub fn tmp_path(name: &str) -> RelativePath {
    RelativePath::from_trusted(format!("{CONTROL_DIR}/tmp/{name}"))
}

/// Whether `name` ends the way [`tmp_suffix`] ends a name.
#[must_use]
pub fn is_tmp_name(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let Some(rest) = name.strip_suffix(".tmp") else {
        return false;
    };
    let mut parts = rest.rsplitn(3, '.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(n), Some(pid), Some(stem)) if digits(n) && digits(pid) && !stem.is_empty()
    )
}

/// Scratch names a writer of this process holds, with how many hold each.
static HELD: Mutex<BTreeMap<OsString, usize>> = Mutex::new(BTreeMap::new());

fn held() -> MutexGuard<'static, BTreeMap<OsString, usize>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scratch file a writer of this process holds, from before it is created
/// until it is renamed into place or abandoned (ADR 0051).
///
/// The orphan report asks [`is_held`], so a write in progress is never
/// reported as a leftover. The key is the file name: scratch names are unique
/// within a process ([`tmp_suffix`], or an archive file id), while the same
/// file can be reached by more than one path.
#[derive(Debug)]
#[must_use = "a scratch file is held only while this value lives"]
pub struct Scratch(OsString);

impl Scratch {
    /// Holds the file name of `path` until the value is dropped.
    pub fn hold(path: &Path) -> Self {
        let name = path
            .file_name()
            .map(OsStr::to_os_string)
            .unwrap_or_default();
        *held().entry(name.clone()).or_default() += 1;
        Self(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let mut held = held();
        if let Some(count) = held.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.0);
            }
        }
    }
}

/// Whether a writer of this process holds a scratch file of this name.
#[must_use]
pub fn is_held(name: &OsStr) -> bool {
    held().contains_key(name)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::sanitize;
    use uguisu_core::archive::PathProfile;

    fn rel(s: &str) -> RelativePath {
        RelativePath::parse(s).unwrap()
    }

    #[test]
    fn control_paths_are_recognised_wherever_they_appear() {
        assert!(is_control_name(".uguisu"));
        assert!(is_control_name(".uguisu-tmp"));
        assert!(is_control_name(".UGUISU"));
        assert!(!is_control_name("uguisu"));
        assert!(!is_control_name("._uguisu"));
        assert!(!is_control_name(".av"));

        assert!(is_control_path(&rel(".uguisu/manifests/x/manifest.sha256")));
        assert!(is_control_path(&rel("Show/.uguisu-tmp/job.part")));
        assert!(!is_control_path(&rel("Show/2024/a.mp3")));
    }

    #[test]
    fn a_podcast_cannot_enter_dot_uguisu() {
        // Without this escape a feed titled `.uguisu` would render a
        // directory that shadows the manifests and artwork.
        for profile in PathProfile::ALL {
            let once = sanitize::segment(".uguisu", profile);
            assert_eq!(once, "._uguisu", "{profile}");
            assert!(!is_control_name(&once));
            assert_eq!(
                sanitize::segment(&once, profile),
                once,
                "{profile}: escaping is stable on a second pass"
            );
            assert_eq!(sanitize::segment(".uguisu-tmp", profile), "._uguisu-tmp");
            // An ordinary leading dot is left alone.
            assert_eq!(sanitize::segment(".hidden", profile), ".hidden");
        }
    }

    #[test]
    fn a_sidecar_path_round_trips() {
        let media = rel("Show/2024/2024-01-05 - Folge 1.mp3");
        let side = sidecar_of(&media);
        assert_eq!(side.as_str(), "Show/2024/2024-01-05 - Folge 1.mp3.json");
        assert!(is_sidecar_path(&side));
        assert!(!is_sidecar_path(&media));
        assert_eq!(media_of(&side), Some(media));
        assert_eq!(media_of(&rel("Show/a.MP3.JSON")), Some(rel("Show/a.MP3")));
        assert_eq!(media_of(&rel("Show/a.mp3")), None);
        assert_eq!(
            media_of(&rel("Show/.json")),
            None,
            "a bare extension is not a sidecar"
        );
    }

    #[test]
    fn asset_paths_anchor_at_the_root() {
        let p: PodcastId = "01J0000000000000000000000P".parse().unwrap();
        let m = manifest_path(p);
        assert_eq!(
            m.as_str(),
            ".uguisu/manifests/01J0000000000000000000000P/manifest.sha256"
        );
        assert!(is_control_path(&m));
        let a = artwork_path(p, "AB12", ArtworkFormat::Jpeg);
        assert_eq!(
            a.as_str(),
            ".uguisu/artwork/01J0000000000000000000000P/ab12.jpg",
            "the hash names the file, so a replacement cannot overwrite it"
        );
        assert!(is_control_path(&tmp_path("01J.import")));
    }

    #[test]
    fn tmp_names_match_the_suffix() {
        assert!(is_tmp_name(&format!("manifest.sha256{}", tmp_suffix())));
        assert!(is_tmp_name("Folge 1.mp3.json.12.0.tmp"));
        for name in [
            "notes.tmp",
            ".12.0.tmp",
            "a.12.x.tmp",
            "a.12.0.tmp.bak",
            "a.mp3",
        ] {
            assert!(!is_tmp_name(name), "{name}");
        }
    }

    #[test]
    fn held_scratch_released_on_drop() {
        let path = Path::new("/media/.uguisu/tmp").join(format!("held{}", tmp_suffix()));
        let name = path.file_name().unwrap().to_owned();
        assert!(!is_held(&name));
        let first = Scratch::hold(&path);
        let second = Scratch::hold(&path);
        assert!(is_held(&name));
        drop(first);
        assert!(is_held(&name), "still held by the second writer");
        drop(second);
        assert!(!is_held(&name));
    }
}
