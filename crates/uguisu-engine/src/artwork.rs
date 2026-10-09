//! Fetching and storing podcast artwork (ADR 0026).
//!
//! Artwork comes from a URL in a feed, which is to say from a stranger,
//! and ends up embedded in files other people's players parse. It
//! therefore goes through exactly the same HTTP stack as everything else
//! untrusted — [`Profile::Artwork`](uguisu_http::Profile::Artwork), with
//! the SSRF policy, per-hop re-validation on redirects and a byte cap
//! applied before a byte is read. There is no second URL validator here,
//! because a second one is a second thing to get wrong.
//!
//! Storage is content-addressed: a file is named after its own hash, so
//! fetching a replacement can never destroy the image before it, and one
//! partial unique index — not a check that could race two fetches —
//! decides which of them is current.
//!
//! Fetching is explicit, or follows a refresh when `UGUISU_ARCHIVE_ARTWORK_FETCH`
//! is on (ADR 0047). That is off by default: installing a release must not
//! start making requests on its own.

use std::io::Write;

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uguisu_archive::image;
use uguisu_archive::layout;
use uguisu_archive::path::resolve_checked;
use uguisu_core::UguisuError;
use uguisu_core::archive::{ArchiveErrorKind, PodcastArtwork};
use uguisu_core::events::{Event, EventKind};
use uguisu_core::ids::{ArtworkId, PodcastId};
use uguisu_http::{Conditional, GetOptions};
use uguisu_storage::{artwork, events, podcasts};

use crate::Engine;
use crate::archive::archive_error;

/// What a fetch did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtworkOutcome {
    /// New bytes were stored and are now the podcast's artwork.
    Fetched(Box<PodcastArtwork>),
    /// The server said what Uguisu already holds is still current.
    Unchanged(ArtworkId),
    /// Nothing was fetched, and why.
    Skipped(&'static str),
}

impl ArtworkOutcome {
    /// The stable word the CLI and the API use.
    #[must_use]
    pub const fn state(&self) -> &'static str {
        match self {
            Self::Fetched(_) => "fetched",
            Self::Unchanged(_) => "unchanged",
            Self::Skipped(_) => "skipped",
        }
    }
}

