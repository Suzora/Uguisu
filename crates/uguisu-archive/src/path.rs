//! Resolving a relative archive path under the media root, and proving it
//! stays there (ADR 0022, `docs/SECURITY.md` §3.2).
//!
//! Containment is decided on **path components**, never by comparing
//! strings: `"/media/podcasts-evil"` starts with `"/media/podcasts"` as a
//! string but is a different directory. Symlinks are resolved for the part
//! of the path that exists, so a link planted inside the archive cannot
//! point the archive out of itself.

use std::path::{Component, Path, PathBuf};

use uguisu_core::archive::ArchiveErrorKind;

/// Why a path cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The path has no usable component.
    #[error("path is empty")]
    Empty,
    /// The path is absolute, or names a drive or a UNC share.
    #[error("path must be relative, got `{0}`")]
    NotRelative(String),
    /// The path contains `.` or `..`.
    #[error("path must not contain `.` or `..`, got `{0}`")]
    Traversal(String),
    /// A component contains a character that cannot be part of a name.
    #[error("path component `{0}` is not usable")]
    UnusableComponent(String),
    /// The resolved path is outside the archive root.
    #[error("resolved path leaves the archive root: `{0}`")]
    OutsideRoot(String),
}

impl PathError {
    /// How the error is classified for the API and the CLI.
    #[must_use]
    pub const fn kind(&self) -> ArchiveErrorKind {
        ArchiveErrorKind::PathInvalid
    }
}

/// A relative archive path that was checked: no traversal, no separators
/// inside a component, no absolute or drive-qualified form.
///
/// Stored form is always POSIX (`/`), whatever the host system uses, so
/// the database is portable between them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelativePath(String);

impl RelativePath {
    /// Checks a relative path and keeps its POSIX form.
    pub fn parse(raw: &str) -> Result<Self, PathError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        // Reject Windows drive letters and UNC shares before anything else:
        // on Unix they are not "absolute" but they are never a file name we
        // wrote, and on Windows they escape the root.
        let looks_absolute = raw.starts_with('/')
            || raw.starts_with('\\')
            || raw
                .as_bytes()
                .get(1)
                .is_some_and(|b| *b == b':' && raw.as_bytes()[0].is_ascii_alphabetic());
        if looks_absolute {
            return Err(PathError::NotRelative(raw.to_owned()));
        }
        let mut parts = Vec::new();
        for part in raw.split(['/', '\\']) {
            if part.is_empty() {
                // `a//b` and a trailing slash are tolerated, not written.
                continue;
            }
            if part == "." || part == ".." {
                return Err(PathError::Traversal(raw.to_owned()));
            }
            if part.contains('\0') {
                return Err(PathError::UnusableComponent(part.to_owned()));
            }
            parts.push(part);
        }
        if parts.is_empty() {
            return Err(PathError::Empty);
        }
        Ok(Self(parts.join("/")))
    }

    /// Builds a path this crate produced itself, without re-checking it.
    ///
    /// Only for paths assembled from fixed literals and identifiers (a
    /// manifest, an artwork file), where [`Self::parse`] cannot fail and
    /// returning a `Result` would push a meaningless error path onto every
    /// caller. The debug assertion catches a mistake in the literals.
    pub(crate) fn from_trusted(value: String) -> Self {
        debug_assert!(
            Self::parse(&value).as_ref().map(Self::as_str) == Ok(value.as_str()),
            "from_trusted was given a path parse would change or refuse: `{value}`"
        );
        Self(value)
    }

    /// Builds a path from already-sanitized components, dropping empty ones.
    pub fn from_components<I, S>(components: I) -> Result<Self, PathError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let joined = components
            .into_iter()
            .map(|c| c.as_ref().to_owned())
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        Self::parse(&joined)
    }

    /// The stored POSIX form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its components.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// The file name (the last component).
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The directory part, if the path has one.
    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        self.0.rsplit_once('/').map(|(dir, _)| dir)
    }

    /// Length in characters, for the total-path limit.
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.0.chars().count()
    }
}

impl std::fmt::Display for RelativePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<RelativePath> for String {
    fn from(p: RelativePath) -> Self {
        p.0
    }
}

/// Joins a checked relative path onto the archive root.
///
/// The result is guaranteed to be `root` plus normal components: the input
/// cannot introduce `..`, an absolute path or a drive.
pub fn resolve(root: &Path, relative: &RelativePath) -> Result<PathBuf, PathError> {
    let mut out = root.to_path_buf();
    for component in relative.components() {
        out.push(component);
    }
    // Belt and braces: whatever the input did, the joined path must still
    // consist of the root plus plain names.
    if !is_inside(root, &out) {
        return Err(PathError::OutsideRoot(out.display().to_string()));
    }
    Ok(out)
}

