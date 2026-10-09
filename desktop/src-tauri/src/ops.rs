//! The native actions the web UI can ask for, and nothing else.
//!
//! Each one is either an OS call or a write to the desktop's own settings
//! file. None of them reads a feed, decides a path, or touches the database:
//! that work stays in the engine, reached over the same HTTP API a browser
//! uses.
//!
//! Two lints are off for the whole module rather than at seven call sites: a
//! Tauri command's handle and state arrive by value because that is the shape
//! the macro requires, and `Report` is a wire struct whose fields happen to be
//! four independent yes-or-no answers about this machine.
#![allow(clippy::needless_pass_by_value, clippy::struct_excessive_bools)]

use std::path::PathBuf;
use std::sync::Mutex;

use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;

use crate::config::{self, Desktop};
use crate::reveal::locate;

/// Everything the commands below need, resolved once at startup.
#[derive(Debug)]
pub struct Native {
    /// Where the archive actually is for this launch.
    pub media_root: PathBuf,
    /// True when `UGUISU_MEDIA_DIR` fixed it, so the picker must not claim
    /// otherwise.
    pub media_pinned: bool,
    /// Where the desktop settings file lives.
    pub settings_path: PathBuf,
    /// The settings themselves.
    pub settings: Mutex<Desktop>,
    /// The login item, or `None` where autostart is not offered.
    pub login_item: Option<AutoLaunch>,
}

/// What the web UI is told about its host.
#[derive(Debug, serde::Serialize)]
pub struct Report {
    media_root: String,
    media_pinned: bool,
    notifications: bool,
    autostart: bool,
    autostart_supported: bool,
}

/// Whether this process is inside a Flatpak sandbox.
///
/// The file is the sandbox's own marker and is the documented way to ask.
pub fn sandboxed() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// The login item for this executable, or `None` where autostart is not offered.
///
/// Autostart writes a desktop entry into the user's configuration, which
/// inside a sandbox lands where the host session never reads it. Rather than
/// offer a switch that silently does nothing, the UI is told it is
/// unavailable.
///
/// Built here rather than by `tauri-plugin-autostart`, which writes the Windows
/// Run value unquoted: `C:\Program Files\Uguisu\uguisu-desktop.exe` is then
/// tried as `C:\Program.exe` first.
fn login_item(app: &tauri::App) -> Option<AutoLaunch> {
    if sandboxed() {
        return None;
    }
    let exe = std::env::current_exe()
        .inspect_err(
            |error| tracing::warn!(%error, "no autostart: the executable's path is unknown"),
        )
        .ok()?;
    // An AppImage starts through its image, not the binary it mounts.
    #[cfg(target_os = "linux")]
    let path = app.env().appimage.map_or_else(
        || exe.display().to_string(),
        |image| image.to_string_lossy().into_owned(),
    );
    #[cfg(not(target_os = "linux"))]
    let path = format!("\"{}\"", exe.display());
    AutoLaunchBuilder::new()
        .set_app_name(&app.package_info().name)
        .set_app_path(&path)
        .build()
        .inspect_err(|error| tracing::warn!(%error, "no autostart"))
        .ok()
}

fn store(native: &Native, change: impl FnOnce(&mut Desktop)) -> Result<Desktop, String> {
    let mut held = native
        .settings
        .lock()
        .map_err(|_| "the desktop settings are unavailable".to_owned())?;
    change(&mut held);
    held.save(&native.settings_path)
        .map_err(|e| format!("the desktop settings could not be saved: {e}"))?;
    Ok(held.clone())
}

fn report(native: &Native) -> Result<Report, String> {
    let held = native
        .settings
        .lock()
        .map_err(|_| "the desktop settings are unavailable".to_owned())?;
    Ok(Report {
        media_root: native.media_root.display().to_string(),
        media_pinned: native.media_pinned,
        notifications: held.notifications,
        autostart: held.autostart && native.login_item.is_some(),
        autostart_supported: native.login_item.is_some(),
    })
}

/// What the shell is, where it put things, and what it can do here.
#[tauri::command]
pub fn desktop_report(native: tauri::State<'_, Native>) -> Result<Report, String> {
    report(&native)
}

/// Asks the OS for a folder and remembers it as the archive root.
///
/// The path is validated before it is stored — a picker returning something
/// is not a reason to trust it — and it takes effect at the next launch,
/// because Uguisu never moves media and the engine resolved its directories
/// when it opened.
#[tauri::command]
pub async fn choose_media_root(
    app: tauri::AppHandle,
    native: tauri::State<'_, Native>,
) -> Result<Report, String> {
    if native.media_pinned {
        return Err("UGUISU_MEDIA_DIR is set, so the archive directory is fixed".to_owned());
    }
    let picked = app.dialog().file().blocking_pick_folder();
    let Some(picked) = picked else {
        return report(&native);
    };
    let chosen = picked
        .into_path()
        .map_err(|e| format!("that folder cannot be used: {e}"))?;
    let resolved = config::usable_folder(&chosen)?;
    store(&native, |settings| {
        settings.media_root = Some(resolved);
    })
    .and_then(|_| report(&native))
}

