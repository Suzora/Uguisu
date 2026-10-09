//! DNS resolver hook that only returns policy-approved addresses.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::policy::{NetworkPolicy, PolicyViolation, ViolationKind};

/// Resolves names with the system resolver and drops every address the
/// policy refuses. When nothing remains the connection fails with a
/// [`PolicyViolation`], so a name that rebinds to a private address between
/// validation and connection still cannot be reached.
#[derive(Debug, Clone)]
pub struct SafeResolver {
    policy: Arc<NetworkPolicy>,
}

impl SafeResolver {
    /// Creates a resolver enforcing `policy`.
    pub fn new(policy: Arc<NetworkPolicy>) -> Self {
        Self { policy }
    }

    /// Resolves `host` and returns the addresses allowed by the policy.
    pub async fn resolve_allowed(&self, host: &str) -> Result<Vec<IpAddr>, ResolveError> {
        let addrs = tokio::net::lookup_host((host, 0))
            .await
            .map_err(|e| ResolveError::Lookup(e.to_string()))?
            .map(|sa| sa.ip())
            .collect::<Vec<_>>();
        if addrs.is_empty() {
            return Err(ResolveError::Lookup("no addresses returned".to_owned()));
        }
        let mut allowed = Vec::with_capacity(addrs.len());
        let mut first_violation = None;
        for ip in addrs {
            match self.policy.check_addr(host, ip) {
                Ok(()) => allowed.push(ip),
                Err(v) => {
                    if first_violation.is_none() {
                        first_violation = Some(v);
                    }
                }
            }
        }
        if allowed.is_empty() {
            let v = first_violation.unwrap_or_else(|| PolicyViolation {
                host: host.to_owned(),
                kind: ViolationKind::NoAllowedAddress,
            });
            tracing::warn!(host, violation = %v, "resolver refused every address");
            return Err(ResolveError::Policy(v));
        }
        Ok(allowed)
    }
}

/// Resolver failure.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ResolveError {
    /// The system resolver failed.
    #[error("lookup failed: {0}")]
    Lookup(String),
    /// Every address was refused by the policy.
    #[error(transparent)]
    Policy(#[from] PolicyViolation),
}

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let this = self.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            match this.resolve_allowed(&host).await {
                Ok(addrs) => {
                    let iter: Addrs = Box::new(addrs.into_iter().map(|ip| SocketAddr::new(ip, 0)));
                    Ok(iter)
                }
                Err(ResolveError::Policy(v)) => Err(Box::new(v) as _),
                Err(ResolveError::Lookup(msg)) => Err(Box::new(std::io::Error::other(msg)) as _),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn localhost_is_refused_unless_allowed() {
        let strict = SafeResolver::new(Arc::new(NetworkPolicy::strict()));
        assert!(matches!(
            strict.resolve_allowed("localhost").await,
            Err(ResolveError::Policy(_))
        ));

        let lenient = SafeResolver::new(Arc::new(
            NetworkPolicy::strict().allow_private_hosts(["localhost"]),
        ));
        let addrs = lenient.resolve_allowed("localhost").await.expect("allowed");
        assert!(addrs.iter().all(IpAddr::is_loopback));
    }

    #[tokio::test]
    async fn trusted_policy_passes_everything() {
        let trusted = SafeResolver::new(Arc::new(NetworkPolicy::trusted()));
        assert!(trusted.resolve_allowed("localhost").await.is_ok());
    }
}