/// Whether `candidate` lies under `root`, compared component by component.
#[must_use]
pub fn is_inside(root: &Path, candidate: &Path) -> bool {
    let root_components: Vec<Component<'_>> = root.components().collect();
    let candidate_components: Vec<Component<'_>> = candidate.components().collect();
    if candidate_components.len() < root_components.len() {
        return false;
    }
    if candidate_components
        .iter()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return false;
    }
    root_components
        .iter()
        .zip(&candidate_components)
        .all(|(a, b)| a == b)
}

/// Resolves the path and proves it stays under the root **after** symlinks.
///
/// The deepest existing ancestor is canonicalized, so a symlinked
/// directory inside the archive that points elsewhere is caught before
/// anything is written or read. A path whose ancestors do not exist yet is
/// accepted: nothing can be behind a link that is not there.
pub fn resolve_checked(root: &Path, relative: &RelativePath) -> Result<PathBuf, PathError> {
    let joined = resolve(root, relative)?;
    let real_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // Walk up to the deepest component that exists.
    let mut existing = joined.as_path();
    loop {
        match existing.canonicalize() {
            Ok(real) => {
                if !is_inside(&real_root, &real) {
                    return Err(PathError::OutsideRoot(real.display().to_string()));
                }
                break;
            }
            Err(_) => match existing.parent() {
                Some(parent) if parent.starts_with(root) || parent == root => existing = parent,
                _ => break,
            },
        }
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn traversal_and_absolute_forms_are_refused() {
        for raw in [
            "../secret",
            "a/../../b",
            "./a",
            "a/./b",
            "..",
            "/etc/passwd",
            "\\windows\\system32",
            "C:/Windows",
            "c:\\windows",
            "\\\\server\\share\\file.mp3",
            "",
            "   ",
        ] {
            assert!(RelativePath::parse(raw).is_err(), "`{raw}` must be refused");
        }
        assert_eq!(
            RelativePath::parse("a\0b").unwrap_err(),
            PathError::UnusableComponent("a\0b".into())
        );
    }

    #[test]
    fn usable_paths_keep_their_posix_form() {
        let p = RelativePath::parse("Show/2024/Ep.mp3").unwrap();
        assert_eq!(p.as_str(), "Show/2024/Ep.mp3");
        assert_eq!(p.file_name(), "Ep.mp3");
        assert_eq!(p.parent(), Some("Show/2024"));
        assert_eq!(p.components().count(), 3);
        // Windows separators and doubled slashes are normalized, not refused.
        assert_eq!(
            RelativePath::parse("Show\\2024\\Ep.mp3").unwrap().as_str(),
            "Show/2024/Ep.mp3"
        );
        assert_eq!(
            RelativePath::parse("Show//2024/Ep.mp3/").unwrap().as_str(),
            "Show/2024/Ep.mp3"
        );
        let single = RelativePath::parse("Ep.mp3").unwrap();
        assert_eq!(single.parent(), None);
        assert_eq!(single.file_name(), "Ep.mp3");
    }

    #[test]
    fn empty_components_disappear() {
        let p = RelativePath::from_components(["Show", "", "2024", "Ep.mp3"]).unwrap();
        assert_eq!(p.as_str(), "Show/2024/Ep.mp3");
        assert!(RelativePath::from_components(["", "  "]).is_err());
    }

    #[test]
    fn containment_is_decided_on_components_not_strings() {
        let root = Path::new("/media/podcasts");
        assert!(is_inside(root, Path::new("/media/podcasts/a/b.mp3")));
        assert!(is_inside(root, root));
        // The classic prefix trap.
        assert!(!is_inside(root, Path::new("/media/podcasts-evil/b.mp3")));
        assert!(!is_inside(root, Path::new("/media")));
        assert!(!is_inside(root, Path::new("/other/podcasts/b.mp3")));
        assert!(!is_inside(root, Path::new("/media/podcasts/../escape")));
    }

    #[test]
    fn resolve_joins_under_the_root() {
        let root = Path::new("/media/podcasts");
        let p = RelativePath::parse("Show/Ep.mp3").unwrap();
        assert_eq!(
            resolve(root, &p).unwrap(),
            Path::new("/media/podcasts/Show/Ep.mp3")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_escaping_symlink_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("media");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(root.join("Show")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

        let inside = RelativePath::parse("Show/Ep.mp3").unwrap();
        assert!(resolve_checked(&root, &inside).is_ok());

        let escaping = RelativePath::parse("escape/Ep.mp3").unwrap();
        let err = resolve_checked(&root, &escaping).unwrap_err();
        assert!(
            matches!(err, PathError::OutsideRoot(_)),
            "a symlinked directory must not lead out: {err}"
        );
    }
}
