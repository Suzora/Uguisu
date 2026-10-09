//! Turning arbitrary feed text into one safe path segment (ADR 0009/0022).
//!
//! Everything here works on a **single** segment. A segment can never
//! become a separator, an absolute path, a parent reference or a reserved
//! device name, whatever the feed contains. The rules are deterministic,
//! so the same episode always produces the same file name — on Linux and
//! on Windows alike when the `portable` profile is used.

use uguisu_core::archive::PathProfile;
use unicode_normalization::UnicodeNormalization;

/// Longest segment Uguisu writes, in characters; [`MAX_SEGMENT_BYTES`]
/// bounds it in bytes as well.
pub const MAX_SEGMENT_CHARS: usize = 120;

/// Longest segment Uguisu writes, in UTF-8 bytes.
///
/// Linux file systems allow 255 **bytes** per name, and 120 characters of
/// CJK text are 360. A sidecar adds `.json` to a media file's name and its
/// scratch copy adds [`tmp_suffix`](crate::layout::tmp_suffix), at most 36
/// bytes, so 200 leaves room for both. NTFS counts UTF-16 units, of which a
/// name has at most as many as it has UTF-8 bytes.
pub const MAX_SEGMENT_BYTES: usize = 200;

/// Longest relative path Uguisu writes, in characters. Windows' classic
/// limit is 260 for the whole absolute path; this leaves room for a long
/// archive root.
pub const MAX_PATH_CHARS: usize = 200;

/// What every profile refuses, because it would change the meaning of the
/// path rather than only look odd.
const ALWAYS_UNSAFE: [char; 2] = ['/', '\\'];

/// Characters Windows refuses in a file name.
const WINDOWS_UNSAFE: [char; 7] = ['<', '>', ':', '"', '|', '?', '*'];

/// Windows device names, which are unusable with or without an extension.
/// The superscript digits count too, and NFC keeps them as they are.
const WINDOWS_RESERVED: [&str; 28] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// The character an unusable one is replaced with, so words stay readable.
const REPLACEMENT: char = '-';

fn strips_windows(profile: PathProfile) -> bool {
    matches!(profile, PathProfile::Windows | PathProfile::Portable)
}

/// Whether a character is a formatting or direction control that could
/// make a name display as something other than what it is.
fn is_hidden_control(c: char) -> bool {
    c.is_control()
        || matches!(c,
            // bidi overrides and isolates
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            // zero-width and other invisible joiners
            | '\u{200B}'..='\u{200D}' | '\u{FEFF}'
        )
}

/// Sanitizes one path segment under `profile`.
///
/// Returns an empty string when nothing usable is left; the caller drops
/// such a segment instead of writing an empty directory name.
#[must_use]
pub fn segment(raw: &str, profile: PathProfile) -> String {
    let windows = strips_windows(profile);
    let mut out = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for c in raw.nfc() {
        // A newline or a tab is a control character, but it separates words:
        // it becomes one space instead of gluing the words together.
        if !c.is_whitespace() && is_hidden_control(c) {
            continue;
        }
        let mapped = if ALWAYS_UNSAFE.contains(&c) || (windows && WINDOWS_UNSAFE.contains(&c)) {
            REPLACEMENT
        } else {
            c
        };
        // Collapse runs of whitespace so a title with newlines or padding
        // does not produce a name full of gaps.
        if mapped.is_whitespace() {
            if !last_was_space && !out.is_empty() {
                out.push(' ');
                last_was_space = true;
            }
            continue;
        }
        last_was_space = false;
        out.push(mapped);
    }

    let mut trimmed = trim_tail(out.trim(), windows).to_owned();
    // A name of only dots would be `.` or `..`: a parent reference, never a file.
    if trimmed.chars().all(|c| c == '.') {
        trimmed.clear();
    }
    // `C:` is a drive reference on Windows, and `C:foo` means "relative to
    // the current directory on drive C". POSIX allows a colon in a name, so
    // only this one shape is rewritten, and only when it could be read as a
    // drive.
    if looks_like_a_drive(&trimmed) {
        trimmed.replace_range(1..2, "-");
    }
    // `.uguisu` at the media root is Uguisu's own bookkeeping (manifests,
    // artwork, scratch). A podcast titled `.uguisu` must not render a
    // segment that shadows it, on any profile — the collision is a
    // property of the archive layout, not of the operating system. The
    // escape goes *inside* the name, after the dot, so a second pass over
    // `._uguisu` leaves it alone.
    if crate::layout::is_control_name(&trimmed) {
        let cut = trimmed.find('.').map_or(0, |i| i + 1);
        trimmed.insert(cut, '_');
    }
    if windows && is_reserved(&trimmed) {
        // Escape the device name itself, not the end of the string: `nul.mp3`
        // becomes `nul_.mp3`, which keeps the extension and — because the
        // escaped stem is no longer a device name — is stable when the
        // segment is sanitized again.
        let cut = trimmed.find('.').unwrap_or(trimmed.len());
        trimmed.insert(cut, '_');
    }
    // The cut goes last, and the tail is trimmed *again* after it: cutting
    // a long name can re-expose a trailing dot or space that the trim above
    // had removed from the end of the whole string. Windows would then
    // silently drop it and the file on disk would not be the path Uguisu
    // stored — and sanitizing the stored name a second time would return
    // something different, which is the property a path generator cannot
    // afford to lose. A property test found this with a 200-character
    // title whose first 120 characters happened to end in " .". A cut
    // that leaves only dots is erased for the same reason as above.
    let cut = trim_tail(
        &truncate(&trimmed, MAX_SEGMENT_CHARS, MAX_SEGMENT_BYTES),
        windows,
    )
    .to_owned();
    if cut.chars().all(|c| c == '.') {
        String::new()
    } else {
        cut
    }
}

