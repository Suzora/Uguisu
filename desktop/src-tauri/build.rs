//! Generates the Tauri context: config, capabilities and platform resources.
//!
//! The app manifest has to name its own commands, or the capability that
//! grants `launch_credential` at runtime has no manifest to resolve against.
fn main() {
    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "launch_credential",
            "desktop_report",
            "choose_media_root",
            "set_notifications",
            "set_autostart",
            "notify",
            "reveal",
        ]));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build could not generate the app context: {error}");
    }
}
