//! Host identity for per-host limits.

use std::fmt;
use std::str::FromStr;

use url::{Host, Url};

use crate::error::HttpError;
use crate::policy::normalize_host;

/// `scheme://host:port` with the host normalized (lower case, no trailing
/// dot) and the port always present (known default filled in), so that
/// `HTTPS://Example.COM/` and `https://example.com.:443/x` share one key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostKey(String);

impl HostKey {
    /// Derives the key of a URL.
    pub fn of(url: &Url) -> Result<Self, HttpError> {
        let host = match url.host() {
            Some(Host::Domain(d)) => normalize_host(d),
            Some(Host::Ipv4(ip)) => ip.to_string(),
            Some(Host::Ipv6(ip)) => format!("[{ip}]"),
            None => return Err(HttpError::InvalidUrl(format!("{url} has no host"))),
        };
        let port = url
            .port_or_known_default()
            .ok_or_else(|| HttpError::InvalidUrl(format!("{url} has no port")))?;
        Ok(Self(format!("{}://{host}:{port}", url.scheme())))
    }

    /// The key as text (the stored form).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for HostKey {
    type Err = HttpError;

    /// Accepts a previously stored key (`scheme://host:port`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| HttpError::InvalidUrl(format!("not a host key: {s}")))?;
        let has_port = rest.rsplit_once(':').is_some_and(|(_, p)| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && !p.contains(']')
        });
        if scheme.is_empty() || rest.is_empty() || !has_port {
            return Err(HttpError::InvalidUrl(format!("not a host key: {s}")));
        }
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn key(u: &str) -> String {
        HostKey::of(&Url::parse(u).unwrap()).unwrap().0
    }

    #[test]
    fn normalizes_case_default_ports_and_trailing_dots() {
        assert_eq!(key("HTTPS://Example.COM/a"), "https://example.com:443");
        assert_eq!(key("https://example.com.:443/x"), "https://example.com:443");
        assert_eq!(key("http://example.com/"), "http://example.com:80");
        assert_eq!(key("http://example.com:8080/"), "http://example.com:8080");
        assert_ne!(key("http://example.com/"), key("https://example.com/"));
        assert_eq!(key("http://127.0.0.1:9000/x"), "http://127.0.0.1:9000");
        assert_eq!(key("http://[::1]:9000/x"), "http://[::1]:9000");
    }

    #[test]
    fn round_trips_through_text() {
        let k = HostKey::of(&Url::parse("https://cdn.example/a").unwrap()).unwrap();
        assert_eq!(k.as_str().parse::<HostKey>().unwrap(), k);
        assert!("cdn.example".parse::<HostKey>().is_err());
        assert!("https://cdn.example".parse::<HostKey>().is_err());
    }
}
