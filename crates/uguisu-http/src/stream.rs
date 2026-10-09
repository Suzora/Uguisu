//! Streaming responses for large bodies (media downloads).

use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::HttpError;
use crate::headers;

/// The head of a response whose body is read incrementally through
/// [`BodyStream`]. Produced by [`HttpClient::get_stream`](crate::HttpClient::get_stream).
#[derive(Debug)]
pub struct StreamingResponse {
    /// URL that produced the final response (after redirects).
    pub url: Url,
    /// Status of the final response.
    pub status: StatusCode,
    /// Headers of the final response.
    pub headers: HeaderMap,
    /// Every URL that answered with a redirect, in order.
    pub redirects: Vec<Url>,
    /// `true` when every hop was permanent (301 or 308) and there was at least one.
    pub permanent_redirect: bool,
    /// Attempts made for the final hop before headers arrived (1 = no retry).
    pub attempts: u32,
    /// Time until the headers of the final hop arrived.
    pub elapsed: Duration,
    /// The body, delivered chunk by chunk.
    pub body: BodyStream,
}

impl StreamingResponse {
    /// `Content-Type` header value, if any.
    pub fn content_type(&self) -> Option<&str> {
        headers::content_type(&self.headers)
    }

    /// `ETag` header value (verbatim), if any.
    pub fn etag(&self) -> Option<&str> {
        headers::etag(&self.headers)
    }

    /// `Last-Modified` header value (verbatim), if any.
    pub fn last_modified(&self) -> Option<&str> {
        headers::last_modified(&self.headers)
    }

    /// `Content-Length` as declared by the server.
    pub fn content_length(&self) -> Option<u64> {
        headers::content_length(&self.headers)
    }

    /// `Content-Encoding` when the body is not identity-encoded.
    pub fn content_encoding(&self) -> Option<String> {
        headers::content_encoding(&self.headers)
    }

    /// Whether the server advertised `Accept-Ranges: bytes`.
    pub fn accept_ranges_bytes(&self) -> bool {
        headers::accept_ranges_bytes(&self.headers)
    }

    /// The parsed `Content-Range` header, if present.
    pub fn content_range(&self) -> Option<Result<headers::ContentRange, HttpError>> {
        headers::content_range(&self.headers)
    }
}

/// Incremental body reader: one chunk at a time, bounded by an optional idle
/// timeout per chunk, the caller's cancellation token and a total size cap.
/// Dropping it closes the connection (the body is not drained).
pub struct BodyStream {
    inner: reqwest::Response,
    idle: Option<Duration>,
    cancel: CancellationToken,
    received: u64,
    max_bytes: u64,
}

impl std::fmt::Debug for BodyStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodyStream")
            .field("idle", &self.idle)
            .field("received", &self.received)
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl BodyStream {
    pub(crate) fn new(
        inner: reqwest::Response,
        idle: Option<Duration>,
        cancel: CancellationToken,
        max_bytes: u64,
    ) -> Self {
        Self {
            inner,
            idle,
            cancel,
            received: 0,
            max_bytes,
        }
    }

    /// The next chunk, or `None` at the end of the body.
    ///
    /// Errors: [`HttpError::Cancelled`] when the token fires,
    /// [`HttpError::Timeout`] when no bytes arrive within the idle timeout,
    /// [`HttpError::BodyTooLarge`] when the cap is exceeded, and
    /// [`HttpError::Body`]/[`HttpError::Transport`] for connection failures
    /// (a server that closes early surfaces here, not as `None`).
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, HttpError> {
        let idle = self.idle;
        let cancel = self.cancel.clone();
        let next = async {
            match idle {
                Some(d) => tokio::time::timeout(d, self.inner.chunk())
                    .await
                    .map_err(|_| HttpError::Timeout(d))?
                    .map_err(|e| HttpError::from_reqwest(&e, d)),
                None => self
                    .inner
                    .chunk()
                    .await
                    .map_err(|e| HttpError::from_reqwest(&e, Duration::ZERO)),
            }
        };
        let chunk = tokio::select! {
            () = cancel.cancelled() => return Err(HttpError::Cancelled),
            r = next => r?,
        };
        if let Some(c) = &chunk {
            self.received = self.received.saturating_add(c.len() as u64);
            if self.received > self.max_bytes {
                return Err(HttpError::BodyTooLarge {
                    limit: self.max_bytes,
                });
            }
        }
        Ok(chunk)
    }

    /// Bytes delivered so far.
    pub const fn received(&self) -> u64 {
        self.received
    }
}
