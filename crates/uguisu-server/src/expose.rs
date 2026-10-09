//! Whether a bind address may be served without a credential (ADR 0037).
//!
//! The predicate and the refusal live here rather than in the CLI so that no
//! caller of [`crate::serve_with_shutdown`] can start a server this rule would
//! have stopped. The message and the exit code are the CLI's, because they are
//! what a person reads.

use std::net::{IpAddr, SocketAddr};

/// Why a server did not start.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// A network address with no credential to protect it.
    #[error(
        "refusing to bind {bind} without authentication: anything that can reach \
         this address could read the whole archive and change every setting"
    )]
    InsecureExposure {
        /// The address that was asked for.
        bind: SocketAddr,
    },
    /// The socket could not be bound, or serving stopped badly.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Whether `addr` can only be reached from this machine.
///
/// `to_canonical` is what makes `::ffff:127.0.0.1` — the IPv4-mapped form a
/// dual-stack listener sees — count as loopback. `0.0.0.0` and `::` are
/// deliberately not: they are every interface, which is the case this exists
/// for.
#[must_use]
pub fn loopback(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.to_canonical().is_loopback(),
    }
}

/// Refuses a bind that would expose an unauthenticated server.
///
/// Authentication is optional on loopback, because a fresh install has no
/// credential and has to be usable before it gets one. Off loopback it is
/// required — and this is a startup failure rather than a warning, because a
/// warning in a log nobody reads is how an archive ends up on the open
/// internet. `allow_insecure` is the deliberate way past it.
pub fn check(
    bind: SocketAddr,
    credential_set: bool,
    allow_insecure: bool,
) -> Result<(), ServeError> {
    if credential_set || loopback(bind) || allow_insecure {
        return Ok(());
    }
    Err(ServeError::InsecureExposure { bind })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)))
    }

    #[test]
    fn only_this_machine_counts_as_loopback() {
        for reachable_here in [
            "127.0.0.1:8484",
            "127.0.0.53:8484",
            "[::1]:8484",
            "[::ffff:127.0.0.1]:8484",
        ] {
            assert!(loopback(addr(reachable_here)), "{reachable_here}");
        }
        for elsewhere in [
            "0.0.0.0:8484",
            "[::]:8484",
            "192.168.1.10:8484",
            "10.0.0.5:8484",
            "203.0.113.7:8484",
        ] {
            assert!(!loopback(addr(elsewhere)), "{elsewhere}");
        }
    }

    #[test]
    fn loopback_needs_no_credential() {
        assert!(check(addr("127.0.0.1:8484"), false, false).is_ok());
        assert!(check(addr("[::1]:8484"), false, false).is_ok());
    }

    #[test]
    fn a_network_address_needs_one() {
        for exposed in ["0.0.0.0:8484", "[::]:8484", "192.168.1.10:8484"] {
            let refused = check(addr(exposed), false, false);
            assert!(refused.is_err(), "{exposed}");
            assert!(
                refused.unwrap_err().to_string().contains(exposed),
                "the refusal has to name the address asked for"
            );
            assert!(
                check(addr(exposed), true, false).is_ok(),
                "{exposed} with a credential"
            );
            assert!(
                check(addr(exposed), false, true).is_ok(),
                "{exposed} with the override"
            );
        }
    }
}
