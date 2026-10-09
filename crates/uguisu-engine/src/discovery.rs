//! Discovery that outlives the process (ADR 0030): the second cache tier
//! and the record of what each resolution decided.
//!
//! `uguisu-discovery` may not depend on storage (ADR 0001), so it names
//! what it needs — [`CacheStore`] — and this is where that port is
//! implemented over the database. Everything here is derived or
//! historical: losing the cache costs a round trip, losing a record costs
//! an entry in a list nobody's library depends on.

use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uguisu_core::UguisuError;
use uguisu_core::ids::{DiscoveryRecordId, PodcastId};
use uguisu_core::provider::{DiscoveryRecord, ProviderId, ResolutionOutcome};
use uguisu_discovery::cache::{CacheKind, CacheStore, CachedValue, StoredEntry};
use uguisu_discovery::resolve::{ResolveFailure, ResolvedFeed};
use uguisu_storage::{Storage, discovery as discovery_repo};

use crate::Engine;

/// The `discovery_cache` table, as the discovery crate's port.
#[derive(Clone)]
pub struct SqliteCacheStore {
    storage: Storage,
}

impl std::fmt::Debug for SqliteCacheStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteCacheStore").finish_non_exhaustive()
    }
}

impl SqliteCacheStore {
    /// A store over this database.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl CacheStore for SqliteCacheStore {
    /// A row that cannot be read or does not parse is a cache miss, and a
    /// cache miss is a provider call. Nothing here may make a search fail.
    async fn get(&self, provider: ProviderId, kind: CacheKind, key: &str) -> Option<StoredEntry> {
        let mut reader = self.storage.reader().await.ok()?;
        let row = discovery_repo::get_cache(&mut reader, provider.as_str(), kind.as_str(), key)
            .await
            .ok()
            .flatten()?;
        if row.expires_at <= OffsetDateTime::now_utc() {
            return None;
        }
        match serde_json::from_str::<CachedValue>(&row.payload) {
            Ok(value) => Some(StoredEntry {
                value,
                expires_at: row.expires_at,
            }),
            Err(e) => {
                // A payload written by an older shape of the model. The
                // row expires on its own; reporting it is enough.
                tracing::debug!(provider = provider.as_str(), error = %e, "cached payload ignored");
                None
            }
        }
    }

    async fn put(
        &self,
        provider: ProviderId,
        kind: CacheKind,
        key: &str,
        value: &CachedValue,
        fetched_at: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) {
        let payload = match serde_json::to_string(value) {
            Ok(payload) => payload,
            Err(e) => {
                tracing::warn!(provider = provider.as_str(), error = %e, "cached value not serialisable");
                return;
            }
        };
        let write = async {
            let mut tx = self.storage.begin().await?;
            discovery_repo::put_cache(
                &mut tx,
                provider.as_str(),
                kind.as_str(),
                key,
                &payload,
                fetched_at,
                expires_at,
            )
            .await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            Ok::<(), uguisu_storage::StorageError>(())
        };
        if let Err(e) = write.await {
            tracing::debug!(provider = provider.as_str(), error = %e, "discovery cache not written");
        }
    }
}

impl Engine {
    /// Records what a resolution decided.
    ///
    /// Written outside the caller's transaction on purpose: provenance
    /// must never be able to fail an operation that otherwise succeeded,
    /// and a missing record is a gap in a list, not a broken library.
    pub(crate) async fn record_resolution(
        &self,
        input: &str,
        result: Result<&ResolvedFeed, &ResolveFailure>,
        podcast_id: Option<PodcastId>,
    ) -> DiscoveryRecordId {
        let now = OffsetDateTime::now_utc();
        let record = match result {
            Ok(feed) => DiscoveryRecord {
                id: DiscoveryRecordId::new(),
                input: input.to_owned(),
                provider: Some(ProviderId::WEBSITE.as_str().to_owned()),
                provider_ref: None,
                feed_url: Some(feed.feed_url.to_string()),
                website: feed.website.as_ref().map(ToString::to_string),
                status: ResolutionOutcome::Resolved,
                detail: feed.title.clone(),
                steps: feed
                    .provenance
                    .iter()
                    .map(|s| format!("{}: {}", s.kind.as_str(), s.detail))
                    .collect(),
                warnings: feed.warnings.clone(),
                podcast_id,
                resolved_at: feed.verified_at,
                created_at: now,
            },
            Err(failure) => DiscoveryRecord {
                id: DiscoveryRecordId::new(),
                input: input.to_owned(),
                provider: None,
                provider_ref: None,
                feed_url: None,
                website: None,
                status: ResolutionOutcome::Unresolved,
                detail: Some(failure.error.to_string()),
                steps: failure
                    .provenance
                    .iter()
                    .map(|s| format!("{}: {}", s.kind.as_str(), s.detail))
                    .collect(),
                warnings: Vec::new(),
                podcast_id: None,
                resolved_at: now,
                created_at: now,
            },
        };
        let id = record.id;
        if let Err(e) = self.write_record(&record).await {
            tracing::debug!(error = %e, "resolution not recorded");
        }
        id
    }

    async fn write_record(&self, record: &DiscoveryRecord) -> Result<(), UguisuError> {
        let mut tx = self.storage().begin().await?;
        discovery_repo::insert_record(&mut tx, record).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        Ok(())
    }

    /// Links a recorded resolution to the podcast it produced.
    pub(crate) async fn attach_resolution(&self, id: DiscoveryRecordId, podcast_id: PodcastId) {
        let attach = async {
            let mut tx = self.storage().begin().await?;
            discovery_repo::attach_podcast(&mut tx, id, podcast_id).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            Ok::<(), uguisu_storage::StorageError>(())
        };
        if let Err(e) = attach.await {
            tracing::debug!(error = %e, "resolution not linked to its podcast");
        }
    }

    /// The most recent resolutions, newest first.
    pub async fn discovery_records(&self, limit: u32) -> Result<Vec<DiscoveryRecord>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(discovery_repo::list_records(&mut reader, limit.clamp(1, 500)).await?)
    }

    /// One recorded resolution.
    pub async fn discovery_record(
        &self,
        id: DiscoveryRecordId,
    ) -> Result<DiscoveryRecord, UguisuError> {
        let mut reader = self.storage().reader().await?;
        discovery_repo::get_record(&mut reader, id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "discovery record".to_owned(),
                id: id.to_string(),
            })
    }

    /// The cache store this engine hands to the discovery stack.
    #[must_use]
    pub fn cache_store(&self) -> Arc<dyn CacheStore> {
        Arc::new(SqliteCacheStore::new(self.storage().clone()))
    }
}
