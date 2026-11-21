//! Outbound HTTP with protection against server-side request forgery.
//!
//! Uguisu fetches URLs that come from users and from feeds. By default it refuses to talk to
//! loopback, private, link-local and similar addresses, so that a feed cannot be used to probe
//! the network Uguisu runs in.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::{Host, Url};

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || ip.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10, carrier-grade NAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15, benchmarking
        || o[0] >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    // Addresses that carry an IPv4 address inside are judged by that address: a gateway
    // would deliver the request there.
    let embedded = |high: u16, low: u16| {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        is_public_v4(Ipv4Addr::new(a, b, c, d))
    };
    if s[0] == 0x2002 {
        return embedded(s[1], s[2]); // 6to4
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        return embedded(s[6], s[7]); // NAT64, well-known prefix
    }
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || s[..6] == [0, 0, 0, 0, 0, 0] // ::/96, IPv4-compatible and other reserved addresses
        || (s[0] & 0xfe00) == 0xfc00 // unique local
        || (s[0] & 0xffc0) == 0xfe80 // link local
        || (s[0] & 0xffc0) == 0xfec0 // site local (deprecated)
        || (s[0] == 0x2001 && s[1] < 0x0200) // 2001::/23: Teredo and other protocol ranges
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x3fff && s[1] < 0x1000) // 3fff::/20, documentation
        || (s[0] == 0x0064 && s[1] == 0xff9b) // NAT64, local-use prefixes
        || (s[0] == 0x0100 && s[1..4] == [0, 0, 0])) // 100::/64, discard only
}

/// Checks scheme and, for IP literals, the address itself. Host names are checked when resolved.
pub fn check_url(url: &Url, allow_private: bool) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!(
            "only http and https URLs are supported, got {}://",
            url.scheme()
        );
    }
    let host = url.host().context("URL has no host")?;
    if allow_private {
        return Ok(());
    }
    let public = match &host {
        Host::Ipv4(ip) => is_public_v4(*ip),
        Host::Ipv6(ip) => is_public_ip(IpAddr::V6(*ip)),
        Host::Domain(name) => !name.eq_ignore_ascii_case("localhost"),
    };
    if !public {
        bail!("refusing to fetch from a private or local address ({host})");
    }
    Ok(())
}

/// Parses user input into a fetchable URL. A missing scheme is taken to mean https.
pub fn parse_user_url(input: &str, allow_private: bool) -> Result<Url> {
    let input = input.trim();
    if input.is_empty() {
        bail!("no URL given");
    }
    let candidate = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&candidate).with_context(|| format!("not a valid URL: {input}"))?;
    check_url(&url, allow_private)?;
    Ok(url)
}

struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = resolved.filter(|a| is_public_ip(a.ip())).collect();
            if public.is_empty() {
                let err = std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{host} resolves only to private or local addresses"),
                );
                return Err(Box::new(err) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

fn proxy_configured() -> bool {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .iter()
    .any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()))
}

pub fn build_client(user_agent: &str, allow_private: bool) -> Result<reqwest::Client> {
    let redirect = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        match check_url(attempt.url(), allow_private) {
            Ok(()) => attempt.follow(),
            Err(e) => attempt.error(e.to_string()),
        }
    });
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent)
        .redirect(redirect)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60));
    if !allow_private {
        if proxy_configured() {
            // With a proxy the proxy resolves host names, so the resolver guard cannot apply.
            tracing::warn!(
                "an HTTP proxy is configured: private-network protection is limited to IP \
                 literals and 'localhost'; make sure the proxy cannot reach internal services"
            );
        } else {
            builder = builder.dns_resolver(Arc::new(GuardedResolver));
        }
    }
    builder.build().context("cannot build HTTP client")
}

/// The address without user name and password, for everything that is shown, logged, written
/// to the archive or published.
pub fn redact(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) if !parsed.username().is_empty() || parsed.password().is_some() => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        _ => url.to_string(),
    }
}

/// Gives `target` the user name and password of the podcast's feed address, if it has none of
/// its own and points at the very same server. Feeds protected by a password usually protect
/// their media files the same way. Credentials never travel to another host, nor from an
/// encrypted connection to an unencrypted one.
pub fn lend_credentials(target: &mut Url, feed_url: &str) {
    let Ok(feed) = Url::parse(feed_url) else {
        return;
    };
    let has_own = !target.username().is_empty() || target.password().is_some();
    let same_server = target.host() == feed.host()
        && target.port_or_known_default() == feed.port_or_known_default()
        && target.scheme() == feed.scheme();
    if has_own || !same_server || (feed.username().is_empty() && feed.password().is_none()) {
        return;
    }
    let _ = target.set_username(feed.username());
    let _ = target.set_password(feed.password());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_redacted_and_lent_only_to_the_same_server() {
        let feed = "https://me:secret@example.org/feed.xml";
        assert_eq!(redact(feed), "https://example.org/feed.xml");
        assert_eq!(
            redact("https://example.org/a?b=1"),
            "https://example.org/a?b=1"
        );
        assert_eq!(redact("not a url"), "not a url");

        let lend = |target: &str| {
            let mut url = Url::parse(target).unwrap();
            lend_credentials(&mut url, feed);
            url.to_string()
        };
        assert_eq!(
            lend("https://example.org/a.mp3"),
            "https://me:secret@example.org/a.mp3"
        );
        assert_eq!(
            lend("https://cdn.example.org/a.mp3"),
            "https://cdn.example.org/a.mp3"
        );
        assert_eq!(lend("http://example.org/a.mp3"), "http://example.org/a.mp3");
        assert_eq!(
            lend("https://example.org:8443/a.mp3"),
            "https://example.org:8443/a.mp3"
        );
        assert_eq!(
            lend("https://other:pw@example.org/a.mp3"),
            "https://other:pw@example.org/a.mp3"
        );
    }

    #[test]
    fn address_classes() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "::10.0.0.1",
            "2002:7f00:1::1",
            "2002:c0a8:1::",
            "64:ff9b::10.0.0.1",
            "64:ff9b:1::1",
            "2001::1",
            "fec0::1",
            "100::1",
        ] {
            assert!(
                !is_public_ip(private.parse().unwrap()),
                "{private} must be blocked"
            );
        }
        for public in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700::1111",
            "2001:4860:4860::8888",
            "2002:101:101::1",
            "64:ff9b::1.1.1.1",
        ] {
            assert!(
                is_public_ip(public.parse().unwrap()),
                "{public} must be allowed"
            );
        }
    }

    #[test]
    fn user_urls() {
        assert_eq!(
            parse_user_url(" example.org/feed ", false)
                .unwrap()
                .as_str(),
            "https://example.org/feed"
        );
        assert!(parse_user_url("ftp://example.org/feed", false).is_err());
        assert!(parse_user_url("http://127.0.0.1/feed", false).is_err());
        assert!(parse_user_url("http://localhost/feed", false).is_err());
        assert!(parse_user_url("http://[::1]/feed", false).is_err());
        assert!(parse_user_url("http://127.0.0.1/feed", true).is_ok());
    }
}
