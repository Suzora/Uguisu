//! Deciding whether a fetched body really is an image Uguisu will store
//! (`docs/SECURITY.md` §3.1, ADR 0026).
//!
//! Artwork arrives from a URL in a feed, which is to say from a stranger.
//! Three things therefore decide the format, in this order of authority:
//! the **bytes**, then the declared media type, and never the extension.
//! A server that says `image/png` while sending an ELF binary is refused;
//! so is one that says `text/html` while sending a valid PNG, because a
//! server contradicting itself is not something to resolve in its favour.
//!
//! Only JPEG, PNG and WebP are accepted. That is what podcast feeds serve
//! and what every tag format can carry. SVG is deliberately absent: it is
//! a document format that can carry script and external references, and
//! embedding one into an audio file would hand a player a program rather
//! than a picture.
//!
//! Nothing here decodes an image. Uguisu stores and embeds artwork; it
//! never resizes or re-encodes it, so a decoder would be attack surface
//! bought for nothing (`docs/SECURITY.md` §3.10, minimal dependencies).

use uguisu_core::archive::{ArchiveErrorKind, ArtworkFormat};

/// Bytes needed before a format can be recognised. WebP needs twelve:
/// `RIFF`, a length, then `WEBP`.
pub const SNIFF_LEN: usize = 12;

/// Why some bytes are not artwork Uguisu will store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    /// The body is empty or shorter than any signature.
    #[error("body is {0} bytes, too short to be an image")]
    TooShort(usize),
    /// The bytes are not one of the accepted formats.
    #[error("body is not a JPEG, PNG or WebP image")]
    Unrecognised,
    /// The server described the body as something else.
    #[error("server declared `{declared}` but the bytes are {sniffed}")]
    Contradicted {
        /// What the response said.
        declared: String,
        /// What the bytes say.
        sniffed: ArtworkFormat,
    },
    /// The server described the body as something that is not an image at
    /// all.
    #[error("server declared `{0}`, which is not an image type")]
    NotAnImage(String),
}

impl ImageError {
    /// How the error is classified for the API and the CLI.
    #[must_use]
    pub const fn kind(&self) -> ArchiveErrorKind {
        ArchiveErrorKind::ArtworkInvalid
    }

    /// The short reason an event carries.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::TooShort(_) => "too_short",
            Self::Unrecognised => "unrecognised",
            Self::Contradicted { .. } => "declared_mismatch",
            Self::NotAnImage(_) => "not_an_image",
        }
    }
}

/// The eight-byte PNG signature.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Recognises an image from its first bytes. `None` means "not one of the
/// three", never "probably fine".
#[must_use]
pub fn sniff_image(head: &[u8]) -> Option<ArtworkFormat> {
    if head.len() < 3 {
        return None;
    }
    // JPEG: SOI followed by any marker.
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ArtworkFormat::Jpeg);
    }
    if head.starts_with(&PNG_MAGIC) {
        return Some(ArtworkFormat::Png);
    }
    // WebP is a RIFF container; the four bytes after the length say which.
    if head.len() >= SNIFF_LEN && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return Some(ArtworkFormat::Webp);
    }
    None
}

/// The format a media type names, if it names one this build stores.
#[must_use]
fn format_of_mime(mime: &str) -> Option<ArtworkFormat> {
    let base = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Some(ArtworkFormat::Jpeg),
        "image/png" | "image/apng" => Some(ArtworkFormat::Png),
        "image/webp" => Some(ArtworkFormat::Webp),
        _ => None,
    }
}

/// Whether a media type carries no claim at all about the content.
///
/// `application/octet-stream` is the standard way of saying "unknown
/// bytes", and a great many CDNs send it for perfectly ordinary images.
/// Treating it as a contradiction would refuse real artwork for no
/// security gain, so it counts as silence rather than as a claim.
fn is_no_claim(mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or(mime).trim();
    base.is_empty() || base.eq_ignore_ascii_case("application/octet-stream")
}

/// Decides whether a response body is artwork Uguisu will store.
///
/// The bytes have the final say on *which* format it is; a declared type
/// can only contradict them, never override them. `declared` is the
/// response's `Content-Type`, when it sent one.
pub fn validate(declared: Option<&str>, bytes: &[u8]) -> Result<ArtworkFormat, ImageError> {
    if bytes.len() < SNIFF_LEN {
        return Err(ImageError::TooShort(bytes.len()));
    }
    let sniffed = sniff_image(bytes).ok_or(ImageError::Unrecognised)?;
    let Some(declared) = declared.filter(|d| !is_no_claim(d)) else {
        return Ok(sniffed);
    };
    match format_of_mime(declared) {
        Some(named) if named == sniffed => Ok(sniffed),
        Some(_) => Err(ImageError::Contradicted {
            declared: declared.to_owned(),
            sniffed,
        }),
        None => Err(ImageError::NotAnImage(declared.to_owned())),
    }
}

