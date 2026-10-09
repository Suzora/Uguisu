//! A tray icon with the two entries a tray is for.
//!
//! Deliberately not a second queue or scheduler view: the window already has
//! those, and duplicating them would be two places to keep right.

use std::panic::AssertUnwindSafe;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

/// Builds the tray, or says why it could not.
///
/// Failure is not fatal. Tray support is a property of the desktop
/// environment, not of Uguisu, and a session without one should still get a
/// window.
pub fn install(app: &AppHandle) {
    // On Linux the indicator library is loaded on first use, and the crate
    // that loads it panics instead of returning an error when the system has
    // neither libayatana-appindicator3 nor libappindicator3 — a Flatpak
    // runtime, a minimal install. Unwinding stops here, so that is a missing
    // tray rather than a process that exits with the engine still open.
    match std::panic::catch_unwind(AssertUnwindSafe(|| build(app))) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::info!(%error, "no tray icon on this desktop"),
        Err(_) => tracing::info!("no tray icon: no appindicator library on this system"),
    }
}

fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Uguisu", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::with_id("uguisu")
        .icon(
            app.default_window_icon().cloned().ok_or_else(|| {
                tauri::Error::AssetNotFound("the window icon is missing".to_owned())
            })?,
        )
        .tooltip("Uguisu")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => reveal_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn reveal_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
