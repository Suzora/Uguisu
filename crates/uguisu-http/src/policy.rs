//! Network policy: which URLs and which resolved addresses Uguisu may talk to.
//!
//! The policy is deliberately conservative: everything that is not a public
//! unicast address is refused unless the host was allow-listed by the
//! administrator (`network.allow_private_hosts`).

use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

/// Classification of an IP address for SSRF purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressClass {
    /// Globally routable unicast address.
    Public,
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// RFC 1918 (`10/8`, `172.16/12`, `192.168/16`), unique-local `fc00::/7`.
    Private,
    /// Carrier-grade NAT `100.64.0.0/10`.
    CarrierGradeNat,
    /// `169.254.0.0/16`, `fe80::/10` (includes cloud metadata endpoints).
    LinkLocal,
    /// `224.0.0.0/4`, `ff00::/8`.
    Multicast,
    /// `0.0.0.0/8`, `::`.
    Unspecified,
    /// `255.255.255.255`.
    Broadcast,
    /// Documentation, benchmarking, discard and other reserved ranges.
    Reserved,
}

impl AddressClass {
    /// Whether a connection to an address of this class is allowed by default.
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }
}

impl fmt::Display for AddressClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Public => "public",
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::CarrierGradeNat => "carrier-grade NAT",
            Self::LinkLocal => "link-local",
            Self::Multicast => "multicast",
            Self::Unspecified => "unspecified",
            Self::Broadcast => "broadcast",
            Self::Reserved => "reserved",
        };
        f.write_str(s)
    }
}

/// Classifies an IP address. IPv6 addresses that embed an IPv4 address
/// (mapped, compatible, NAT64, 6to4, Teredo) are classified by the embedded
/// IPv4 address so that `::ffff:127.0.0.1` is loopback, not public.
pub fn classify_ip(ip: IpAddr) -> AddressClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    let [a, b, c, _] = ip.octets();
    if ip.is_unspecified() || a == 0 {
        AddressClass::Unspecified
    } else if ip.is_loopback() {
        AddressClass::Loopback
    } else if ip.is_private() {
        AddressClass::Private
    } else if a == 100 && (64..=127).contains(&b) {
        AddressClass::CarrierGradeNat
    } else if ip.is_link_local() {
        AddressClass::LinkLocal
    } else if ip.is_multicast() {
        AddressClass::Multicast
    } else if ip.is_broadcast() {
        AddressClass::Broadcast
    } else if ip.is_documentation()
        || (a == 198 && (b == 18 || b == 19)) // benchmarking 198.18.0.0/15
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments 192.0.0.0/24
        || (a == 192 && b == 88 && c == 99) // deprecated 6to4 relay anycast
        || a >= 240
    // 240.0.0.0/4 reserved (255.255.255.255 handled above)
    {
        AddressClass::Reserved
    } else {
        AddressClass::Public
    }
}

/// Builds an IPv4 address from two 16-bit IPv6 segments.
fn v4_from_segments(hi: u16, lo: u16) -> Ipv4Addr {
    let [a, b] = hi.to_be_bytes();
    let [c, d] = lo.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    let seg = ip.segments();
    if ip.is_unspecified() {
        return AddressClass::Unspecified;
    }
    if ip.is_loopback() {
        return AddressClass::Loopback;
    }
    // IPv4-mapped ::ffff:a.b.c.d and deprecated IPv4-compatible ::a.b.c.d
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    if seg[..5] == [0, 0, 0, 0, 0] && seg[5] == 0 && (seg[6] != 0 || seg[7] != 0) {
        return classify_v4(v4_from_segments(seg[6], seg[7]));
    }
    // NAT64 well-known prefix 64:ff9b::/96
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
        return classify_v4(v4_from_segments(seg[6], seg[7]));
    }
    // 6to4 2002:AABB:CCDD::/48 embeds a.b.c.d
    if seg[0] == 0x2002 {
        return classify_v4(v4_from_segments(seg[1], seg[2]));
    }
    // Teredo 2001:0000::/32 embeds the client IPv4 (inverted) in the last 32 bits
    if seg[0] == 0x2001 && seg[1] == 0 {
        return classify_v4(v4_from_segments(!seg[6], !seg[7]));
    }
    if (seg[0] & 0xffc0) == 0xfe80 {
        return AddressClass::LinkLocal;
    }
    if (seg[0] & 0xfe00) == 0xfc00 {
        return AddressClass::Private;
    }
    if (seg[0] & 0xff00) == 0xff00 {
        return AddressClass::Multicast;
    }
    // documentation 2001:db8::/32, ORCHID 2001:10::/28 and 2001:20::/28, discard 100::/64
    if (seg[0] == 0x2001
        && (seg[1] == 0x0db8 || (seg[1] & 0xfff0) == 0x0010 || (seg[1] & 0xfff0) == 0x0020))
        || (seg[0] == 0x0100 && seg[1..4] == [0, 0, 0])
    {
        return AddressClass::Reserved;
    }
    AddressClass::Public
}

