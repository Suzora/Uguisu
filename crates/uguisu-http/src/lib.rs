//! The only crate in Uguisu that builds HTTP clients.
//!
//! Every outbound request — discovery providers, feed resolution, feed
//! fetching, media downloads — goes through [`HttpClient`], which applies one
//! [`NetworkPolicy`] in two layers (ADR 0011, `docs/SECURITY.md` §3.1):
//!
//! 1. **Before connecting:** the URL is checked (scheme, port, host deny list,
//!    literal IP classification) and the host name is resolved and classified,
//!    so loopback, private, link-local, multicast and other non-public ranges
//!    are refused unless the host is allow-listed.
//! 2. **While connecting:** a DNS resolver hook returns only addresses that
//!    passed the same classification, so the socket can never reach one that
//!    was not validated. This is what closes the DNS-rebinding gap.
//!
//! Redirects are never followed automatically: [`HttpClient::get`] walks them,
//! re-validates every hop and drops credentials when the origin changes. Bodies
//! have a per-profile size cap, requests honour a [`CancellationToken`],
//! idempotent `GET`s retry with backoff, providers share a [`Throttle`] and
//! downloads take per-host slots from [`HostThrottles`].
//! [`HttpClient::get_stream`] reads large bodies incrementally with validated
//! `Range` resumption and an idle timeout.
//!
//! [`CancellationToken`]: tokio_util::sync::CancellationToken

mod client;
mod error;
pub mod headers;
mod host;
mod limiter;
mod policy;
mod resolver;
mod retry;
mod stream;

pub use client::{ClientConfig, Conditional, GetOptions, HttpClient, Profile, Response};
pub use error::HttpError;
pub use headers::ContentRange;
pub use host::HostKey;
pub use limiter::{HostThrottles, Throttle, ThrottleConfig, ThrottlePermit};
pub use policy::{
    AddressClass, NetworkPolicy, PolicyViolation, ViolationKind, classify_ip, normalize_host,
};
pub use resolver::SafeResolver;
pub use retry::{RetryPolicy, retry_after};
pub use stream::{BodyStream, StreamingResponse};

pub use bytes::Bytes;
pub use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
pub use tokio_util::sync::CancellationToken;
pub use url::Url;
