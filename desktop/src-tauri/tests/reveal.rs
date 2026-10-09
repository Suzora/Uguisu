//! What "Reveal" will and will not open (brief §20, §42).

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../src/reveal.rs"]
mod reveal;

/// Builds an archive with the awkward names a real one contains.
fn archive() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for relative in [
        "Plain/episode.mp3",
        "With Spaces/an episode, live.mp3",
        "Ünïcode Podcast/Épisode ①.mp3",
        "Deep/Nested/Folder/Tree/episode.mp3",
        "Shell $(rm -rf ~) `id` ; drop/e&p^e.mp3",
    ] {
        let target = root.join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"audio").unwrap();
    }
    std::fs::write(root.parent().unwrap().join("outside.mp3"), b"no").unwrap();
    dir
}

#[test]
fn ordinary_and_awkward_names_resolve() {
    let dir = archive();
    for relative in [
        "Plain/episode.mp3",
        "With Spaces/an episode, live.mp3",
        "Ünïcode Podcast/Épisode ①.mp3",
        "Deep/Nested/Folder/Tree/episode.mp3",
        "Shell $(rm -rf ~) `id` ; drop/e&p^e.mp3",
    ] {
        let found = reveal::locate(dir.path(), relative);
        assert!(found.is_ok(), "{relative} should resolve: {found:?}");
        assert!(found.unwrap().starts_with(dir.path()));
    }
}

#[test]
fn nothing_outside_the_archive_resolves() {
    let dir = archive();
    for refused in [
        "../outside.mp3",
        "Plain/../../outside.mp3",
        "/etc/passwd",
        "C:\\Windows\\System32\\drivers\\etc\\hosts",
        "\\\\server\\share\\file.mp3",
        "",
        "   ",
        "Plain",
        "Plain/missing.mp3",
    ] {
        assert!(
            reveal::locate(dir.path(), refused).is_err(),
            "{refused:?} must be refused"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_archive_resolves_to_nothing() {
    let dir = archive();
    let escape = dir.path().join("Plain/escape.mp3");
    std::os::unix::fs::symlink(std::path::Path::new("/etc/hostname"), &escape).unwrap();
    assert!(
        reveal::locate(dir.path(), "Plain/escape.mp3").is_err(),
        "a symlink pointing out of the archive must not be revealed"
    );
}
