//! What nothing owns under the media root (ADR 0051).
//!
//! A read-only report: scratch files of interrupted writes, `.part` files no
//! job owns, media files no record names, and sidecars whose media file is
//! gone. Nothing here writes, moves or deletes a file, and nothing runs it at
//! start-up: it walks the whole tree, and every embedded command opens the
//! engine.

use std::collections::HashSet;
use std::path::Path;

use uguisu_archive::layout;
use uguisu_archive::manifest::Findings;
use uguisu_archive::scan::{self, MEDIA_EXTENSIONS, ScanOptions, ScannedFile};
use uguisu_core::UguisuError;
use uguisu_storage::{archive_files, downloads};

use crate::Engine;
use crate::rebuild::note;

/// What `archive orphans` found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrphanReport {
    /// Files looked at: media, sidecars and scratch files.
    pub scanned: u64,
    /// Scratch files of an interrupted write that no writer holds.
    pub leftovers: Findings,
    /// `.part` files no download job owns.
    pub orphan_parts: Findings,
    /// Media files no archive record names.
    pub unknown_media: Findings,
    /// Sidecars whose media file is not there.
    pub stray_sidecars: Findings,
    /// Entries the walk could not look at (a link, an unreadable directory).
    pub unreadable: Findings,
}

impl OrphanReport {
    /// Whether anything was found.
    #[must_use]
    pub const fn has_findings(&self) -> bool {
        !self.leftovers.is_empty()
            || !self.orphan_parts.is_empty()
            || !self.unknown_media.is_empty()
            || !self.stray_sidecars.is_empty()
            || !self.unreadable.is_empty()
    }
}

impl Engine {
    /// Walks the media root for what nothing owns (ADR 0051). Reads only.
    ///
    /// What judges a file — a writer holding it, a record naming it, a job
    /// owning it — is asked after the file was seen, so work that finishes
    /// during the walk is not reported.
    pub async fn orphans(&self) -> Result<OrphanReport, UguisuError> {
        let root = self.media_root()?;
        let mut report = OrphanReport::default();
        // Nothing has been downloaded yet. Any other failure to read the
        // root is the walk's to report.
        if std::fs::symlink_metadata(&root).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(report);
        }
        control_leftovers(&root, &mut report);

        let mut extensions: Vec<String> =
            MEDIA_EXTENSIONS.iter().map(|e| (*e).to_owned()).collect();
        extensions.extend([layout::SIDECAR_EXT.to_owned(), "tmp".to_owned()]);
        let options = ScanOptions {
            read_head: false,
            extensions,
            ..ScanOptions::default()
        };
        let mut conn = self.storage().reader().await?;
        let mut unknown: Vec<ScannedFile> = Vec::new();
        // The walk is a blocking iterator awaited one entry at a time, as in
        // a rebuild: collecting it first would hold the whole tree.
        for entry in scan::walk(&root, &options) {
            let file = match entry {
                Ok(f) => f,
                Err(e) => {
                    let path = e.path();
                    let shown = path.strip_prefix(&root).unwrap_or(path);
                    note(&mut report.unreadable, &shown.to_string_lossy());
                    continue;
                }
            };
            report.scanned += 1;
            match file.extension().as_deref() {
                Some("tmp") => {
                    if layout::is_tmp_name(file.relative.file_name()) && is_leftover(&file.absolute)
                    {
                        note(&mut report.leftovers, file.relative.as_str());
                    }
                }
                Some(layout::SIDECAR_EXT) => {
                    // `<name>.<media extension>.json` only: a user's
                    // `notes.json` is not a sidecar.
                    if let Some(media) = layout::media_of(&file.relative)
                        && is_media_name(media.file_name())
                        && !root.join(media.as_str()).exists()
                    {
                        note(&mut report.stray_sidecars, file.relative.as_str());
                    }
                }
                _ => {
                    if archive_files::owner_of_path(&mut conn, file.relative.as_str())
                        .await?
                        .is_none()
                    {
                        unknown.push(file);
                    }
                }
            }
        }

        // Listed before the job ids are read: a job that starts meanwhile
        // has a row by the time it is asked about.
        let parts = uguisu_download::part_files(&root);
        let jobs: HashSet<String> = downloads::job_ids_owning_parts(&mut conn)
            .await?
            .iter()
            .map(ToString::to_string)
            .collect();
        for part in parts {
            let stem = Path::new(&part)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if !jobs.contains(stem) {
                note(&mut report.orphan_parts, &part);
            }
        }
        // A download or an import that finished during the walk has its
        // record now, or has moved its file on.
        for file in unknown {
            if file.absolute.exists()
                && archive_files::owner_of_path(&mut conn, file.relative.as_str())
                    .await?
                    .is_none()
            {
                note(&mut report.unknown_media, file.relative.as_str());
            }
        }
        Ok(report)
    }
}

/// Scratch files under `.uguisu`: anything in `tmp/`, and `tmp_suffix`
/// names beside the artwork and the manifests.
fn control_leftovers(root: &Path, report: &mut OrphanReport) {
    let control = root.join(layout::CONTROL_DIR);
    let mut dirs = vec![(
        control.join("tmp"),
        format!("{}/tmp", layout::CONTROL_DIR),
        true,
    )];
    for kind in ["artwork", "manifests"] {
        let Ok(entries) = std::fs::read_dir(control.join(kind)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let name = entry.file_name();
                let relative = format!("{}/{kind}/{}", layout::CONTROL_DIR, name.to_string_lossy());
                dirs.push((entry.path(), relative, false));
            }
        }
    }
    dirs.sort();
    for (dir, relative, every) in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name())
            .collect();
        names.sort();
        for name in names {
            let text = name.to_string_lossy();
            if !every && !layout::is_tmp_name(&text) {
                continue;
            }
            report.scanned += 1;
            if is_leftover(&dir.join(&name)) {
                note(&mut report.leftovers, &format!("{relative}/{text}"));
            }
        }
    }
}

/// Held first, then present: a writer that lets go has renamed its file
/// away, so a file it held a moment ago is not reported.
fn is_leftover(path: &Path) -> bool {
    path.file_name().is_some_and(|name| !layout::is_held(name)) && path.exists()
}

fn is_media_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && MEDIA_EXTENSIONS.iter().any(|m| ext.eq_ignore_ascii_case(m))
    })
}