/// Removes what a file system would drop from the end of a name: trailing
/// whitespace everywhere, and trailing dots too where Windows rules apply.
fn trim_tail(name: &str, windows: bool) -> &str {
    if windows {
        name.trim_end_matches(['.', ' ', '\t', '\n', '\r'])
    } else {
        name.trim_end()
    }
}

/// Whether a name begins with an ASCII letter followed by a colon, which
/// Windows reads as a drive reference rather than as part of the name.
fn looks_like_a_drive(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Whether a name is a Windows device name (`CON`, `LPT1`, `nul.mp3`, …).
#[must_use]
pub fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim();
    WINDOWS_RESERVED
        .iter()
        .any(|r| stem.eq_ignore_ascii_case(r))
}

/// Cuts a string to `max` characters (never inside one).
#[must_use]
pub fn truncate_chars(s: &str, max: usize) -> String {
    truncate(s, max, usize::MAX)
}

/// Cuts a string to at most `max_chars` characters and `max_bytes` UTF-8
/// bytes, never inside a character.
#[must_use]
pub fn truncate(s: &str, max_chars: usize, max_bytes: usize) -> String {
    let mut end = 0;
    for (count, (at, c)) in s.char_indices().enumerate() {
        if count == max_chars || at + c.len_utf8() > max_bytes {
            return s[..end].trim_end().to_owned();
        }
        end = at + c.len_utf8();
    }
    s.to_owned()
}

/// Builds `<stem>.<ext>` for one segment, keeping the extension whatever
/// happens to the stem: the extension decides how the file opens, so it is
/// never the part that gets cut.
#[must_use]
pub fn file_name(
    stem: &str,
    extension: &str,
    suffix: Option<&str>,
    profile: PathProfile,
) -> String {
    let windows = strips_windows(profile);
    let ext = segment(extension, profile);
    let suffix = suffix.map(|s| segment(s, profile)).unwrap_or_default();
    // Reserve room for `.ext` and the collision/truncation suffix.
    let separators = usize::from(!ext.is_empty()) + usize::from(!suffix.is_empty());
    let reserved = ext.chars().count() + suffix.chars().count() + separators;
    let room = MAX_SEGMENT_CHARS.saturating_sub(reserved).max(1);
    let room_bytes = MAX_SEGMENT_BYTES
        .saturating_sub(ext.len() + suffix.len() + separators)
        .max(1);
    let mut stem = trim_tail(
        &truncate(&segment(stem, profile), room, room_bytes),
        windows,
    )
    .to_owned();
    if stem.is_empty() {
        // Never produce a name that is only an extension: the caller's
        // fallback stem (the episode id) keeps the file addressable.
        stem.push_str("untitled");
    }
    if windows && is_reserved(&stem) {
        // `segment` already escaped a reserved stem, but cutting it to
        // `room` can produce a new one (`conversation` → `con`).
        if stem.chars().count() >= room || stem.len() >= room_bytes {
            stem = truncate(&stem, room.saturating_sub(1), room_bytes.saturating_sub(1));
        }
        stem.push('_');
    }
    let mut name = stem;
    if !suffix.is_empty() {
        name.push(' ');
        name.push_str(&suffix);
    }
    if !ext.is_empty() {
        name.push('.');
        name.push_str(&ext);
    }
    name
}

