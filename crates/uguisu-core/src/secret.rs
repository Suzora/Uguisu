//! A wrapper for credentials that must never appear in logs or debug output.

use std::fmt;

use subtle::ConstantTimeEq;

/// A value that is redacted in `Debug` and `Display` output.
///
/// Use [`Secret::expose`] at the single place where the value is needed
/// (for example when signing a request). `Secret` deliberately does not
/// implement `Deref`, `Serialize` or `Display` for the inner value.
///
/// The derived `PartialEq` compares structurally and returns as soon as the
/// two values differ, so it must never decide whether a *presented*
/// credential is the right one; use [`Secret::ct_eq`] there.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wraps a value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Returns the wrapped value. Call this only where the secret is used.
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Consumes the wrapper and returns the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: AsRef<[u8]>> Secret<T> {
    /// Whether `presented` is the secret, in time independent of where the
    /// two first differ (`docs/SECURITY.md` §3.5).
    #[must_use]
    pub fn ct_eq(&self, presented: &[u8]) -> bool {
        let mine = self.0.as_ref();
        // `ConstantTimeEq` on slices requires equal lengths, and a length is
        // not the secret here: every credential Uguisu compares this way is a
        // fixed-width hex string.
        if mine.len() != presented.len() {
            return false;
        }
        mine.ct_eq(presented).into()
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<T> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn debug_and_display_are_redacted() {
        let s = Secret::new(String::from("hunter2"));
        assert_eq!(format!("{s:?}"), "Secret([redacted])");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn ct_eq_matches_and_rejects() {
        let s = Secret::new(String::from("0f1e2d3c"));
        assert!(s.ct_eq(b"0f1e2d3c"));
        assert!(!s.ct_eq(b"0f1e2d3d"));
        assert!(!s.ct_eq(b"ff1e2d3c"));
    }

    #[test]
    fn ct_eq_rejects_length_change() {
        let s = Secret::new(String::from("0f1e2d3c"));
        assert!(!s.ct_eq(b"0f1e2d3"));
        assert!(!s.ct_eq(b"0f1e2d3c0"));
        assert!(!s.ct_eq(b""));
    }
}
