//! The discovery cache: memory in front, and optionally a store behind
//! it that outlives the process (ADR 0030).
//!
//! Keys are `(provider, kind, normalized key)`; the value carries its own
//! TTL so a provider's `Cache-Control` can shorten the configured default
//! (Podcast Index terms, ADR 0005).
//!
//! The second tier is a *port*: this crate must not depend on storage
//! (ADR 0001), so it describes what it needs and the engine supplies it —
//! the same shape as the download queue's `EventSink`. In memory an entry
//! keeps a monotonic `Instant`, which a row cannot; the store therefore
//! speaks wall-clock times and this module converts.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use moka::Expiry;
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::provider::ProviderId;

use crate::candidate::PodcastCandidate;

/// Bump when the candidate model changes incompatibly so stale entries are ignored.
const SCHEMA_VERSION: u32 = 1;

/// What kind of call produced an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheKind {
    /// A search.
    Search,
    /// A lookup.
    Lookup,
}

impl CacheKind {
    /// Stable string form (also the stored value).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Lookup => "lookup",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "search" => Some(Self::Search),
            "lookup" => Some(Self::Lookup),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    version: u32,
    provider: ProviderId,
    kind: CacheKind,
    key: String,
}

/// A cached provider result.
///
/// `SCHEMA_VERSION` is part of the stored key, so a change to this shape
/// makes older rows unreachable rather than mis-read.
/// Adjacently tagged, not internally: an internal tag cannot carry a
/// variant whose payload is a sequence, and `Search` is a list — serde
/// would have failed at run time, on the write, silently.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CachedValue {
    /// Search results.
    Search(Vec<PodcastCandidate>),
    /// A lookup result (possibly "not found"); boxed to keep the enum small.
    Lookup(Box<Option<PodcastCandidate>>),
}

#[derive(Debug, Clone)]
struct Entry {
    value: CachedValue,
    ttl: Duration,
    stored_at: Instant,
}

/// The second cache tier: somewhere a row can outlive the process.
///
/// Implemented by the engine over `discovery_cache`. Every method is
/// allowed to fail silently — a cache that cannot be read is a cache
/// miss, and a cache that cannot be written is a provider call next time.
/// Nothing here may ever make a search fail.
#[async_trait]
pub trait CacheStore: Send + Sync + std::fmt::Debug {
    /// The stored value, if one is there and has not expired.
    async fn get(&self, provider: ProviderId, kind: CacheKind, key: &str) -> Option<StoredEntry>;

    /// Stores a value until `expires_at`.
    async fn put(
        &self,
        provider: ProviderId,
        kind: CacheKind,
        key: &str,
        value: &CachedValue,
        fetched_at: OffsetDateTime,
        expires_at: OffsetDateTime,
    );
}

/// What the store hands back.
#[derive(Debug, Clone)]
pub struct StoredEntry {
    /// The value.
    pub value: CachedValue,
    /// When it stops being usable.
    pub expires_at: OffsetDateTime,
}

struct PerEntryTtl;

impl Expiry<CacheKey, Entry> for PerEntryTtl {
    fn expire_after_create(
        &self,
        _key: &CacheKey,
        value: &Entry,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(value.ttl)
    }
}

/// Hit/miss counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CacheStats {
    /// Cache hits.
    pub hits: u64,
    /// Cache misses.
    pub misses: u64,
    /// Entries currently held (approximate).
    pub entries: u64,
}

/// The discovery cache.
#[derive(Clone)]
pub struct DiscoveryCache {
    inner: Cache<CacheKey, Entry>,
    store: Option<Arc<dyn CacheStore>>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
}

impl std::fmt::Debug for DiscoveryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryCache")
            .field("entries", &self.inner.entry_count())
            .finish_non_exhaustive()
    }
}