#[cfg(test)]
mod tests {
    // The extension under test is a literal lowercase `mp3`, so an exact
    // suffix assertion is the point.
    #![allow(clippy::case_sensitive_file_extension_comparisons)]

    use super::*;

    #[test]
    fn separators_and_parent_references_can_never_survive() {
        for profile in PathProfile::ALL {
            let s = segment("../../etc/passwd", profile);
            assert_eq!(s, "..-..-etc-passwd", "{profile}");
            assert!(!s.contains('/'), "{profile}: {s}");
            assert!(!s.contains('\\'), "{profile}: {s}");
            assert!(s != ".." && s != ".", "{profile}: {s}");
            assert_eq!(segment("..", profile), "");
            assert_eq!(segment(".", profile), "");
            assert_eq!(segment("...", profile), "");
            assert!(!segment("a\\b", profile).contains('\\'));
            assert!(!segment("a/b", profile).contains('/'));
        }
    }

    #[test]
    fn a_cut_to_dots_is_erased() {
        let dots = ".".repeat(130) + "x";
        for profile in PathProfile::ALL {
            let once = segment(&dots, profile);
            assert_eq!(segment(&once, profile), once, "{profile}");
            assert!(once.is_empty(), "{profile}: {once}");
        }
    }

    #[test]
    fn windows_rules_apply_to_windows_and_portable() {
        for profile in [PathProfile::Windows, PathProfile::Portable] {
            assert_eq!(segment("what? really*", profile), "what- really-");
            assert_eq!(segment("12:30 <live>", profile), "12-30 -live-");
            assert_eq!(segment("trailing.  ", profile), "trailing");
            assert_eq!(segment("CON", profile), "CON_");
            assert_eq!(segment("nul.mp3", profile), "nul_.mp3");
            assert_eq!(segment("com9", profile), "com9_");
            assert_eq!(segment("COM\u{b9}.mp3", profile), "COM\u{b9}_.mp3");
            assert_eq!(segment("lpt\u{b3}", profile), "lpt\u{b3}_");
            assert_eq!(
                segment(&segment("COM\u{b2}", profile), profile),
                "COM\u{b2}_"
            );
            assert_eq!(segment("console", profile), "console");
        }
        // POSIX keeps what POSIX allows.
        assert_eq!(
            segment("what? really*", PathProfile::Posix),
            "what? really*"
        );
        assert_eq!(segment("12:30 <live>", PathProfile::Posix), "12:30 <live>");
        assert_eq!(segment("CON", PathProfile::Posix), "CON");
        // A drive reference is never a name, on any profile.
        for profile in PathProfile::ALL {
            assert_eq!(segment("C:", profile), "C-", "{profile}");
            assert_eq!(segment("c:relative", profile), "c-relative", "{profile}");
        }
        assert_eq!(
            segment("AC:DC", PathProfile::Posix),
            "AC:DC",
            "a colon further in is a normal character on POSIX"
        );
    }

    #[test]
    fn invisible_and_control_characters_are_dropped() {
        let raw = "a\u{202E}b\u{0000}c\u{200B}d\ne\tf";
        let s = segment(raw, PathProfile::Portable);
        assert_eq!(s, "abcd e f");
        assert!(!s.chars().any(is_hidden_control));
        assert_eq!(segment("   ", PathProfile::Portable), "");
        assert_eq!(segment("\u{202E}", PathProfile::Portable), "");
    }