/// The file extension for a format, without the dot.
#[must_use]
pub const fn extension_for(format: ArtworkFormat) -> &'static str {
    format.extension()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn jpeg() -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend_from_slice(b"\x00\x10JFIF\x00\x01\x02\x00");
        v
    }

    fn png() -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.extend_from_slice(b"\x00\x00\x00\x0DIHDR");
        v
    }

    fn webp() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&[0x20, 0, 0, 0]);
        v.extend_from_slice(b"WEBPVP8 ");
        v
    }

    #[test]
    fn the_bytes_decide_the_format() {
        assert_eq!(sniff_image(&jpeg()), Some(ArtworkFormat::Jpeg));
        assert_eq!(sniff_image(&png()), Some(ArtworkFormat::Png));
        assert_eq!(sniff_image(&webp()), Some(ArtworkFormat::Webp));
        assert_eq!(validate(None, &png()).unwrap(), ArtworkFormat::Png);
        assert_eq!(
            validate(Some("image/png"), &png()).unwrap(),
            ArtworkFormat::Png
        );
        assert_eq!(
            validate(Some("image/jpeg; charset=binary"), &jpeg()).unwrap(),
            ArtworkFormat::Jpeg
        );
        assert_eq!(extension_for(ArtworkFormat::Jpeg), "jpg");
    }

    #[test]
    fn only_the_three_formats_get_through() {
        // Each of these has been served as podcast artwork by something at
        // some point, and each would be a different kind of bad day.
        for (name, body) in [
            (
                "an ELF binary",
                b"\x7FELF\x02\x01\x01\x00\x00\x00\x00\x00".to_vec(),
            ),
            ("a shell script", b"#!/bin/sh\necho hi\n".to_vec()),
            ("an HTML error page", b"<!DOCTYPE html><html>404".to_vec()),
            (
                "an SVG document",
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script/></svg>".to_vec(),
            ),
            ("a GIF", b"GIF89a\x01\x00\x01\x00\x00\x00".to_vec()),
            (
                "a ZIP archive",
                b"PK\x03\x04\x14\x00\x00\x00\x08\x00\x00\x00".to_vec(),
            ),
            (
                "a RIFF that is not WebP",
                b"RIFF\x20\x00\x00\x00WAVEfmt ".to_vec(),
            ),
        ] {
            assert_eq!(sniff_image(&body), None, "{name} is not an image");
            assert_eq!(
                validate(Some("image/png"), &body),
                Err(ImageError::Unrecognised),
                "{name} advertised as a PNG is still not a PNG"
            );
        }
        // SVG is refused even when it is honestly declared: Uguisu stores
        // pictures, not documents that can carry script.
        assert_eq!(
            validate(Some("image/svg+xml"), b"<svg xmlns=\"x\"></svg>"),
            Err(ImageError::Unrecognised)
        );
    }

    #[test]
    fn a_self_contradicting_server_is_refused() {
        assert_eq!(
            validate(Some("image/jpeg"), &png()),
            Err(ImageError::Contradicted {
                declared: "image/jpeg".to_owned(),
                sniffed: ArtworkFormat::Png,
            })
        );
        // Real bytes, but the server says it is sending a web page: that is
        // not a disagreement to resolve in the server's favour.
        assert_eq!(
            validate(Some("text/html"), &png()),
            Err(ImageError::NotAnImage("text/html".to_owned()))
        );
        assert_eq!(
            validate(Some("text/html"), &png()).unwrap_err().reason(),
            "not_an_image"
        );
    }

    #[test]
    fn unknown_bytes_is_silence_not_a_contradiction() {
        // A great many CDNs send this for ordinary images; refusing it
        // would cost real artwork and buy nothing.
        assert_eq!(
            validate(Some("application/octet-stream"), &jpeg()).unwrap(),
            ArtworkFormat::Jpeg
        );
        assert_eq!(validate(Some("  "), &jpeg()).unwrap(), ArtworkFormat::Jpeg);
        // ...but it still does not make non-image bytes acceptable.
        assert_eq!(
            validate(Some("application/octet-stream"), b"not an image at all"),
            Err(ImageError::Unrecognised)
        );
    }

    #[test]
    fn a_truncated_header_is_refused() {
        assert_eq!(
            validate(Some("image/png"), b""),
            Err(ImageError::TooShort(0))
        );
        assert_eq!(
            validate(Some("image/png"), &PNG_MAGIC[..4]),
            Err(ImageError::TooShort(4)),
            "half a signature is not a signature"
        );
        assert_eq!(sniff_image(b"RI"), None);
        // Long enough to sniff, but the RIFF form is undecided.
        assert_eq!(sniff_image(b"RIFF\x20\x00\x00\x00WEB"), None);
    }
}
