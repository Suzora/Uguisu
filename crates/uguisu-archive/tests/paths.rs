//! Properties the archive layout must hold for *any* feed text.
//!
//! The rules under test are the ones `docs/SECURITY.md` §3.2 relies on: a
//! rendered path is always relative, always stays under the archive root,
//! never gains a separator from a value, and always renders the same way
//! for the same input.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use proptest::prelude::*;
use time::OffsetDateTime;
use uguisu_archive::path::{RelativePath, is_inside, resolve};
use uguisu_archive::sanitize::{self, MAX_PATH_CHARS, MAX_SEGMENT_BYTES, MAX_SEGMENT_CHARS};
use uguisu_archive::template::{Context, Template};
use uguisu_core::archive::PathProfile;
use uguisu_core::ids::{EpisodeId, PodcastId};

const DEFAULT_TEMPLATE: &str =
    "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}";

/// Templates a user could plausibly configure, covering every element kind.
const TEMPLATES: [&str; 6] = [
    DEFAULT_TEMPLATE,
    "{podcast.title}/{episode.title}.{extension}",
    "{podcast.title|slug}/[S{episode.season|pad:2}E{episode.number|pad:2} - ]{episode.title|truncate:40}.{extension}",
    "{podcast.title|ascii}/{episode.published:%Y-%m}/{episode.title|lower}.{extension}",
    "{podcast.id}/{episode.id}.{extension}",
    "{episode.identity} {episode.title|default:\"Untitled\"}.{extension}",
];

/// 2024-01-15 13:20 UTC.
const PUBLISHED: i64 = 1_705_325_400;

fn render(template: &str, title: &str, episode: &str, profile: PathProfile) -> RelativePath {
    let ctx = Context::synthetic(
        title,
        episode,
        "mp3",
        PodcastId::new(),
        EpisodeId::new(),
        Some(OffsetDateTime::from_unix_timestamp(PUBLISHED).unwrap()),
    );
    Template::parse(template)
        .unwrap()
        .render(&ctx, profile)
        .unwrap()
}

#[test]
fn every_profile_renders_the_same_path() {
    for profile in PathProfile::ALL {
        let p = render(
            DEFAULT_TEMPLATE,
            "Darknet Diaries",
            "The Pizza Problem",
            profile,
        );
        assert_eq!(
            p.as_str(),
            "Darknet Diaries/2024/2024-01-15 - The Pizza Problem.mp3",
            "{profile}"
        );
    }
    // The profiles differ only on what a character does to the name.
    assert_eq!(
        render(DEFAULT_TEMPLATE, "AC:DC?", "Ep*1", PathProfile::Posix).as_str(),
        "AC:DC?/2024/2024-01-15 - Ep*1.mp3"
    );
    assert_eq!(
        render(DEFAULT_TEMPLATE, "AC:DC?", "Ep*1", PathProfile::Portable).as_str(),
        "AC-DC-/2024/2024-01-15 - Ep-1.mp3"
    );
    assert_eq!(
        render(DEFAULT_TEMPLATE, "AC:DC?", "Ep*1", PathProfile::Windows).as_str(),
        "AC-DC-/2024/2024-01-15 - Ep-1.mp3"
    );
}

/// Strings chosen to break path handling: traversal, separators, absolute
/// forms, device names, control characters, confusables and sheer length.
fn nasty() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("../../etc/passwd".to_owned()),
        Just("..\\..\\windows\\system32".to_owned()),
        Just("/absolute/path".to_owned()),
        Just("C:\\Windows\\System32".to_owned()),
        Just("\\\\server\\share".to_owned()),
        Just("..".to_owned()),
        Just(".".to_owned()),
        Just("CON".to_owned()),
        Just("nul.mp3".to_owned()),
        Just("trailing.   ".to_owned()),
        Just("\u{202E}gnp.exe".to_owned()),
        Just("a\u{0000}b".to_owned()),
        Just("   ".to_owned()),
        Just(String::new()),
        Just("🎧".to_owned()),
        Just("Ｃ：／Ｗｉｎ".to_owned()),
        "\\PC{0,200}",
        "[a-zA-Z0-9 .\\-_/\\\\:*?\"<>|]{0,80}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever the feed says, the rendered path is relative, stays under
    /// the root, and has no component that is a traversal or a separator.
    #[test]
    fn a_rendered_path_never_leaves_the_archive(
        title in nasty(),
        episode in nasty(),
        template_index in 0usize..TEMPLATES.len(),
        profile_index in 0usize..PathProfile::ALL.len(),
    ) {
        let profile = PathProfile::ALL[profile_index];
        let rendered = render(TEMPLATES[template_index], &title, &episode, profile);

        for component in rendered.components() {
            prop_assert!(!component.is_empty());
            prop_assert!(component != "." && component != "..");
            prop_assert!(!component.contains('/'), "{component:?}");
            prop_assert!(!component.contains('\\'), "{component:?}");
            prop_assert!(!component.contains('\0'), "{component:?}");
            prop_assert!(component.chars().count() <= MAX_SEGMENT_CHARS);
            prop_assert!(component.len() <= MAX_SEGMENT_BYTES, "{component:?}");
        }
        prop_assert!(rendered.char_len() <= MAX_PATH_CHARS, "{rendered}");

        // Re-parsing the stored form must accept it unchanged, and joining
        // it onto a root must stay under that root.
        let reparsed = RelativePath::parse(rendered.as_str()).unwrap();
        prop_assert_eq!(reparsed.as_str(), rendered.as_str());
        let root = Path::new("/media/podcasts");
        let joined = resolve(root, &rendered).unwrap();
        prop_assert!(is_inside(root, &joined), "{}", joined.display());
        prop_assert!(!is_inside(Path::new("/media/podcasts-evil"), &joined));
    }

    /// The same episode always lands in the same place — the archive would
    /// be unusable if a second run chose a different name.
    #[test]
    fn rendering_survives_re_sanitization(
        title in nasty(),
        episode in nasty(),
        template_index in 0usize..TEMPLATES.len(),
        profile_index in 0usize..PathProfile::ALL.len(),
    ) {
        let profile = PathProfile::ALL[profile_index];
        let template = Template::parse(TEMPLATES[template_index]).unwrap();
        let ctx = Context::synthetic(
            &title,
            &episode,
            "mp3",
            PodcastId::new(),
            EpisodeId::new(),
            Some(OffsetDateTime::from_unix_timestamp(PUBLISHED).unwrap()),
        );
        let first = template.render(&ctx, profile).unwrap();
        let second = template.render(&ctx, profile).unwrap();
        prop_assert_eq!(first.as_str(), second.as_str());

        // Sanitizing an already-sanitized component changes nothing, so a
        // path read back from the database and re-checked does not drift.
        for component in first.components() {
            prop_assert_eq!(sanitize::segment(component, profile), component.to_owned());
        }
    }

    /// A value can never introduce a path component: a title containing a
    /// separator produces one segment, not two.
    #[test]
    fn a_value_cannot_add_a_component(title in nasty(), episode in nasty()) {
        let rendered = render(
            "{podcast.title}/{episode.title}.{extension}",
            &title,
            &episode,
            PathProfile::Portable,
        );
        prop_assert!(rendered.components().count() <= 2, "{rendered}");
    }
}
