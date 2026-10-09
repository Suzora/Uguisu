//! Builds a complete discovery stack (registry, engine, resolver) from
//! configuration. Used by the CLI's embedded mode and by the server.

use std::sync::Arc;
use std::time::Duration;

use uguisu_core::config::DiscoveryConfig;
use uguisu_core::provider::ProviderId;
use uguisu_http::{ClientConfig, HttpClient, HttpError, Profile, RetryPolicy};

use crate::cache::DiscoveryCache;
use crate::provider::{DiscoveryProvider, ProviderError};
use crate::providers::apple::AppleProvider;
use crate::providers::gpoddernet::GpodderNetProvider;
use crate::providers::podcastindex::PodcastIndexProvider;
use crate::registry::{ProviderRegistry, RegistryConfig};
use crate::resolve::{Resolver, ResolverConfig};
use crate::search::SearchEngine;

/// A ready-to-use discovery stack.
#[derive(Debug, Clone)]
pub struct Discovery {
    /// Provider registry (status, cache).
    pub registry: Arc<ProviderRegistry>,
    /// Search engine.
    pub engine: SearchEngine,
    /// Feed resolver.
    pub resolver: Resolver,
    /// The configuration it was built from.
    pub config: DiscoveryConfig,
}

/// Why the stack could not be built.
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    /// HTTP client construction failed.
    #[error("http client: {0}")]
    Http(#[from] HttpError),
    /// A provider rejected its configuration.
    #[error("provider {0}: {1}")]
    Provider(ProviderId, ProviderError),
}

/// Builds the stack. Providers that are disabled or lack credentials are
/// registered with a visible reason so `providers` status explains them.
pub fn assemble(config: DiscoveryConfig) -> Result<Discovery, AssembleError> {
    assemble_with_cache_store(config, None)
}

/// [`assemble`] with a second cache tier behind the in-memory one
/// (ADR 0030). The engine passes the database; a frontend that has none
/// passes nothing and gets the in-memory behaviour.
pub fn assemble_with_cache_store(
    config: DiscoveryConfig,
    store: Option<Arc<dyn crate::cache::CacheStore>>,
) -> Result<Discovery, AssembleError> {
    let client_config = ClientConfig {
        connect_timeout: config.network.connect_timeout,
        request_timeout: config.network.request_timeout.min(
            config
                .search
                .provider_timeout
                .max(config.network.connect_timeout),
        ),
        ..ClientConfig::from_network(&config.network)
    };
    // Provider calls fail fast: one quick retry, and a `Retry-After` longer than two seconds is
    // reported as rate limiting so the registry's circuit breaker can back off instead of blocking a search.
    let provider_retry = RetryPolicy {
        max_attempts: 2,
        base: Duration::from_millis(300),
        cap: Duration::from_secs(2),
        max_retry_after: Duration::from_secs(2),
        jitter: true,
    };
    let discovery_client = HttpClient::new(
        Profile::Discovery,
        ClientConfig {
            retry: provider_retry,
            ..client_config.clone()
        },
    )?;
    let feed_client = HttpClient::new(
        Profile::Feed,
        ClientConfig {
            request_timeout: config.network.request_timeout,
            ..client_config
        },
    )?;

    let registry_config = RegistryConfig {
        call_timeout: config.search.provider_timeout,
        search_ttl: config.cache.search_ttl,
        lookup_ttl: config.cache.lookup_ttl,
        ..RegistryConfig::default()
    };
    let mut registry = ProviderRegistry::new(
        registry_config,
        match store {
            Some(store) => DiscoveryCache::new(config.cache.max_entries).with_store(store),
            None => DiscoveryCache::new(config.cache.max_entries),
        },
    );

    let apple = AppleProvider::new(discovery_client.clone(), &config.apple)
        .map_err(|e| AssembleError::Provider(ProviderId::APPLE, e))?;
    registry.register(
        Arc::new(apple) as Arc<dyn DiscoveryProvider>,
        (!config.apple.enabled).then(|| "disabled in configuration".to_owned()),
    );

    let pi = PodcastIndexProvider::new(discovery_client.clone(), &config.podcastindex)
        .map_err(|e| AssembleError::Provider(ProviderId::PODCAST_INDEX, e))?;
    let pi_reason = if !config.podcastindex.has_credentials() {
        Some(
            "no credentials (set UGUISU_PODCASTINDEX_KEY and UGUISU_PODCASTINDEX_SECRET)"
                .to_owned(),
        )
    } else if !config.podcastindex.enabled {
        Some("disabled in configuration".to_owned())
    } else {
        None
    };
    registry.register(Arc::new(pi) as Arc<dyn DiscoveryProvider>, pi_reason);

    let gp = GpodderNetProvider::new(discovery_client, &config.gpoddernet)
        .map_err(|e| AssembleError::Provider(ProviderId::GPODDER_NET, e))?;
    registry.register(
        Arc::new(gp) as Arc<dyn DiscoveryProvider>,
        (!config.gpoddernet.enabled).then(|| {
            "disabled in configuration (opt in with UGUISU_DISCOVERY_GPODDERNET_ENABLED=true)"
                .to_owned()
        }),
    );

    let registry = Arc::new(registry);
    let engine = SearchEngine::new(Arc::clone(&registry), config.search.clone());
    let resolver = Resolver::new(
        feed_client,
        Some(Arc::clone(&registry)),
        ResolverConfig::default(),
    );
    Ok(Discovery {
        registry,
        engine,
        resolver,
        config,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn default_config_enables_apple_only() {
        let d = assemble(DiscoveryConfig::default()).expect("assemble");
        assert_eq!(d.registry.enabled(), vec![ProviderId::APPLE]);
        let status = d.registry.status();
        assert_eq!(status.len(), 3);
        assert!(
            status
                .iter()
                .find(|s| s.id == ProviderId::PODCAST_INDEX)
                .and_then(|s| s.disabled_reason.as_deref())
                .is_some_and(|r| r.contains("credentials"))
        );
        assert!(
            status
                .iter()
                .find(|s| s.id == ProviderId::GPODDER_NET)
                .and_then(|s| s.disabled_reason.as_deref())
                .is_some_and(|r| r.contains("opt in"))
        );
    }

    #[test]
    fn credentials_enable_podcast_index() {
        let cfg = DiscoveryConfig::from_lookup(|k| match k {
            "UGUISU_PODCASTINDEX_KEY" => Some("k".into()),
            "UGUISU_PODCASTINDEX_SECRET" => Some("s".into()),
            "UGUISU_DISCOVERY_GPODDERNET_ENABLED" => Some("true".into()),
            _ => None,
        })
        .expect("config");
        let d = assemble(cfg).expect("assemble");
        assert_eq!(
            d.registry.enabled(),
            vec![
                ProviderId::APPLE,
                ProviderId::PODCAST_INDEX,
                ProviderId::GPODDER_NET
            ]
        );
    }
}
