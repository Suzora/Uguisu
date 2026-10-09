//! What `desktop.json` gives back.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../src/config.rs"]
mod config;

#[test]
fn saved_folder_loads_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE);
    let folder = dir.path().join("Podcäst Ablage");
    let settings = config::Desktop {
        media_root: Some(folder.clone()),
        notifications: false,
        autostart: true,
    };
    settings.save(&path).unwrap();

    let loaded = config::Desktop::load(&path);
    assert_eq!(loaded.media_root, Some(folder));
    assert!(!loaded.notifications);
    assert!(loaded.autostart);
}

#[cfg(windows)]
#[test]
fn verbatim_folder_loads_plain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE);
    std::fs::write(
        &path,
        r#"{"media_root": "\\\\?\\C:\\Users\\Example\\Podcäst Ablage"}"#,
    )
    .unwrap();
    assert_eq!(
        config::Desktop::load(&path).media_root,
        Some(std::path::PathBuf::from(r"C:\Users\Example\Podcäst Ablage"))
    );
}

#[test]
fn a_missing_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("Externe Platte").join("Podcasts");
    assert!(config::usable_folder(&gone).is_err());
    assert!(!gone.exists(), "the check never creates the folder");
}

#[test]
fn a_file_is_not_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("archive");
    std::fs::write(&file, b"not a folder").unwrap();
    assert!(config::usable_folder(&file).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"not a folder");
}

#[test]
fn a_usable_folder_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Podcäst Ablage");
    std::fs::create_dir(&folder).unwrap();
    let resolved = config::usable_folder(&folder).unwrap();
    assert_eq!(resolved, dunce::canonicalize(&folder).unwrap());
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
}

#[test]
fn the_probe_spares_existing_files() {
    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join(".uguisu-write-test");
    std::fs::write(&mine, b"meine Notizen").unwrap();
    config::usable_folder(dir.path()).unwrap();
    assert_eq!(std::fs::read(&mine).unwrap(), b"meine Notizen");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
