//! Serving bytes that live under the media root: archived media and artwork.
//!
//! One helper for both routes. It resolves the stored relative path through
//! `uguisu_archive::path`, refuses anything that is not a regular file, and
//! answers `Range` so a browser's `<audio>` element can seek.
//!
//! No caller ever names a path: both routes take an id, read the path from
//! the record and hand it here. An error never carries the resolved
//! filesystem path, only the relative one the API already publishes.

use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uguisu_archive::{RelativePath, resolve_checked};
use uguisu_core::archive::ArchiveErrorKind;
use uguisu_core::error::UguisuError;

/// What a `Range` header asks for, already clamped to the file length.
#[derive(Debug, PartialEq, Eq)]
enum Ranged {
    /// Serve the whole file: no `Range`, a syntax Uguisu does not answer, or
    /// an `If-Range` that no longer matches.
    Full,
    /// Serve `start..=end` inclusive.
    Partial { start: u64, end: u64 },
    /// The range names no byte of this file.
    Unsatisfiable,
}

/// Parses one byte range against a known length. Multiple ranges, a unit
/// other than `bytes` and any malformed value come back as [`Ranged::Full`]:
/// RFC 9110 lets a server ignore `Range`, and a partial response is only
/// ever an optimisation here.
fn range_of(raw: &str, size: u64) -> Ranged {
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        return Ranged::Full;
    };
    if spec.contains(',') {
        return Ranged::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Ranged::Full;
    };
    if size == 0 {
        return Ranged::Unsatisfiable;
    }
    let end_of_file = size - 1;
    match (first.trim(), last.trim()) {
        ("", suffix) => match suffix.parse::<u64>() {
            Ok(0) => Ranged::Unsatisfiable,
            Ok(n) => Ranged::Partial {
                start: size.saturating_sub(n),
                end: end_of_file,
            },
            Err(_) => Ranged::Full,
        },
        (start, "") => match start.parse::<u64>() {
            Ok(s) if s > end_of_file => Ranged::Unsatisfiable,
            Ok(s) => Ranged::Partial {
                start: s,
                end: end_of_file,
            },
            Err(_) => Ranged::Full,
        },
        (start, last) => match (start.parse::<u64>(), last.parse::<u64>()) {
            (Ok(s), Ok(e)) if s > end_of_file || s > e => Ranged::Unsatisfiable,
            (Ok(s), Ok(e)) => Ranged::Partial {
                start: s,
                end: e.min(end_of_file),
            },
            _ => Ranged::Full,
        },
    }
}

fn archive_error(kind: ArchiveErrorKind, detail: impl Into<String>) -> UguisuError {
    UguisuError::Archive {
        kind,
        detail: detail.into(),
    }
}

/// Serves the file `relative` names under `root`.
///
/// `etag` is an opaque strong validator — both callers pass the recorded
/// SHA-256 of the bytes, which changes whenever the file does.
pub(crate) async fn serve_file(
    root: &Path,
    relative: &RelativePath,
    content_type: &str,
    etag: &str,
    request: &HeaderMap,
    cache_control: &'static str,
) -> Result<Response, UguisuError> {
    let resolved = resolve_checked(root, relative).map_err(|e| {
        // The reason can name an absolute path outside the archive. That
        // belongs in the log, never in a response a browser receives.
        tracing::warn!(relative = relative.as_str(), error = %e, "path leaves the archive");
        archive_error(
            e.kind(),
            format!("{} does not resolve inside the archive", relative.as_str()),
        )
    })?;

    // `symlink_metadata` does not follow the final component: a symlink
    // where the artifact should be is not the artifact, whatever it points
    // at, so it is never opened.
    let meta = tokio::fs::symlink_metadata(&resolved)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => archive_error(
                ArchiveErrorKind::ArchiveMissing,
                format!("{} is not on disk", relative.as_str()),
            ),
            _ => archive_error(
                ArchiveErrorKind::VerificationIo,
                format!("{}: {}", relative.as_str(), e.kind()),
            ),
        })?;
    if !meta.is_file() {
        return Err(archive_error(
            ArchiveErrorKind::ArchiveInvalid,
            format!("{} is not a regular file", relative.as_str()),
        ));
    }
    let size = meta.len();
    let tag = format!("\"{etag}\"");

    if header_eq(request, &header::IF_NONE_MATCH, &tag) {
        return Ok(not_modified(&tag, cache_control));
    }
    let ranged = match request.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        // An `If-Range` that no longer matches means the client's copy is
        // stale: it must be given the whole file, not a splice of two versions.
        Some(_) if !if_range_ok(request, &tag) => Ranged::Full,
        Some(raw) => range_of(raw, size),
        None => Ranged::Full,
    };
    let (start, end) = match ranged {
        Ranged::Full => (0, size.saturating_sub(1)),
        Ranged::Partial { start, end } => (start, end),
        Ranged::Unsatisfiable => return Ok(unsatisfiable(size, &tag)),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };

    let mut file = tokio::fs::File::open(&resolved).await.map_err(|e| {
        archive_error(
            ArchiveErrorKind::VerificationIo,
            format!("{}: {}", relative.as_str(), e.kind()),
        )
    })?;
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| {
                archive_error(
                    ArchiveErrorKind::VerificationIo,
                    format!("{}: {}", relative.as_str(), e.kind()),
                )
            })?;
    }

    let status = if matches!(ranged, Ranged::Partial { .. }) {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file.take(length))));
    *response.status_mut() = status;
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, value(content_type));
    h.insert(header::CONTENT_LENGTH, value(&length.to_string()));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::ETAG, value(&tag));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(
            header::CONTENT_RANGE,
            value(&format!("bytes {start}-{end}/{size}")),
        );
    }
    Ok(response)
}

