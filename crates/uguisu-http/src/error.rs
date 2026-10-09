//! Error type shared by every request.

use std::time::Duration;

use crate::policy::PolicyViolation;

/// Why a request failed.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The URL or a resolved address violates the network policy.
    #[error("blocked by network policy: {0}")]
    Policy(#[from] PolicyViolation),
    /// The URL could not be parsed or has no host.
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    /// DNS resolution failed (no answer, NXDOMAIN, resolver error).
    #[error("dns resolution failed for {host}: {detail}")]
    Dns {
        /// Host that failed to resolve.
        host: String,
        /// Underlying detail.
        detail: String,
    },
    /// TCP/TLS connection failed.
    #[error("connection failed: {0}")]
    Connect(String),
    /// The request or body read exceeded its timeout.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    /// More redirects than allowed.
    #[error("too many redirects (limit {limit})")]
    TooManyRedirects {
        /// Configured hop limit.
        limit: usize,
    },
    /// A redirect response carried no usable `Location` header.
    #[error("redirect without a valid Location header (status {0})")]
    BadRedirect(u16),
    /// The response body exceeded the profile's size cap.
    #[error("response body exceeds {limit} bytes")]
    BodyTooLarge {
        /// Cap in bytes.
        limit: u64,
    },
    /// The server answered with an error status after retries were exhausted.
    #[error("http status {status}")]
    Status {
        /// Status code.
        status: u16,
        /// First bytes of the body, for diagnostics.
        body: bytes::Bytes,
    },
    /// The body could not be read or decoded.
    #[error("body read failed: {0}")]
    Body(String),
    /// A response header the caller depends on could not be parsed.
    #[error("malformed {name} header: {value:?}")]
    MalformedHeader {
        /// Header name (lower case).
        name: String,
        /// Raw value.
        value: String,
    },
    /// The request was cancelled through its token.
    #[error("cancelled")]
    Cancelled,
    /// The client could not be built.
    #[error("client build failed: {0}")]
    Build(String),
    /// Any other transport error.
    #[error("request failed: {0}")]
    Transport(String),
}

impl HttpError {
    /// Short stable identifier for logs and API error bodies.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Policy(_) => "policy",
            Self::InvalidUrl(_) => "invalid_url",
            Self::Dns { .. } => "dns",
            Self::Connect(_) => "connect",
            Self::Timeout(_) => "timeout",
            Self::TooManyRedirects { .. } => "too_many_redirects",
            Self::BadRedirect(_) => "bad_redirect",
            Self::BodyTooLarge { .. } => "body_too_large",
            Self::Status { .. } => "status",
            Self::Body(_) => "body",
            Self::MalformedHeader { .. } => "malformed_header",
            Self::Cancelled => "cancelled",
            Self::Build(_) => "build",
            Self::Transport(_) => "transport",
        }
    }

    /// Whether an idempotent request may be retried after this error.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Connect(_) | Self::Timeout(_) | Self::Transport(_) | Self::Body(_) => true,
            Self::Status { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }

    /// The HTTP status, when the error carries one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub(crate) fn from_reqwest(err: &reqwest::Error, timeout: Duration) -> Self {
        // A policy violation raised inside the resolver hook surfaces as a
        // connect/request error with our type in the source chain.
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
        while let Some(s) = source {
            if let Some(v) = s.downcast_ref::<PolicyViolation>() {
                return Self::Policy(v.clone());
            }
            source = s.source();
        }
        if err.is_timeout() {
            Self::Timeout(timeout)
        } else if err.is_dns() {
            Self::Dns {
                host: err
                    .url()
                    .and_then(|u| u.host_str())
                    .unwrap_or("?")
                    .to_owned(),
                detail: root_message(err),
            }
        } else if err.is_connect() {
            Self::Connect(root_message(err))
        } else if err.is_body() || err.is_decode() {
            Self::Body(root_message(err))
        } else if err.is_builder() {
            Self::Build(root_message(err))
        } else {
            Self::Transport(root_message(err))
        }
    }
}

fn root_message(err: &dyn std::error::Error) -> String {
    let mut msg = err.to_string();
    let mut source = err.source();
    while let Some(s) = source {
        msg = s.to_string();
        source = s.source();
    }
    msg
}
