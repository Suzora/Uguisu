//! What hostile feed text must never be able to do to the filesystem.
//!
//! These are the assertions `docs/SECURITY.md` §3.2 rests on, written
//! against a real directory so the check is "did it write outside the
//! root", not "did the string look wrong".

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // The extension under test is a literal lowercase `mp3`.
    clippy::case_sensitive_file_extension_comparisons
)]

use std::path::Path;

use time::OffsetDateTime;
use uguisu_archive::path::{self, PathError, RelativePath};
use uguisu_archive::sanitize;
use uguisu_archive::template::{Context, Template};
use uguisu_core::archive::PathProfile;
use uguisu_core::ids::{EpisodeId, PodcastId};

/// Titles that have been used to escape a directory, confuse a reader or
/// break a filesystem.
const HOSTILE: [&str; 26] = [
    "../../etc/passwd",
    "..\\..\\windows\\system32\\config\\sam",
    "....//....//etc/shadow",
    "/etc/passwd",
    "\\\\evil.example\\share\\payload",
    "C:\\Windows\\System32\\drivers\\etc\\hosts",
    "c:relative\\payload",
    "..",
    ".",
    "...",
    "./.",
    "a/../../../b",
    "CON",
    "PRN.mp3",
    "nul",
    "LPT1.tar.gz",
    "aux.",
    "trailing.",
    "trailing ",
    "with\0nul",
    "\u{202E}3pm.exe",
    "\u{200B}\u{200C}\u{FEFF}",
    "Ｃ：／Ｗｉｎｄｏｗｓ",
    "\u{0000}\u{0001}\u{001F}",
    "   ",
    "",
];

fn ctx<'a>(title: &'a str, episode: &'a str) -> Context<'a> {
    Context::synthetic(
        title,
        episode,
        "mp3",
        PodcastId::new(),
        EpisodeId::new(),
        Some(OffsetDateTime::from_unix_timestamp(1_705_325_400).unwrap()),
    )
}

#[test]
fn no_hostile_title_leaves_the_root() {
    let template = Template::parse(
        "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}",
    )
    .unwrap();
    let root = Path::new("/media/podcasts");
    for profile in PathProfile::ALL {
        for title in HOSTILE {
            for episode in HOSTILE {
                let rendered = template
                    .render(&ctx(title, episode), profile)
                    .unwrap_or_else(|e| panic!("{profile}/{title:?}/{episode:?}: {e}"));

                for component in rendered.components() {
                    assert!(component != ".." && component != ".", "{rendered}");
                    assert!(!component.is_empty(), "{rendered}");
                    assert!(!component.contains(['/', '\\', '\0']), "{rendered}");
                }
                let joined = path::resolve(root, &rendered).unwrap();
                assert!(path::is_inside(root, &joined), "{}", joined.display());
                // The prefix trap, from the other side.
                assert!(!path::is_inside(Path::new("/media"), Path::new("/media-x")));
                assert!(!path::is_inside(Path::new("/media/podcasts-evil"), &joined));
            }
        }
    }
}