/// What the policy objected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKind {
    /// Only `http` and `https` are allowed.
    SchemeNotAllowed(String),
    /// Port outside the allowed set.
    PortNotAllowed(u16),
    /// Host explicitly denied or a well-known local name.
    HostDenied,
    /// Host or a resolved address is not public.
    NonPublicAddress {
        /// The offending address.
        addr: IpAddr,
        /// Its classification.
        class: AddressClass,
    },
    /// Name resolved to no address that passes the policy.
    NoAllowedAddress,
    /// URL has no host or an unparsable one.
    InvalidHost,
}

/// A refused URL or address, with the host for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct PolicyViolation {
    /// Host as it appeared in the URL.
    pub host: String,
    /// What was wrong.
    pub kind: ViolationKind,
}

impl fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ViolationKind::SchemeNotAllowed(s) => write!(f, "scheme `{s}` is not allowed"),
            ViolationKind::PortNotAllowed(p) => {
                write!(f, "port {p} is not allowed for {}", self.host)
            }
            ViolationKind::HostDenied => write!(f, "host `{}` is denied", self.host),
            ViolationKind::NonPublicAddress { addr, class } => {
                write!(
                    f,
                    "`{}` resolves to {addr} which is a {class} address",
                    self.host
                )
            }
            ViolationKind::NoAllowedAddress => {
                write!(
                    f,
                    "`{}` resolved to no address allowed by policy",
                    self.host
                )
            }
            ViolationKind::InvalidHost => write!(f, "url has no valid host"),
        }
    }
}

/// The outbound network policy.
#[derive(Debug, Clone)]
pub struct NetworkPolicy {
    allow_private_hosts: HashSet<String>,
    deny_hosts: HashSet<String>,
    allowed_ports: Option<HashSet<u16>>,
    trusted: bool,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self::strict()
    }
}

impl NetworkPolicy {
    /// Public addresses only, ports 80/443 or ≥ 1024, no allow-list.
    pub fn strict() -> Self {
        Self {
            allow_private_hosts: HashSet::new(),
            deny_hosts: HashSet::new(),
            allowed_ports: None,
            trusted: false,
        }
    }

    /// Everything allowed. Only for user-configured internal targets such as
    /// the local Uguisu server the CLI talks to — never for feed or provider URLs.
    pub fn trusted() -> Self {
        Self {
            trusted: true,
            ..Self::strict()
        }
    }