/// A header value built from text Uguisu controls; anything a header cannot
/// carry becomes an empty value rather than a panic.
fn value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn header_eq(headers: &HeaderMap, name: &header::HeaderName, tag: &str) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|raw| {
            raw == "*"
                || raw
                    .split(',')
                    .any(|candidate| candidate.trim().trim_start_matches("W/") == tag)
        })
}

fn if_range_ok(headers: &HeaderMap, tag: &str) -> bool {
    match headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok()) {
        // Only the entity tag is accepted: Uguisu does not publish a
        // `Last-Modified` strong enough to splice a file on.
        Some(raw) => raw.trim() == tag,
        None => true,
    }
}

fn not_modified(tag: &str, cache_control: &'static str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    let h = response.headers_mut();
    h.insert(header::ETAG, value(tag));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

fn unsatisfiable(size: u64, tag: &str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    let h = response.headers_mut();
    h.insert(header::CONTENT_RANGE, value(&format!("bytes */{size}")));
    h.insert(header::ETAG, value(tag));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
}

/// The media type Uguisu serves for one archived artifact.
///
/// The recorded `Content-Type` came from the publisher's response, so it is
/// never echoed: it is mapped to a container and back to the one type Uguisu
/// publishes for that container, falling back to the sniffed container. A
/// type Uguisu does not recognise — including `text/html`, which a
/// same-origin response would turn into stored XSS — becomes
/// `application/octet-stream`.
pub(crate) fn media_type(content_type: Option<&str>, sniffed: Option<&str>) -> &'static str {
    content_type
        .and_then(uguisu_download::paths::mime_extension)
        .or(sniffed)
        .and_then(mime_for_container)
        .unwrap_or("application/octet-stream")
}

/// The type Uguisu publishes for a container extension, or `None` when the
/// container is not media at all.
fn mime_for_container(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "weba" => "audio/webm",
        "mp4" => "video/mp4",
        "m4v" => "video/x-m4v",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn open_range_reaches_the_end() {
        assert_eq!(
            range_of("bytes=100-", 500),
            Ranged::Partial {
                start: 100,
                end: 499
            }
        );
    }

    #[test]
    fn closed_range_clamps_to_length() {
        assert_eq!(
            range_of("bytes=0-9999", 500),
            Ranged::Partial { start: 0, end: 499 }
        );
    }

    #[test]
    fn suffix_range_counts_backwards() {
        assert_eq!(
            range_of("bytes=-10", 500),
            Ranged::Partial {
                start: 490,
                end: 499
            }
        );
    }

    #[test]
    fn suffix_longer_than_file_starts_at_zero() {
        assert_eq!(
            range_of("bytes=-900", 500),
            Ranged::Partial { start: 0, end: 499 }
        );
    }

    #[test]
    fn start_past_the_end_is_unsatisfiable() {
        assert_eq!(range_of("bytes=500-600", 500), Ranged::Unsatisfiable);
        assert_eq!(range_of("bytes=500-", 500), Ranged::Unsatisfiable);
    }

    #[test]
    fn inverted_range_is_unsatisfiable() {
        assert_eq!(range_of("bytes=300-100", 500), Ranged::Unsatisfiable);
    }

    #[test]
    fn zero_length_suffix_is_unsatisfiable() {
        assert_eq!(range_of("bytes=-0", 500), Ranged::Unsatisfiable);
    }

    #[test]
    fn any_range_on_an_empty_file_fails() {
        assert_eq!(range_of("bytes=0-0", 0), Ranged::Unsatisfiable);
    }

    #[test]
    fn unanswerable_syntax_serves_everything() {
        for raw in [
            "bytes=0-10,20-30",
            "items=0-10",
            "bytes=abc-def",
            "bytes=",
            "0-10",
        ] {
            assert_eq!(range_of(raw, 500), Ranged::Full, "{raw}");
        }
    }

    #[test]
    fn a_recorded_type_becomes_a_canonical_one() {
        assert_eq!(media_type(Some("audio/mpeg"), None), "audio/mpeg");
        assert_eq!(
            media_type(Some("AUDIO/MP3; charset=binary"), None),
            "audio/mpeg"
        );
        assert_eq!(media_type(Some("audio/x-m4a"), None), "audio/mp4");
        assert_eq!(media_type(Some("video/quicktime"), None), "video/quicktime");
    }

    #[test]
    fn an_unknown_type_uses_the_container() {
        assert_eq!(
            media_type(Some("application/binary"), Some("flac")),
            "audio/flac"
        );
        assert_eq!(media_type(None, Some("mp4")), "video/mp4");
    }

    #[test]
    fn markup_is_never_served_as_markup() {
        assert_eq!(
            media_type(Some("text/html"), Some("html")),
            "application/octet-stream"
        );
        assert_eq!(
            media_type(Some("image/svg+xml"), None),
            "application/octet-stream"
        );
        assert_eq!(media_type(None, None), "application/octet-stream");
    }

    #[test]
    fn a_weak_validator_still_matches() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("W/\"abc\""));
        assert!(header_eq(&headers, &header::IF_NONE_MATCH, "\"abc\""));
    }

    #[test]
    fn a_stale_if_range_is_refused() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_RANGE, HeaderValue::from_static("\"old\""));
        assert!(!if_range_ok(&headers, "\"new\""));
        assert!(if_range_ok(&HeaderMap::new(), "\"new\""));
    }
}