#[test]
fn every_stored_escape_is_refused() {
    for raw in [
        "../secret",
        "..",
        "./x",
        "a/./b",
        "a/../b",
        "/absolute",
        "\\windows",
        "C:/x",
        "c:\\x",
        "\\\\server\\share",
        "",
        "   ",
        "/",
        "//",
    ] {
        assert!(
            RelativePath::parse(raw).is_err(),
            "`{raw}` must never parse as a stored archive path"
        );
    }
    assert_eq!(
        RelativePath::parse("a\0b").unwrap_err(),
        PathError::UnusableComponent("a\0b".into())
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_cannot_redirect_a_write() {
    use std::os::unix::fs;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("media");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(root.join("Show")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret"), b"not ours").unwrap();

    // A directory inside the archive that points out of it.
    fs::symlink(&outside, root.join("Escape")).unwrap();
    // And one that points at the root's own parent.
    fs::symlink(dir.path(), root.join("Up")).unwrap();

    for escaping in ["Escape/Ep.mp3", "Escape/secret", "Up/outside/secret"] {
        let relative = RelativePath::parse(escaping).unwrap();
        // The plain join looks fine — that is exactly why the check has to
        // resolve symlinks before anything is written.
        assert!(path::resolve(&root, &relative).is_ok());
        let err = path::resolve_checked(&root, &relative).unwrap_err();
        assert!(
            matches!(err, PathError::OutsideRoot(_)),
            "`{escaping}` must be refused: {err}"
        );
    }

    // A path whose parents do not exist yet is fine: nothing can hide
    // behind a directory that is not there.
    let fresh = RelativePath::parse("Brand New/2024/Ep.mp3").unwrap();
    assert!(path::resolve_checked(&root, &fresh).is_ok());
    // And an ordinary path inside the archive stays allowed.
    let inside = RelativePath::parse("Show/Ep.mp3").unwrap();
    assert!(path::resolve_checked(&root, &inside).is_ok());
    // Nothing was created by asking.
    assert!(!root.join("Brand New").exists());
}

#[test]
fn long_and_deep_templates_stay_bounded() {
    // Twenty levels of 400-character titles.
    let long = "L".repeat(400);
    let mut source = String::new();
    for _ in 0..20 {
        source.push_str("{podcast.title}/");
    }
    source.push_str("{episode.title}.{extension}");
    let template = Template::parse(&source).unwrap();

    for profile in PathProfile::ALL {
        let rendered = template.render(&ctx(&long, &long), profile).unwrap();
        assert!(
            rendered.char_len() <= sanitize::MAX_PATH_CHARS,
            "{} chars",
            rendered.char_len()
        );
        for component in rendered.components() {
            assert!(!component.is_empty());
            assert!(component.chars().count() <= sanitize::MAX_SEGMENT_CHARS);
            assert!(component.len() <= sanitize::MAX_SEGMENT_BYTES);
        }
        assert!(rendered.file_name().ends_with(".mp3"), "{rendered}");
        // Deterministic even after truncation.
        assert_eq!(
            rendered.as_str(),
            template
                .render(&ctx(&long, &long), profile)
                .unwrap()
                .as_str()
        );
    }
}

#[test]
fn a_template_is_never_absolute() {
    for source in [
        "/{episode.title}.{extension}",
        "//{episode.title}.{extension}",
        "{episode.title}.{extension}/",
        "///",
        "{podcast.title}//{episode.title}.{extension}",
    ] {
        let Ok(template) = Template::parse(source) else {
            continue;
        };
        let rendered = template
            .render(&ctx("Show", "Ep"), PathProfile::Portable)
            .unwrap();
        assert!(
            !rendered.as_str().starts_with('/'),
            "{source} -> {rendered}"
        );
        assert!(!rendered.as_str().contains("//"), "{source} -> {rendered}");
        assert!(RelativePath::parse(rendered.as_str()).is_ok());
    }
    // A template that renders nothing at all is refused at parse time.
    assert!(Template::parse("").is_err());
    assert!(Template::parse("/").is_err());
}

#[test]
fn an_unknown_variable_is_refused() {
    for source in [
        "{episode.secret}.{extension}",
        "{../etc/passwd}",
        "{episode.title|exec:rm}",
        "{episode.title",
        "episode.title}",
        "[{episode.title}",
        "{episode.title}]",
        "{}",
        "{episode.title|pad:x}",
        "{podcast.title:%Y}",
    ] {
        assert!(
            Template::parse(source).is_err(),
            "`{source}` must not parse"
        );
    }
}

// Sidecars, manifests, imports and artwork.
//
// Everything below is about input Uguisu did not write: a document in a
// directory it was pointed at, a manifest someone edited, a tree from
// another tool, bytes from a URL in a feed. The rule is the same
// throughout - the input decides nothing that could take Uguisu outside
// the root it was given, and it never gets the benefit of the doubt.

/// Paths that must never be accepted from a manifest, because a manifest
/// line decides which file gets read.
#[test]
fn a_poisoned_manifest_stays_inside() {
    use uguisu_archive::manifest;

    let digest = "a".repeat(64);
    for (path, why) in [
        ("/etc/passwd", "absolute"),
        ("../../etc/passwd", "traversal"),
        ("Show/../../../etc/passwd", "traversal in the middle"),
        ("C:\\Windows\\System32\\x.mp3", "a Windows drive"),
        ("\\\\server\\share\\x.mp3", "a UNC share"),
        ("./x.mp3", "a dot component"),
        (
            ".uguisu/manifests/x/manifest.sha256",
            "Uguisu's own bookkeeping",
        ),
        (
            "Show/.uguisu-tmp/job.part",
            "a download's scratch directory",
        ),
    ] {
        let line = format!("{digest}  {path}");
        assert!(
            manifest::parse_line(&line, 1).is_err(),
            "`{path}` ({why}) must be refused"
        );
    }

    // A digest that is not one is refused before the path is even looked
    // at, so a malformed file cannot be half-applied.
    for bad in ["", "z".repeat(64).as_str(), "abc", &"a".repeat(63)] {
        assert!(manifest::parse_line(&format!("{bad}  Show/a.mp3"), 1).is_err());
    }

    // Two hashes for one file is a contradiction, not a last-one-wins.
    let doubled = format!("{digest}  Show/a.mp3\n{}  Show/a.mp3\n", "b".repeat(64));
    assert!(manifest::parse(&doubled).is_err());
}

/// A sidecar is a document in a directory Uguisu was pointed at, so its
/// contents are input.
#[test]
fn a_hostile_sidecar_cannot_redirect() {
    use uguisu_archive::sidecar;

    // Too large to read at all: a scan of an untrusted archive must not
    // become an out-of-memory because one file has a `.json` name.
    let huge = vec![b' '; sidecar::MAX_SIDECAR_BYTES + 1];
    assert!(matches!(
        sidecar::parse(&huge).unwrap_err(),
        sidecar::SidecarError::TooLarge { .. }
    ));
    // Read from disk too, where only a byte past the cap is ever read.
    let root = tempfile::tempdir().unwrap();
    let on_disk = std::fs::File::create(root.path().join("big.mp3.json")).unwrap();
    on_disk.set_len(1 << 30).unwrap();
    let relative = uguisu_archive::path::RelativePath::parse("big.mp3.json").unwrap();
    let refused = sidecar::read(root.path(), &relative).unwrap_err();
    assert!(
        matches!(refused, sidecar::SidecarError::TooLarge { bytes, .. } if bytes == sidecar::MAX_SIDECAR_BYTES + 1),
        "{refused}"
    );

    // A schema from the future is refused by name, not half-read.
    let future = serde_json::json!({ "schema": 99 });
    assert!(matches!(
        sidecar::parse(future.to_string().as_bytes()).unwrap_err(),
        sidecar::SidecarError::UnsupportedSchema { found: 99, .. }
    ));

    // Nothing that is not JSON is guessed at.
    for body in [
        b"<?xml version=\"1.0\"?><x/>".as_slice(),
        b"#!/bin/sh\nrm -rf /".as_slice(),
        b"\x7FELF".as_slice(),
        b"".as_slice(),
    ] {
        assert!(sidecar::parse(body).is_err());
    }
}

/// The scan an import and a rebuild both run.
#[cfg(unix)]
#[test]
fn a_scan_never_leaves_its_tree() {
    use uguisu_archive::scan::{self, ScanError, ScanOptions};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(root.join("Show")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.mp3"), b"ID3\x04not yours").unwrap();
    std::fs::write(root.join("Show/real.mp3"), b"ID3\x04fine").unwrap();
    // Every shape of link that could take a walk out of the tree.
    std::os::unix::fs::symlink(&outside, root.join("Show/dir-link")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.mp3"), root.join("Show/file-link.mp3"))
        .unwrap();
    std::os::unix::fs::symlink("/", root.join("Show/root-link")).unwrap();

    let options = ScanOptions::default();
    let (found, errors): (Vec<_>, Vec<_>) = scan::walk(&root, &options).partition(Result::is_ok);
    let paths: Vec<String> = found
        .into_iter()
        .map(|f| f.unwrap().relative.as_str().to_owned())
        .collect();
    assert_eq!(
        paths,
        vec!["Show/real.mp3"],
        "nothing behind a link was walked"
    );
    assert_eq!(
        errors.len(),
        3,
        "and every link was reported rather than passed over in silence"
    );
    assert!(
        errors
            .into_iter()
            .all(|e| matches!(e.unwrap_err(), ScanError::Symlink(_))),
        "a link is reported as a link, not as some other failure"
    );
}

/// Bytes from a URL in a feed, destined to be embedded in files other
/// people's players parse.
#[test]
fn only_an_image_becomes_artwork() {
    use uguisu_archive::image;

    let png = {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(b"\x00\x00\x00\x0DIHDR");
        v
    };
    for (name, body) in [
        (
            "an ELF binary",
            b"\x7FELF\x02\x01\x01\x00\x00\x00\x00\x00".to_vec(),
        ),
        (
            "a Mach-O binary",
            b"\xCF\xFA\xED\xFE\x07\x00\x00\x01\x00\x00\x00\x00".to_vec(),
        ),
        (
            "a Windows executable",
            b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00".to_vec(),
        ),
        ("a shell script", b"#!/bin/sh\nrm -rf /\n#padding".to_vec()),
        (
            "an HTML error page",
            b"<!DOCTYPE html><html>403 Forbidden".to_vec(),
        ),
        (
            "an SVG with a script element",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>x()</script></svg>".to_vec(),
        ),
        (
            "a ZIP archive",
            b"PK\x03\x04\x14\x00\x00\x00\x08\x00\x00\x00".to_vec(),
        ),
        (
            "a gzip stream",
            b"\x1F\x8B\x08\x00\x00\x00\x00\x00\x00\x03pad".to_vec(),
        ),
    ] {
        for declared in [None, Some("image/png"), Some("image/jpeg"), Some("*/*")] {
            assert!(
                image::validate(declared, &body).is_err(),
                "{name} declared as {declared:?} must be refused"
            );
        }
    }
    // Real bytes described as something else are refused too: a server
    // contradicting itself is not resolved in its favour.
    assert!(image::validate(Some("text/html"), &png).is_err());
    assert!(image::validate(Some("image/jpeg"), &png).is_err());
    // Half a signature is not a signature.
    assert!(image::validate(Some("image/png"), &png[..4]).is_err());
}

/// Text from a foreign archive ends up in file names and in tags.
#[test]
fn hostile_foreign_text_stays_harmless() {
    use uguisu_archive::import::ImportFormat;
    use uguisu_archive::path::RelativePath;
    use uguisu_archive::scan::ScannedFile;
    use uguisu_archive::{layout, sanitize};
    use uguisu_core::archive::PathProfile;

    let reader = ImportFormat::Podgrab.reader();
    for hostile in [
        "../../etc/passwd",
        "..\\..\\windows\\system32",
        "a\u{202E}gnp.exe",
        "\u{0000}nul",
        "CON",
        ".uguisu",
        "-rf /",
        "$(rm -rf /)",
        "'; DROP TABLE episodes; --",
    ] {
        // Whatever it says, it becomes one segment that cannot separate,
        // traverse, name a device or shadow Uguisu's own directory.
        for profile in PathProfile::ALL {
            let segment = sanitize::segment(hostile, profile);
            assert!(!segment.contains('/'), "`{hostile}` -> `{segment}`");
            assert!(!segment.contains('\\'), "`{hostile}` -> `{segment}`");
            assert!(segment != "." && segment != "..", "`{hostile}`");
            assert!(
                !layout::is_control_name(&segment),
                "`{hostile}` -> `{segment}` would shadow .uguisu"
            );
            if !segment.is_empty() {
                assert!(
                    RelativePath::parse(&segment).is_ok(),
                    "`{hostile}` -> `{segment}` is not a usable path"
                );
            }
        }
        // And side metadata saying that changes nothing about where the
        // file is: the layout reads it for values, never for a location.
        let file = ScannedFile {
            relative: RelativePath::parse("Show/x.mp3").unwrap(),
            absolute: std::path::PathBuf::new(),
            size_bytes: 1,
            mtime_unix: None,
            head: b"ID3\x04............".to_vec(),
        };
        let mut candidate = reader.observe(&file);
        reader.refine(&mut candidate, hostile);
        assert_eq!(candidate.relative.as_str(), "Show/x.mp3");
    }
}