/// Turns native notifications on or off for this installation.
#[tauri::command]
pub fn set_notifications(
    native: tauri::State<'_, Native>,
    enabled: bool,
) -> Result<Report, String> {
    store(&native, |settings| settings.notifications = enabled)?;
    report(&native)
}

/// Asks the OS to start Uguisu at login, or to stop doing so.
#[tauri::command]
pub fn set_autostart(native: tauri::State<'_, Native>, enabled: bool) -> Result<Report, String> {
    let Some(item) = &native.login_item else {
        return Err("starting at login is not available here".to_owned());
    };
    let outcome = if enabled {
        item.enable()
    } else {
        // An installer may have removed it already, which is what was asked for.
        item.disable().or_else(|error| match item.is_enabled() {
            Ok(false) => Ok(()),
            _ => Err(error),
        })
    };
    outcome.map_err(|e| format!("the login item could not be changed: {e}"))?;
    store(&native, |settings| settings.autostart = enabled)?;
    report(&native)
}

/// Shows a native notification, if the user left them on.
///
/// Best-effort by design: a desktop with no notification daemon is a normal
/// desktop, and a missing toast is not worth an error in the UI.
#[tauri::command]
pub fn notify(
    app: tauri::AppHandle,
    native: tauri::State<'_, Native>,
    title: String,
    body: String,
) {
    let wanted = native.settings.lock().is_ok_and(|s| s.notifications);
    if !wanted {
        return;
    }
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!(%error, "a notification was not shown");
    }
}

/// Shows an archived file in the system file manager.
///
/// Takes the archive-relative path the API already publishes and resolves it
/// against the media root with Uguisu's own path rules, so a page cannot ask
/// for a file outside the archive however it spells the request. Nothing here
/// builds a command line.
#[tauri::command]
pub fn reveal(native: tauri::State<'_, Native>, path: String) -> Result<(), String> {
    let target = locate(&native.media_root, &path)?;
    tauri_plugin_opener::reveal_item_in_dir(&target)
        .map_err(|e| format!("the file manager could not be opened: {e}"))
}

/// Brings the login item and `desktop.json` back into agreement.
///
/// A choice made here is put back when an installer removed the entry: an
/// NSIS upgrade or any uninstall deletes the Windows Run value. Any other
/// difference is a choice made outside Uguisu — Task Manager's Startup tab, a
/// deleted autostart file — and is recorded rather than overridden.
fn reconcile(native: &Native) {
    let Some(item) = &native.login_item else {
        return;
    };
    let wanted = native.settings.lock().is_ok_and(|s| s.autostart);
    let enabled = match item.is_enabled() {
        Ok(enabled) => enabled,
        Err(error) => {
            tracing::warn!(%error, "the login item could not be read");
            return;
        }
    };
    if wanted && !enabled && removed_by_installer(item) {
        match item.enable() {
            Ok(()) => {
                tracing::info!("login item restored");
                return;
            }
            Err(error) => tracing::warn!(%error, "the login item could not be restored"),
        }
    }
    if enabled != wanted {
        tracing::info!(autostart = enabled, "login item changed outside uguisu");
        if let Err(error) = store(native, |settings| settings.autostart = enabled) {
            tracing::warn!(%error, "the login item's state was not recorded");
        }
    }
}

/// Whether the Windows Run value is gone.
///
/// Task Manager never deletes it — switching an item off there is recorded
/// beside it — so a missing value is an installer's doing, not the user's.
#[cfg(windows)]
fn removed_by_installer(item: &AutoLaunch) -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .and_then(|run| run.get_raw_value(item.get_app_name()))
        .is_err()
}

/// Packages never touch a Linux autostart file, so only the user removes one.
#[cfg(not(windows))]
fn removed_by_installer(_: &AutoLaunch) -> bool {
    false
}

/// Resolves the settings file and the archive root for this launch.
///
/// A debug build leaves the login item alone: it runs from a checkout, and
/// restoring the item would point the user's login at that build.
pub fn native(app: &tauri::App, media_root: PathBuf, media_pinned: bool) -> Native {
    let settings_path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(config::FILE);
    let settings = Desktop::load(&settings_path);
    let native = Native {
        media_root,
        media_pinned,
        settings_path,
        settings: Mutex::new(settings),
        login_item: login_item(app),
    };
    if !cfg!(debug_assertions) {
        reconcile(&native);
    }
    native
}
