//! The handful of settings that are genuinely the desktop's own.
//!
//! Uguisu keeps its configuration; this file holds only what the shell decides
//! and the server has no opinion about — where the window put the archive, and
//! whether the tray, autostart and notifications are wanted. It is a small
//! JSON file beside the app's other per-user state, written the way Uguisu
//! writes everything: to a temporary file, then renamed over the old one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the shell remembers between launches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Desktop {
    /// Where the user pointed the archive, if they ever did.
    ///
    /// Only consulted when Uguisu's own configuration does not already name a
    /// media directory: an `UGUISU_MEDIA_DIR` in the environment is pinned and
    /// stays authoritative.
    pub media_root: Option<PathBuf>,
    /// Whether finished and failed downloads raise a native notification.
    pub notifications: bool,
    /// Whether the shell asked the OS to start it at login.
    pub autostart: bool,
}

impl Desktop {
    /// Reads the file, or the defaults when it is missing or unreadable.
    ///
    /// A corrupt file is not a startup failure: the defaults are all usable,
    /// and refusing to launch over a preferences file would be worse than the
    /// preference being lost.
    pub fn load(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::first_run();
        };
        let mut settings: Self = serde_json::from_str(&raw).unwrap_or_else(|error| {
            tracing::warn!(%error, "the desktop settings file could not be read");
            Self::first_run()
        });
        // A stored folder may be in Windows' `\\?\` form, which the engine
        // and the settings panel would otherwise carry as is.
        settings.media_root = settings
            .media_root
            .map(|root| dunce::simplified(&root).to_path_buf());
        settings
    }

    /// Notifications on, autostart off, no archive chosen yet.
    fn first_run() -> Self {
        Self {
            media_root: None,
            notifications: true,
            autostart: false,
        }
    }

    /// Writes the file, creating its directory: to a temporary file of this
    /// writer's own, synced, then renamed over the old one.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(uguisu_archive::layout::tmp_suffix());
        let temporary = path.with_file_name(name);
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    }
}

/// The folder as the archive root will use it, or why it cannot be one.
///
/// Never creates the folder: one on a drive that is not connected must be
/// refused, not recreated on the disk underneath it (ADR 0044).
pub fn usable_folder(folder: &Path) -> Result<PathBuf, String> {
    let metadata =
        std::fs::metadata(folder).map_err(|e| format!("that folder cannot be reached: {e}"))?;
    if !metadata.is_dir() {
        return Err("that is not a folder".to_owned());
    }
    // Writable, and a real directory rather than a symlink into somewhere
    // else: the same two things the engine would find out the hard way. The
    // probe's name is this check's own, and `create_new` never opens a file
    // that is already there.
    let mut name = std::ffi::OsString::from(".uguisu-write-test");
    name.push(uguisu_archive::layout::tmp_suffix());
    let probe = folder.join(name);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| format!("that folder cannot be written to: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    // `dunce` keeps Windows' `\\?\` prefix off a path that does not need it,
    // so the folder reads the way the person picked it.
    dunce::canonicalize(folder).map_err(|e| format!("that folder cannot be resolved: {e}"))
}

/// Where the settings file lives inside the app's per-user configuration.
pub const FILE: &str = "desktop.json";
