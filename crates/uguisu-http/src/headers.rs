//! Header accessors shared by buffered and streaming responses.

use std::time::Duration;

use http::HeaderMap;

use crate::error::HttpError;

fn text(headers: &HeaderMap, name: http::header::HeaderName) -> Option<&str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// `Content-Type` header value, if any.
pub fn content_type(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
}

/// `ETag` header value, if any (kept verbatim, including weak markers
/// and quotes, so it can be sent back in `If-None-Match` unchanged).
pub fn etag(headers: &HeaderMap) -> Option<&str> {
    text(headers, http::header::ETAG)
}

/// Whether an `ETag` value is a strong validator (not `W/…`), which is the
/// only kind that may be used in `If-Range`.
pub fn is_strong_etag(etag: &str) -> bool {
    let t = etag.trim();
    !t.starts_with("W/") && !t.starts_with("w/") && t.starts_with('"') && t.len() >= 2
}

/// `Last-Modified` header value, if any (verbatim, for `If-Modified-Since`).
pub fn last_modified(headers: &HeaderMap) -> Option<&str> {
    text(headers, http::header::LAST_MODIFIED)
}

/// `Content-Length` parsed from the header itself (not inferred from the
/// transport), so it reflects what the server declared.
pub fn content_length(headers: &HeaderMap) -> Option<u64> {
    text(headers, http::header::CONTENT_LENGTH)?.parse().ok()
}

/// `Content-Encoding` value in lower case, if present and not `identity`.
pub fn content_encoding(headers: &HeaderMap) -> Option<String> {
    let v = text(headers, http::header::CONTENT_ENCODING)?.to_ascii_lowercase();
    (v != "identity").then_some(v)
}

/// Whether the server advertised `Accept-Ranges: bytes`.
pub fn accept_ranges_bytes(headers: &HeaderMap) -> bool {
    text(headers, http::header::ACCEPT_RANGES)
        .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("bytes")))
}

/// A parsed `Content-Range` header (RFC 9110 §14.4), bytes unit only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentRange {
    /// `bytes start-end/total` (`total` is `None` for `*`).
    Bytes {
        /// First byte position of the returned range.
        start: u64,
        /// Last byte position (inclusive).
        end: u64,
        /// Complete length when known.
        total: Option<u64>,
    },
    /// `bytes */total`, sent with `416 Range Not Satisfiable`.
    Unsatisfied {
        /// Complete length when known.
        total: Option<u64>,
    },
}

impl ContentRange {
    /// Parses a `Content-Range` value.
    pub fn parse(value: &str) -> Result<Self, HttpError> {
        let malformed = || HttpError::MalformedHeader {
            name: "content-range".to_owned(),
            value: value.to_owned(),
        };
        let v = value.trim();
        let rest = v
            .strip_prefix("bytes ")
            .or_else(|| v.strip_prefix("bytes\t"))
            .ok_or_else(malformed)?
            .trim();
        let (range, total) = rest.split_once('/').ok_or_else(malformed)?;
        let total = match total.trim() {
            "*" => None,
            t => Some(t.parse::<u64>().map_err(|_| malformed())?),
        };
        if range.trim() == "*" {
            return Ok(Self::Unsatisfied { total });
        }
        let (start, end) = range.split_once('-').ok_or_else(malformed)?;
        let start: u64 = start.trim().parse().map_err(|_| malformed())?;
        let end: u64 = end.trim().parse().map_err(|_| malformed())?;
        if end < start || total.is_some_and(|t| end >= t) {
            return Err(malformed());
        }
        Ok(Self::Bytes { start, end, total })
    }

    /// Complete length when the header carries one.
    pub const fn total(self) -> Option<u64> {
        match self {
            Self::Bytes { total, .. } | Self::Unsatisfied { total } => total,
        }
    }
}

/// `Content-Range` parsed, `None` when absent.
pub fn content_range(headers: &HeaderMap) -> Option<Result<ContentRange, HttpError>> {
    text(headers, http::header::CONTENT_RANGE).map(ContentRange::parse)
}

/// How long the response may be cached according to `Cache-Control`:
/// `max-age`/`s-maxage` in seconds, [`Duration::ZERO`] for `no-store` or
/// `no-cache` (the origin forbids reuse), `None` when the header says
/// nothing about freshness.
pub fn cache_max_age(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(http::header::CACHE_CONTROL)?.to_str().ok()?;
    let forbids_reuse = value
        .split(',')
        .map(str::trim)
        .any(|d| d.eq_ignore_ascii_case("no-store") || d.eq_ignore_ascii_case("no-cache"));
    if forbids_reuse {
        return Some(Duration::ZERO);
    }
    value.split(',').map(str::trim).find_map(|d| {
        let (k, v) = d.split_once('=')?;
        (k.eq_ignore_ascii_case("max-age") || k.eq_ignore_ascii_case("s-maxage"))
            .then(|| v.trim_matches('"').parse::<u64>().ok())
            .flatten()
            .map(Duration::from_secs)
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(
                http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        m
    }

    #[test]
    fn content_range_table() {
        assert_eq!(
            ContentRange::parse("bytes 100-199/200").unwrap(),
            ContentRange::Bytes {
                start: 100,
                end: 199,
                total: Some(200)
            }
        );
        assert_eq!(
            ContentRange::parse("bytes 100-199/*").unwrap(),
            ContentRange::Bytes {
                start: 100,
                end: 199,
                total: None
            }
        );
        assert_eq!(
            ContentRange::parse("bytes */200").unwrap(),
            ContentRange::Unsatisfied { total: Some(200) }
        );
        for bad in [
            "",
            "bytes",
            "bytes 1-0/10",
            "bytes 5-10/10",
            "bytes a-b/c",
            "items 0-1/2",
            "bytes 0-1",
        ] {
            assert!(
                matches!(
                    ContentRange::parse(bad),
                    Err(HttpError::MalformedHeader { .. })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn accessors() {
        let h = map(&[
            ("content-length", " 42 "),
            ("accept-ranges", "none, bytes"),
            ("content-encoding", "GZIP"),
            ("etag", "\"abc\""),
        ]);
        assert_eq!(content_length(&h), Some(42));
        assert!(accept_ranges_bytes(&h));
        assert_eq!(content_encoding(&h).as_deref(), Some("gzip"));
        assert!(is_strong_etag(etag(&h).unwrap()));
        assert!(!is_strong_etag("W/\"abc\""));
        assert!(!is_strong_etag("abc"));
        let h = map(&[("content-encoding", "identity"), ("accept-ranges", "none")]);
        assert_eq!(content_encoding(&h), None);
        assert!(!accept_ranges_bytes(&h));
        assert_eq!(content_length(&map(&[("content-length", "x")])), None);
    }
}