    /// Adds hosts (names or IP literals) that may resolve to non-public addresses.
    #[must_use]
    pub fn allow_private_hosts<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.allow_private_hosts
            .extend(hosts.into_iter().map(|h| normalize_host(h.as_ref())));
        self
    }

    /// Adds hosts that are always refused.
    #[must_use]
    pub fn deny_hosts<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.deny_hosts
            .extend(hosts.into_iter().map(|h| normalize_host(h.as_ref())));
        self
    }

    /// Restricts ports to an explicit set (default: 80, 443 and every port ≥ 1024).
    #[must_use]
    pub fn allowed_ports<I: IntoIterator<Item = u16>>(mut self, ports: I) -> Self {
        self.allowed_ports = Some(ports.into_iter().collect());
        self
    }

    /// Whether this policy allows everything.
    pub const fn is_trusted(&self) -> bool {
        self.trusted
    }

    /// Whether `host` (name or IP literal) may use non-public addresses.
    pub fn is_private_allowed(&self, host: &str) -> bool {
        self.trusted || self.allow_private_hosts.contains(&normalize_host(host))
    }

    /// Checks scheme, port and host of a URL without resolving it.
    pub fn check_url(&self, url: &Url) -> Result<(), PolicyViolation> {
        let host_str = url.host_str().unwrap_or_default().to_owned();
        let violation = |kind| PolicyViolation {
            host: host_str.clone(),
            kind,
        };

        match url.scheme() {
            "http" | "https" => {}
            other => return Err(violation(ViolationKind::SchemeNotAllowed(other.to_owned()))),
        }
        let Some(host) = url.host() else {
            return Err(violation(ViolationKind::InvalidHost));
        };
        if self.trusted {
            return Ok(());
        }
        if let Some(port) = url.port() {
            let allowed = match &self.allowed_ports {
                Some(set) => set.contains(&port),
                None => port == 80 || port == 443 || port >= 1024,
            };
            if !allowed {
                return Err(violation(ViolationKind::PortNotAllowed(port)));
            }
        }
        match host {
            Host::Domain(name) => {
                let name = normalize_host(name);
                if self.deny_hosts.contains(&name) {
                    return Err(violation(ViolationKind::HostDenied));
                }
                if self.allow_private_hosts.contains(&name) {
                    return Ok(());
                }
                if is_local_name(&name) {
                    return Err(violation(ViolationKind::HostDenied));
                }
                Ok(())
            }
            Host::Ipv4(ip) => self.check_addr(&host_str, IpAddr::V4(ip)),
            Host::Ipv6(ip) => self.check_addr(&host_str, IpAddr::V6(ip)),
        }
    }

    /// Checks one resolved address for `host`.
    pub fn check_addr(&self, host: &str, addr: IpAddr) -> Result<(), PolicyViolation> {
        if self.is_private_allowed(host) {
            return Ok(());
        }
        let class = classify_ip(addr);
        if class.is_public() {
            Ok(())
        } else {
            Err(PolicyViolation {
                host: host.to_owned(),
                kind: ViolationKind::NonPublicAddress { addr, class },
            })
        }
    }
}