impl DiscoveryCache {
    /// Creates a cache holding at most `max_entries`.
    pub fn new(max_entries: u64) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(max_entries.max(1))
                .expire_after(PerEntryTtl)
                .build(),
            store: None,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The same cache with a second tier behind it.
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn CacheStore>) -> Self {
        self.store = Some(store);
        self
    }

    fn key(provider: ProviderId, kind: CacheKind, key: &str) -> CacheKey {
        CacheKey {
            version: SCHEMA_VERSION,
            provider,
            kind,
            key: key.to_owned(),
        }
    }

    /// Returns a cached value that has not expired.
    ///
    /// Memory first; then the store, if there is one, whose answer is put
    /// back in memory so the next call is local again. A hit is counted
    /// once, wherever it came from.
    pub async fn get(
        &self,
        provider: ProviderId,
        kind: CacheKind,
        key: &str,
    ) -> Option<CachedValue> {
        let entry = self.inner.get(&Self::key(provider, kind, key)).await;
        if let Some(e) = entry
            && e.stored_at.elapsed() < e.ttl
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(e.value);
        }
        if let Some(store) = &self.store
            && let Some(stored) = store.get(provider, kind, key).await
        {
            let remaining = stored.expires_at - OffsetDateTime::now_utc();
            if let Ok(ttl) = Duration::try_from(remaining) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                self.inner
                    .insert(
                        Self::key(provider, kind, key),
                        Entry {
                            value: stored.value.clone(),
                            ttl,
                            stored_at: Instant::now(),
                        },
                    )
                    .await;
                return Some(stored.value);
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Stores a value. A zero TTL stores nothing.
    pub async fn put(
        &self,
        provider: ProviderId,
        kind: CacheKind,
        key: &str,
        value: CachedValue,
        ttl: Duration,
    ) {
        if ttl.is_zero() {
            return;
        }
        if let Some(store) = &self.store {
            let now = OffsetDateTime::now_utc();
            let expires_at = time::Duration::try_from(ttl)
                .ok()
                .and_then(|d| now.checked_add(d));
            if let Some(expires_at) = expires_at {
                store
                    .put(provider, kind, key, &value, now, expires_at)
                    .await;
            }
        }
        self.inner
            .insert(
                Self::key(provider, kind, key),
                Entry {
                    value,
                    ttl,
                    stored_at: Instant::now(),
                },
            )
            .await;
    }

    /// Drops every entry of one provider.
    pub async fn invalidate_provider(&self, provider: ProviderId) {
        let keys: Vec<CacheKey> = self
            .inner
            .iter()
            .filter(|(k, _)| k.provider == provider)
            .map(|(k, _)| (*k).clone())
            .collect();
        for k in keys {
            self.inner.invalidate(&k).await;
        }
    }

    /// Drops everything.
    pub fn invalidate_all(&self) {
        self.inner.invalidate_all();
    }

    /// Counters.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries: self.inner.entry_count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_and_expires_per_entry() {
        let cache = DiscoveryCache::new(10);
        assert!(
            cache
                .get(ProviderId::APPLE, CacheKind::Search, "x")
                .await
                .is_none()
        );
        cache
            .put(
                ProviderId::APPLE,
                CacheKind::Search,
                "x",
                CachedValue::Search(vec![]),
                Duration::from_millis(50),
            )
            .await;
        cache
            .put(
                ProviderId::APPLE,
                CacheKind::Search,
                "y",
                CachedValue::Search(vec![]),
                Duration::from_secs(60),
            )
            .await;
        assert!(
            cache
                .get(ProviderId::APPLE, CacheKind::Search, "x")
                .await
                .is_some()
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            cache
                .get(ProviderId::APPLE, CacheKind::Search, "x")
                .await
                .is_none(),
            "short ttl expired"
        );
        assert!(
            cache
                .get(ProviderId::APPLE, CacheKind::Search, "y")
                .await
                .is_some()
        );
        let s = cache.stats();
        assert_eq!((s.hits, s.misses), (2, 2));
    }

    #[tokio::test]
    async fn keys_are_provider_and_kind_scoped() {
        let cache = DiscoveryCache::new(10);
        cache
            .put(
                ProviderId::APPLE,
                CacheKind::Search,
                "q",
                CachedValue::Search(vec![]),
                Duration::from_secs(60),
            )
            .await;
        assert!(
            cache
                .get(ProviderId::PODCAST_INDEX, CacheKind::Search, "q")
                .await
                .is_none()
        );
        assert!(
            cache
                .get(ProviderId::APPLE, CacheKind::Lookup, "q")
                .await
                .is_none()
        );
        cache.inner.run_pending_tasks().await;
        cache.invalidate_provider(ProviderId::APPLE).await;
        assert!(
            cache
                .get(ProviderId::APPLE, CacheKind::Search, "q")
                .await
                .is_none()
        );
        cache
            .put(
                ProviderId::APPLE,
                CacheKind::Search,
                "z",
                CachedValue::Search(vec![]),
                Duration::ZERO,
            )
            .await;
        assert!(
            cache
                .get(ProviderId::APPLE, CacheKind::Search, "z")
                .await
                .is_none(),
            "zero ttl stores nothing"
        );
    }
}