    #[test]
    fn a_cut_name_ends_legally() {
        // The trim that removes trailing dots and spaces used to run only
        // before the 120-character cut, so the cut could put one back.
        // Windows would then drop it silently, and the path Uguisu stored
        // would not be the file on disk. A property test found this shape
        // first; it is pinned here so it cannot come back by luck.
        let raw = format!("{} .{}", "x".repeat(MAX_SEGMENT_CHARS - 2), "y".repeat(50));
        for profile in PathProfile::ALL {
            let once = segment(&raw, profile);
            // Idempotence is the property that actually matters: a path read
            // back out of the database and checked again must not drift.
            assert_eq!(segment(&once, profile), once, "{profile:?}");
            assert!(!once.ends_with(' '), "{profile:?}: {once:?}");
            let name = file_name(&raw, "mp3", None, profile);
            assert!(name.ends_with(".mp3"), "{profile:?}: {name:?}");
            assert_eq!(segment(&name, profile), name, "{profile:?}: {name:?}");

            if strips_windows(profile) {
                // Where Windows rules apply the dot goes too, because the
                // file system would drop it and the stored path would then
                // name a file that does not exist.
                assert_eq!(once.chars().count(), MAX_SEGMENT_CHARS - 2, "{profile:?}");
                assert!(!once.ends_with('.'), "{profile:?}: {once:?}");
                assert!(!name.contains(" ."), "{profile:?}: {name:?}");
            } else {
                // POSIX keeps a trailing dot: it is a legal name there, and
                // dropping characters a system accepts loses information.
                assert!(once.ends_with(" ."), "{profile:?}: {once:?}");
            }
        }
    }

    #[test]
    fn multibyte_names_fit_in_bytes() {
        for c in ['\u{e9}', '\u{65e5}', '\u{1f399}'] {
            let long: String = std::iter::repeat_n(c, 300).collect();
            for profile in PathProfile::ALL {
                let once = segment(&long, profile);
                assert!(once.len() <= MAX_SEGMENT_BYTES, "{c}: {} bytes", once.len());
                assert_eq!(segment(&once, profile), once, "{c}: idempotent");
                let name = file_name(&long, "mp3", Some("01ARZ3ND"), profile);
                assert!(name.len() <= MAX_SEGMENT_BYTES, "{c}: {} bytes", name.len());
                assert!(name.ends_with(" 01ARZ3ND.mp3"), "{name}");
            }
        }
        assert_eq!(
            truncate("a\u{65e5}\u{65e5}", 10, 4),
            "a\u{65e5}",
            "never inside a character"
        );
    }

    #[test]
    fn unicode_is_normalized_and_kept() {
        // Composed and decomposed forms must produce the same segment, or
        // the same episode would get two different files.
        let composed = segment("Bär", PathProfile::Portable);
        let decomposed = segment("Ba\u{0308}r", PathProfile::Portable);
        assert_eq!(composed, decomposed);
        assert_eq!(segment("Καθημερινά", PathProfile::Portable), "Καθημερινά");
        assert_eq!(segment("日本語", PathProfile::Portable), "日本語");
        assert_eq!(segment("emoji 🎧 ok", PathProfile::Portable), "emoji 🎧 ok");
    }

    #[test]
    fn segments_and_names_stay_within_the_limits() {
        let long = "x".repeat(400);
        let s = segment(&long, PathProfile::Portable);
        assert_eq!(s.chars().count(), MAX_SEGMENT_CHARS);

        let name = file_name(&long, "mp3", None, PathProfile::Portable);
        assert!(name.chars().count() <= MAX_SEGMENT_CHARS, "{}", name.len());
        assert!(name.ends_with(".mp3"), "the extension always survives");

        let with_suffix = file_name(&long, "mp3", Some("[ab12cd]"), PathProfile::Portable);
        assert!(with_suffix.chars().count() <= MAX_SEGMENT_CHARS);
        assert!(with_suffix.ends_with(".mp3"));
        assert!(with_suffix.contains("ab12cd"), "{with_suffix}");
    }

    #[test]
    fn a_nameless_episode_still_gets_a_file() {
        assert_eq!(
            file_name("", "mp3", None, PathProfile::Portable),
            "untitled.mp3"
        );
        assert_eq!(
            file_name("...", "mp3", Some("x"), PathProfile::Portable),
            "untitled x.mp3"
        );
        assert_eq!(file_name("ok", "", None, PathProfile::Portable), "ok");
        assert_eq!(
            file_name("nul", "mp3", None, PathProfile::Portable),
            "nul_.mp3",
            "a reserved stem is escaped before the extension is added"
        );
        assert_eq!(
            file_name("nul", "mp3", None, PathProfile::Posix),
            "nul.mp3",
            "POSIX has no device names"
        );
    }
}
