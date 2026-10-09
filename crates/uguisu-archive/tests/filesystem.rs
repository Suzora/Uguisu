//! Hostile names created on the file system the tests run on: NTFS on
//! Windows, ext4 or tmpfs on Linux. `paths.rs` renders paths without
//! creating them; this creates each one, with the sidecar and the scratch
//! name Uguisu writes beside it, and reads the directory back, so a name the
//! renderer allows and the file system refuses or rewrites is caught
//! (RISKS R6).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use time::OffsetDateTime;
use uguisu_archive::layout::{sidecar_of, tmp_suffix};
use uguisu_archive::path::{RelativePath, is_inside, resolve};
use uguisu_archive::template::{Context, Template};
use uguisu_core::archive::PathProfile;
use uguisu_core::ids::{EpisodeId, PodcastId};

const TEMPLATE: &str =
    "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}";

/// Titles a feed can carry that a file system may refuse or rewrite.
fn hostile() -> Vec<String> {
    let mut titles: Vec<String> = [
        "CON",
        "con.mp3",
        "NUL",
        "Aux",
        "COM1",
        "LPT9",
        "COM\u{b9}",
        "LPT\u{b3}.txt",
        "a:b",
        "C:",
        "what?",
        "star*",
        "pipe|",
        "quote\"",
        "<angle>",
        "back\\slash",
        "fwd/slash",
        "trailing.",
        "trailing ",
        " leading",
        "..",
        ".",
        "...",
        ".uguisu",
        "\u{202e}bidi",
        "zero\u{200b}width",
        "nul\u{0}byte",
        "tab\tname",
        "line\nbreak",
        "full\u{ff1a}width",
        "\u{dc}n\u{ef}c\u{f6}d\u{e9} caf\u{e9}",
        "e\u{301}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "emoji \u{1f399}\u{fe0f}",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    // Long titles in one-, two-, three- and four-byte characters: the
    // renderer's limits must hold in the unit each file system counts.
    for c in ['x', '\u{e9}', '\u{65e5}', '\u{1f399}'] {
        titles.push(std::iter::repeat_n(c, 300).collect());
    }
    titles
}

fn profiles() -> Vec<PathProfile> {
    // A `posix` name may hold `:` or `?`, which Windows refuses by design;
    // CONFIGURATION.md says not to choose it there.
    PathProfile::ALL
        .into_iter()
        .filter(|p| !(cfg!(windows) && *p == PathProfile::Posix))
        .collect()
}

fn render(podcast: &str, episode: &str, profile: PathProfile) -> RelativePath {
    let ctx = Context::synthetic(
        podcast,
        episode,
        "mp3",
        PodcastId::new(),
        EpisodeId::new(),
        Some(OffsetDateTime::from_unix_timestamp(1_705_325_400).unwrap()),
    );
    Template::parse(TEMPLATE)
        .unwrap()
        .render(&ctx, profile)
        .unwrap()
}

/// Writes `relative` under `root`, then its sidecar and a scratch copy of
/// the sidecar, and checks that each name reads back exactly.
fn create(root: &Path, relative: &RelativePath) {
    let sidecar = sidecar_of(relative);
    let scratch = RelativePath::parse(&format!("{}{}", sidecar.as_str(), tmp_suffix())).unwrap();
    for path in [relative, &sidecar, &scratch] {
        let full = resolve(root, path).unwrap();
        std::fs::create_dir_all(full.parent().unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.as_str()));
        std::fs::write(&full, path.as_str()).unwrap_or_else(|e| {
            panic!("{} ({} bytes): {e}", path.as_str(), path.file_name().len())
        });
        assert_eq!(std::fs::read_to_string(&full).unwrap(), path.as_str());
        let listed: Vec<String> = std::fs::read_dir(full.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            listed.iter().any(|n| n == path.file_name()),
            "the file system stored {:?} as one of {listed:?}",
            path.file_name()
        );
        assert!(
            is_inside(root, &full.canonicalize().unwrap()),
            "{}",
            path.as_str()
        );
    }
}

#[test]
fn hostile_names_are_created_exactly() {
    for profile in profiles() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        for (i, title) in hostile().iter().enumerate() {
            // One directory per title: two titles may sanitize alike, and
            // collisions are the collision rule's business, not this test's.
            let own = root.join(i.to_string());
            std::fs::create_dir(&own).unwrap();
            create(&own, &render(title, title, profile));
        }
    }
}

#[test]
fn long_absolute_paths_are_created() {
    // Past Windows' classic 260-character limit for the whole path.
    let root = tempfile::tempdir().unwrap();
    let deep = root.path().canonicalize().unwrap().join("r".repeat(100));
    std::fs::create_dir(&deep).unwrap();
    let long: String = "x".repeat(300);
    for profile in profiles() {
        let relative = render(&long, &long, profile);
        let full = resolve(&deep, &relative).unwrap();
        assert!(full.as_os_str().len() > 260, "{}", full.display());
        create(&deep, &relative);
    }
}
