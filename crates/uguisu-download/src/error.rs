//! Download failures and their classification into the stable
//! [`DownloadErrorKind`] taxonomy (`docs/DOWNLOAD_ENGINE.md` "Retry taxonomy").

use std::path::PathBuf;
use std::time::Duration;

use uguisu_core::download::DownloadErrorKind;
use uguisu_http::HttpError;

/// Why an attempt failed.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// Transport, policy or protocol failure from the HTTP client.
    #[error("{0}")]
    Http(#[from] HttpError),
    /// A non-success status on the final hop.
    #[error("http status {status}")]
    Status {
        /// The status.
        status: u16,
        /// Parsed `Retry-After`, when the server sent one.
        retry_after: Option<Duration>,
    },
    /// A `206` whose `Content-Range` does not match the request.
    #[error("range answer does not match the request: {0}")]
    RangeInvalid(String),
    /// Fewer or more bytes than the declared length.
    #[error("received {received} bytes, expected {expected}")]
    LengthMismatch {
        /// Declared length (remaining after the resume offset).
        expected: u64,
        /// Bytes actually received.
        received: u64,
    },
    /// The body failed validation.
    #[error("validation failed: {0}")]
    Validation(String),
    /// A file system operation failed.
    #[error("i/o at {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// The database failed while the job ran.
    #[error("storage: {0}")]
    Storage(#[from] uguisu_storage::StorageError),
    /// Cancelled through the job token.
    #[error("cancelled")]
    Cancelled,
}

impl DownloadError {
    /// The HTTP status carried by the error, when any.
    #[must_use]
    pub const fn http_status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// `Retry-After`, when the server sent one.
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Status { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// A short, header-free description for the attempt log.
    #[must_use]
    pub fn detail(&self) -> String {
        let s = self.to_string();
        s.chars().take(512).collect()
    }
}

/// Whether an I/O error means the file system is full (`ENOSPC`, Windows
/// `ERROR_DISK_FULL`/`ERROR_HANDLE_DISK_FULL`).
#[must_use]
pub fn is_disk_full(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::StorageFull {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(code) => code == 28,
        #[cfg(windows)]
        Some(code) => code == 112 || code == 39,
        #[cfg(not(any(unix, windows)))]
        Some(_) => false,
        None => false,
    }
}

/// Classifies an error: the stable kind and whether a later attempt may
/// succeed without user action. `DiskFull` is never retried (it pauses
/// the queue); `Cancelled` and `Storage` are not failures of the resource.
#[must_use]
pub fn classify(err: &DownloadError) -> (DownloadErrorKind, bool) {
    use DownloadErrorKind as K;
    let kind = match err {
        DownloadError::Http(h) => match h {
            HttpError::Policy(_) => K::PolicyBlocked,
            HttpError::Dns { .. } => K::Dns,
            HttpError::Connect(d) | HttpError::Transport(d) if looks_like_tls(d) => K::Tls,
            HttpError::Connect(_) | HttpError::Transport(_) | HttpError::Body(_) => K::Network,
            HttpError::Timeout(_) => K::Timeout,
            HttpError::InvalidUrl(_)
            | HttpError::TooManyRedirects { .. }
            | HttpError::BadRedirect(_)
            | HttpError::Build(_) => K::Http,
            HttpError::MalformedHeader { .. } => K::RangeInvalid,
            HttpError::BodyTooLarge { .. } => K::Validation,
            HttpError::Status { status, .. } => status_kind(*status),
            HttpError::Cancelled => K::Cancelled,
        },
        DownloadError::Status { status, .. } => status_kind(*status),
        DownloadError::RangeInvalid(_) => K::RangeInvalid,
        DownloadError::LengthMismatch { .. } => K::ContentLengthMismatch,
        DownloadError::Validation(_) => K::Validation,
        DownloadError::Io { source, .. } => {
            if is_disk_full(source) {
                K::DiskFull
            } else if source.kind() == std::io::ErrorKind::PermissionDenied {
                K::PermissionDenied
            } else {
                K::Io
            }
        }
        DownloadError::Storage(_) => K::Storage,
        DownloadError::Cancelled => K::Cancelled,
    };
    let retryable = match (err, kind) {
        // Malformed redirects and the like are not retried; unknown-status
        // retries are decided by the status table.
        (
            DownloadError::Http(
                HttpError::InvalidUrl(_)
                | HttpError::TooManyRedirects { .. }
                | HttpError::BadRedirect(_)
                | HttpError::Build(_),
            ),
            _,
        ) => false,
        (
            DownloadError::Status { status, .. }
            | DownloadError::Http(HttpError::Status { status, .. }),
            K::Http,
        ) => status_retryable(*status),
        _ => kind.is_retryable(),
    };
    (kind, retryable)
}

fn looks_like_tls(detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    d.contains("tls") || d.contains("certificate") || d.contains("ssl") || d.contains("handshake")
}

const fn status_kind(status: u16) -> DownloadErrorKind {
    use DownloadErrorKind as K;
    match status {
        429 => K::RateLimited,
        401 => K::Unauthorized,
        403 => K::Forbidden,
        404 | 410 => K::NotFound,
        _ => K::Http,
    }
}

/// Statuses worth another attempt.
const fn status_retryable(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

#[cfg(test)]
mod tests {
    use uguisu_http::PolicyViolation;

    use super::*;
    use DownloadErrorKind as K;

    fn status(s: u16) -> DownloadError {
        DownloadError::Status {
            status: s,
            retry_after: None,
        }
    }

    #[test]
    fn status_table() {
        for (s, kind, retry) in [
            (401, K::Unauthorized, false),
            (403, K::Forbidden, false),
            (404, K::NotFound, false),
            (410, K::NotFound, false),
            (408, K::Http, true),
            (425, K::Http, true),
            (429, K::RateLimited, true),
            (500, K::Http, true),
            (502, K::Http, true),
            (503, K::Http, true),
            (504, K::Http, true),
            (400, K::Http, false),
            (418, K::Http, false),
            (501, K::Http, false),
        ] {
            assert_eq!(classify(&status(s)), (kind, retry), "status {s}");
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn transport_and_local_errors() {
        let cases: Vec<(DownloadError, K, bool)> = vec![
            (
                DownloadError::Http(HttpError::Policy(PolicyViolation {
                    host: "x".into(),
                    kind: uguisu_http::ViolationKind::HostDenied,
                })),
                K::PolicyBlocked,
                false,
            ),
            (
                DownloadError::Http(HttpError::Dns {
                    host: "x".into(),
                    detail: "nx".into(),
                }),
                K::Dns,
                true,
            ),
            (
                DownloadError::Http(HttpError::Connect("certificate verify failed".into())),
                K::Tls,
                false,
            ),
            (
                DownloadError::Http(HttpError::Connect("connection refused".into())),
                K::Network,
                true,
            ),
            (
                DownloadError::Http(HttpError::Transport("connection reset by peer".into())),
                K::Network,
                true,
            ),
            (
                DownloadError::Http(HttpError::Timeout(Duration::from_secs(1))),
                K::Timeout,
                true,
            ),
            (
                DownloadError::Http(HttpError::TooManyRedirects { limit: 5 }),
                K::Http,
                false,
            ),
            (
                DownloadError::Http(HttpError::BodyTooLarge { limit: 1 }),
                K::Validation,
                false,
            ),
            (
                DownloadError::Http(HttpError::Cancelled),
                K::Cancelled,
                false,
            ),
            (
                DownloadError::RangeInvalid("x".into()),
                K::RangeInvalid,
                false,
            ),
            (
                DownloadError::LengthMismatch {
                    expected: 10,
                    received: 5,
                },
                K::ContentLengthMismatch,
                true,
            ),
            (
                DownloadError::Validation("empty".into()),
                K::Validation,
                false,
            ),
            (
                DownloadError::Io {
                    path: "/x".into(),
                    source: std::io::Error::from(std::io::ErrorKind::StorageFull),
                },
                K::DiskFull,
                false,
            ),
            (
                DownloadError::Io {
                    path: "/x".into(),
                    source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                },
                K::PermissionDenied,
                false,
            ),
            (
                DownloadError::Io {
                    path: "/x".into(),
                    source: std::io::Error::other("boom"),
                },
                K::Io,
                false,
            ),
            (DownloadError::Cancelled, K::Cancelled, false),
        ];
        for (e, kind, retry) in cases {
            assert_eq!(classify(&e), (kind, retry), "{e}");
        }
        #[cfg(unix)]
        assert!(is_disk_full(&std::io::Error::from_raw_os_error(28)));
        assert!(!is_disk_full(&std::io::Error::other("x")));
        let d = status(503).detail();
        assert_eq!(d, "http status 503");
    }
}