/// Lowercases, strips a trailing dot and IPv6 brackets.
pub fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// Names that always mean "this machine or this LAN" and never a public host.
fn is_local_name(name: &str) -> bool {
    const LOCAL_SUFFIXES: [&str; 8] = [
        "localhost",
        "local",
        "internal",
        "home.arpa",
        "lan",
        "intranet",
        "onion",
        "localdomain",
    ];
    name == "localhost"
        || !name.contains('.')
        || LOCAL_SUFFIXES.iter().any(|suffix| {
            name.len() > suffix.len()
                && name.as_bytes()[name.len() - suffix.len() - 1] == b'.'
                && name[name.len() - suffix.len()..] == **suffix
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn class(s: &str) -> AddressClass {
        classify_ip(s.parse().unwrap())
    }

    #[test]
    fn ipv4_classification() {
        assert_eq!(class("8.8.8.8"), AddressClass::Public);
        assert_eq!(class("1.1.1.1"), AddressClass::Public);
        assert_eq!(class("127.0.0.1"), AddressClass::Loopback);
        assert_eq!(class("127.1.2.3"), AddressClass::Loopback);
        assert_eq!(class("10.0.0.1"), AddressClass::Private);
        assert_eq!(class("172.16.0.1"), AddressClass::Private);
        assert_eq!(class("172.31.255.255"), AddressClass::Private);
        assert_eq!(class("172.32.0.1"), AddressClass::Public);
        assert_eq!(class("192.168.1.1"), AddressClass::Private);
        assert_eq!(class("100.64.0.1"), AddressClass::CarrierGradeNat);
        assert_eq!(class("100.127.255.255"), AddressClass::CarrierGradeNat);
        assert_eq!(class("100.128.0.1"), AddressClass::Public);
        assert_eq!(class("169.254.169.254"), AddressClass::LinkLocal);
        assert_eq!(class("224.0.0.1"), AddressClass::Multicast);
        assert_eq!(class("0.0.0.0"), AddressClass::Unspecified);
        assert_eq!(class("0.1.2.3"), AddressClass::Unspecified);
        assert_eq!(class("255.255.255.255"), AddressClass::Broadcast);
        assert_eq!(class("192.0.2.1"), AddressClass::Reserved);
        assert_eq!(class("198.51.100.7"), AddressClass::Reserved);
        assert_eq!(class("203.0.113.9"), AddressClass::Reserved);
        assert_eq!(class("198.18.0.1"), AddressClass::Reserved);
        assert_eq!(class("192.0.0.1"), AddressClass::Reserved);
        assert_eq!(class("240.0.0.1"), AddressClass::Reserved);
    }

    #[test]
    fn ipv6_classification() {
        assert_eq!(class("2606:4700::1111"), AddressClass::Public);
        assert_eq!(class("::1"), AddressClass::Loopback);
        assert_eq!(class("::"), AddressClass::Unspecified);
        assert_eq!(class("::ffff:127.0.0.1"), AddressClass::Loopback);
        assert_eq!(class("::ffff:10.1.1.1"), AddressClass::Private);
        assert_eq!(class("::ffff:8.8.8.8"), AddressClass::Public);
        assert_eq!(class("::10.0.0.1"), AddressClass::Private);
        assert_eq!(class("64:ff9b::7f00:1"), AddressClass::Loopback);
        assert_eq!(class("2002:0a00:0001::1"), AddressClass::Private);
        assert_eq!(class("2002:0808:0808::1"), AddressClass::Public);
        // RFC 4380 example: embeds 192.0.2.45 (documentation range)
        assert_eq!(
            class("2001:0:4136:e378:8000:63bf:3fff:fdd2"),
            AddressClass::Reserved
        );
        // Teredo client 8.8.8.8 → !8.8.8.8 = f7f7:f7f7
        assert_eq!(
            class("2001:0:4136:e378:8000:63bf:f7f7:f7f7"),
            AddressClass::Public
        );
        // Teredo client 127.0.0.1 → last 32 bits are !127.0.0.1 = 80ff:fffe
        assert_eq!(
            class("2001:0:4136:e378:8000:63bf:80ff:fffe"),
            AddressClass::Loopback
        );
        assert_eq!(class("fe80::1"), AddressClass::LinkLocal);
        assert_eq!(class("fc00::1"), AddressClass::Private);
        assert_eq!(class("fd12:3456::1"), AddressClass::Private);
        assert_eq!(class("ff02::1"), AddressClass::Multicast);
        assert_eq!(class("2001:db8::1"), AddressClass::Reserved);
        assert_eq!(class("2001:10::1"), AddressClass::Reserved);
        assert_eq!(class("100::1"), AddressClass::Reserved);
    }

    fn check(policy: &NetworkPolicy, url: &str) -> Result<(), ViolationKind> {
        policy
            .check_url(&Url::parse(url).unwrap())
            .map_err(|v| v.kind)
    }

    #[test]
    fn url_checks() {
        let p = NetworkPolicy::strict();
        assert!(check(&p, "https://example.com/feed").is_ok());
        assert!(check(&p, "http://example.com:8080/feed").is_ok());
        assert_eq!(
            check(&p, "ftp://example.com/x"),
            Err(ViolationKind::SchemeNotAllowed("ftp".into()))
        );
        assert_eq!(
            check(&p, "file:///etc/passwd"),
            Err(ViolationKind::SchemeNotAllowed("file".into()))
        );
        assert_eq!(
            check(&p, "http://example.com:22/"),
            Err(ViolationKind::PortNotAllowed(22))
        );
        assert!(
            check(&p, "http://example.com:6379/").is_ok(),
            "ports >= 1024 are allowed by default"
        );
        assert_eq!(
            check(&p, "http://example.com:25/"),
            Err(ViolationKind::PortNotAllowed(25))
        );
        assert_eq!(
            check(&p, "http://localhost/"),
            Err(ViolationKind::HostDenied)
        );
        assert_eq!(
            check(&p, "http://foo.localhost/"),
            Err(ViolationKind::HostDenied)
        );
        assert_eq!(
            check(&p, "http://localhost.localdomain/"),
            Err(ViolationKind::HostDenied)
        );
        assert!(check(&p, "http://notlocal.example/").is_ok());
        assert!(
            check(&p, "http://mylan.example/").is_ok(),
            "suffix must be a full label"
        );
        assert_eq!(
            check(&p, "http://nas.local/"),
            Err(ViolationKind::HostDenied)
        );
        assert_eq!(
            check(&p, "http://metadata.internal/"),
            Err(ViolationKind::HostDenied)
        );
        assert_eq!(
            check(&p, "http://intranet/"),
            Err(ViolationKind::HostDenied),
            "single-label names"
        );
        assert!(matches!(
            check(&p, "http://127.0.0.1/"),
            Err(ViolationKind::NonPublicAddress { .. })
        ));
        assert!(matches!(
            check(&p, "http://[::1]/"),
            Err(ViolationKind::NonPublicAddress { .. })
        ));
        assert!(matches!(
            check(&p, "http://[::ffff:127.0.0.1]/"),
            Err(ViolationKind::NonPublicAddress { .. })
        ));
        assert!(matches!(
            check(&p, "http://169.254.169.254/latest/"),
            Err(ViolationKind::NonPublicAddress { .. })
        ));
    }

    #[test]
    fn obfuscated_ipv4_literals_are_normalized() {
        // The WHATWG URL parser turns these into canonical IPv4 hosts.
        let p = NetworkPolicy::strict();
        for weird in [
            "http://2130706433/",
            "http://0x7f.1/",
            "http://0177.0.0.1/",
            "http://127.1/",
            "http://0x7f000001/",
        ] {
            let url = Url::parse(weird).unwrap();
            assert_eq!(url.host_str(), Some("127.0.0.1"), "{weird}");
            assert!(
                matches!(
                    check(&p, weird),
                    Err(ViolationKind::NonPublicAddress { .. })
                ),
                "{weird}"
            );
        }
    }

    #[test]
    fn allow_and_deny_lists() {
        let p = NetworkPolicy::strict()
            .allow_private_hosts(["NAS.local", "127.0.0.1", "[::1]"])
            .deny_hosts(["evil.example"]);
        assert!(check(&p, "http://nas.local/feed").is_ok());
        assert!(check(&p, "http://127.0.0.1:9000/feed").is_ok());
        assert!(check(&p, "http://[::1]:9000/feed").is_ok());
        assert_eq!(
            check(&p, "https://evil.example/"),
            Err(ViolationKind::HostDenied)
        );
        assert!(
            p.check_addr("nas.local", "192.168.1.5".parse().unwrap())
                .is_ok()
        );
        assert!(
            p.check_addr("other.example", "192.168.1.5".parse().unwrap())
                .is_err()
        );
        assert!(
            p.check_addr("other.example", "93.184.216.34".parse().unwrap())
                .is_ok()
        );
    }

    #[test]
    fn trusted_policy_allows_everything() {
        let p = NetworkPolicy::trusted();
        assert!(check(&p, "http://127.0.0.1:8484/api").is_ok());
        assert!(check(&p, "http://localhost:22/").is_ok());
        assert!(p.check_addr("x", "10.0.0.1".parse().unwrap()).is_ok());
        assert_eq!(
            check(&p, "gopher://x/"),
            Err(ViolationKind::SchemeNotAllowed("gopher".into()))
        );
    }

    #[test]
    fn custom_port_set() {
        let p = NetworkPolicy::strict().allowed_ports([443]);
        assert!(
            check(&p, "https://example.com/").is_ok(),
            "default port is not in url.port()"
        );
        assert_eq!(
            check(&p, "https://example.com:8443/"),
            Err(ViolationKind::PortNotAllowed(8443))
        );
    }
}
