//! Turning an archive-relative path from the page into a real file.
//!
//! Its own module so it can be tested without a desktop, and so the rule it
//! enforces is in one readable place: everything a page can say goes through
//! Uguisu's own parser and root check, and nothing here ever builds a command
//! line.

use std::path::{Path, PathBuf};

use uguisu_archive::path::{RelativePath, resolve_checked};

/// Resolves `path` inside `root`, or says why it will not.
///
/// `..`, an absolute path, a Windows drive or UNC prefix, and a symlink that
/// leaves the archive all fail here rather than reaching the file manager.
pub fn locate(root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = RelativePath::parse(path).map_err(|e| format!("that is not a path: {e}"))?;
    let target = resolve_checked(root, &relative)
        .map_err(|e| format!("that file is not in the archive: {e}"))?;
    if !target.is_file() {
        return Err("that file is not there".to_owned());
    }
    Ok(target)
}