impl Engine {
    /// The artwork a podcast currently uses, if it has any.
    pub async fn current_artwork(
        &self,
        podcast_id: PodcastId,
    ) -> Result<Option<PodcastArtwork>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(artwork::current(&mut reader, podcast_id).await?)
    }

    /// Every artwork stored for a podcast, newest first. Superseded images
    /// stay listed, because they are still on disk.
    pub async fn artwork_history(
        &self,
        podcast_id: PodcastId,
    ) -> Result<Vec<PodcastArtwork>, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(artwork::list_for_podcast(&mut reader, podcast_id).await?)
    }

    /// Fetches a podcast's artwork from the URL its feed gave.
    ///
    /// `force` skips the conditional request, which is how a user asks for
    /// a re-fetch after a server has been caching badly. Without it, a
    /// stored `ETag` or `Last-Modified` is sent and a `304` costs one
    /// round trip and no bytes.
    #[allow(clippy::too_many_lines)] // fetch, validate, store, record: one path
    pub async fn fetch_artwork(
        &self,
        podcast_id: PodcastId,
        force: bool,
    ) -> Result<ArtworkOutcome, UguisuError> {
        let mut reader = self.storage().reader().await?;
        let podcast = podcasts::get(&mut reader, podcast_id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: podcast_id.to_string(),
            })?;
        let current = artwork::current(&mut reader, podcast_id).await?;
        drop(reader);

        let Some(url) = podcast.artwork_url.clone() else {
            return Ok(ArtworkOutcome::Skipped("the feed names no artwork"));
        };
        let conditional = match (&current, force) {
            (Some(existing), false)
                if existing.etag.is_some() || existing.last_modified.is_some() =>
            {
                Some(Conditional {
                    etag: existing.etag.clone(),
                    last_modified: existing.last_modified.clone(),
                })
            }
            _ => None,
        };
        let options = GetOptions {
            max_bytes: Some(self.config().archive.artwork_max_bytes),
            conditional,
            ..GetOptions::default()
        };
        let response = match self.artwork_client().get_with(&url, &options).await {
            Ok(r) => r,
            Err(e) => {
                self.publish_artwork_failure(podcast_id, &url, e.kind())
                    .await?;
                // A refusal by the network policy is the policy's exit code
                // and error kind, as for a feed, not a network failure.
                return Err(if matches!(e, uguisu_http::HttpError::Policy(_)) {
                    UguisuError::BlockedByPolicy(e.to_string())
                } else {
                    UguisuError::Network {
                        kind: uguisu_core::feed::FetchErrorKind::NetworkError,
                        detail: e.to_string(),
                    }
                });
            }
        };

        if response.status.as_u16() == 304 {
            let Some(existing) = current else {
                return Ok(ArtworkOutcome::Skipped(
                    "the server says nothing changed, but nothing is stored",
                ));
            };
            let etag = header(&response, "etag");
            let last_modified = header(&response, "last-modified");
            let now = OffsetDateTime::now_utc();
            let event = Event::now(
                Some(podcast_id),
                None,
                EventKind::PodcastArtworkUnchanged {
                    artwork_id: existing.id,
                },
            );
            let mut tx = self.storage().begin().await?;
            artwork::touch(
                &mut tx,
                existing.id,
                etag.as_deref(),
                last_modified.as_deref(),
                now,
            )
            .await?;
            events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            self.bus().publish(std::slice::from_ref(&event));
            return Ok(ArtworkOutcome::Unchanged(existing.id));
        }
        if !response.status.is_success() {
            let reason = format!("http_{}", response.status.as_u16());
            self.publish_artwork_failure(podcast_id, &url, &reason)
                .await?;
            return Err(archive_error(
                ArchiveErrorKind::ArtworkInvalid,
                format!("{url} answered {}", response.status),
            ));
        }

        let declared = header(&response, "content-type");
        let format = match image::validate(declared.as_deref(), &response.body) {
            Ok(f) => f,
            Err(e) => {
                self.publish_artwork_failure(podcast_id, &url, e.reason())
                    .await?;
                return Err(archive_error(
                    ArchiveErrorKind::ArtworkInvalid,
                    e.to_string(),
                ));
            }
        };
        let hash = hex::encode(Sha256::digest(&response.body));

        // Content-addressed: the same bytes are the same file and the same
        // row, so a re-fetch of unchanged artwork writes nothing new.
        let relative = layout::artwork_path(podcast_id, &hash, format);
        let root = self.media_root()?;
        let full = resolve_checked(&root, &relative)
            .map_err(|e| archive_error(ArchiveErrorKind::PathInvalid, e.to_string()))?;
        if !full.is_file() {
            write_atomic(&full, &response.body)?;
        }

        let now = OffsetDateTime::now_utc();
        let record = PodcastArtwork {
            id: current
                .as_ref()
                .filter(|a| a.hash_value == hash)
                .map_or_else(ArtworkId::new, |a| a.id),
            podcast_id,
            source_url: Some(url),
            relative_path: relative.as_str().to_owned(),
            format,
            content_type: declared,
            size_bytes: response.body.len() as u64,
            hash_algo: "sha256".to_owned(),
            hash_value: hash,
            etag: header(&response, "etag"),
            last_modified: header(&response, "last-modified"),
            is_current: true,
            retrieved_at: now,
            created_at: now,
            updated_at: now,
        };
        let event = Event::now(
            Some(podcast_id),
            None,
            EventKind::PodcastArtworkFetched {
                artwork_id: record.id,
                path: record.relative_path.clone(),
                format,
                size_bytes: record.size_bytes,
                hash_value: record.hash_value.clone(),
            },
        );
        let mut tx = self.storage().begin().await?;
        // Demote first: the partial unique index allows exactly one
        // current row, and this is the whole reason it exists.
        artwork::clear_current(&mut tx, podcast_id, now).await?;
        artwork::upsert_current(&mut tx, &record).await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        Ok(ArtworkOutcome::Fetched(Box::new(record)))
    }

    async fn publish_artwork_failure(
        &self,
        podcast_id: PodcastId,
        url: &url::Url,
        reason: &str,
    ) -> Result<(), UguisuError> {
        let event = Event::now(
            Some(podcast_id),
            None,
            EventKind::PodcastArtworkFailed {
                url: url.clone(),
                reason: reason.to_owned(),
            },
        );
        let mut tx = self.storage().begin().await?;
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        Ok(())
    }
}

fn header(response: &uguisu_http::Response, name: &str) -> Option<String> {
    response
        .headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned)
}

/// Writes a file through a temporary name, so the final path never holds a
/// partial image.
fn write_atomic(full: &std::path::Path, body: &[u8]) -> Result<(), UguisuError> {
    let parent = full.parent().ok_or_else(|| {
        archive_error(
            ArchiveErrorKind::PathInvalid,
            format!("{} has no directory", full.display()),
        )
    })?;
    let io = |e: std::io::Error| {
        archive_error(
            ArchiveErrorKind::VerificationIo,
            format!("{}: {e}", full.display()),
        )
    };
    std::fs::create_dir_all(parent).map_err(io)?;
    let mut tmp = full.to_path_buf().into_os_string();
    tmp.push(layout::tmp_suffix());
    let tmp = std::path::PathBuf::from(tmp);
    let _held = layout::Scratch::hold(&tmp);
    {
        let mut file = std::fs::File::create(&tmp).map_err(io)?;
        file.write_all(body).map_err(io)?;
        file.flush().map_err(io)?;
        file.sync_all().map_err(io)?;
    }
    std::fs::rename(&tmp, full).map_err(io)?;
    if let Ok(dir) = std::fs::File::open(parent) {
        drop(dir.sync_all());
    }
    Ok(())
}
