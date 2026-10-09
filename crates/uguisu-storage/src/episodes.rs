//! `episodes`, `enclosures` and `episode_extras` repository.
//!
//! Upserts are keyed on `(podcast_id, identity_key)` (ADR 0006). Callers
//! resolve identities against [`index`] first and reuse the stored episode
//! id for existing episodes, so enclosure and extras rows always reference
//! the right row.

use std::collections::HashMap;

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::ids::{EnclosureId, EpisodeId, PodcastId};
use uguisu_core::model::{
    ArchiveState, DateQuality, Enclosure, EnclosureKind, Episode, EpisodeExtras, EpisodeIdentity,
    IdentitySource,
};
use url::Url;

use crate::row::{self, bool_from, i64_from_u64, opt_url, to_json, u32_from, u64_from};
use crate::{Result, to_db_ts};

const TABLE: &str = "episodes";
/// Rows per `IN (...)` chunk.
const CHUNK: usize = 500;

/// The light view of an episode used for identity matching and removal
/// detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeIndex {
    /// Episode id.
    pub id: EpisodeId,
    /// Identity key.
    pub identity_key: String,
    /// Identity source.
    pub identity_source: IdentitySource,
    /// Normalized GUID key, if any.
    pub guid_key: Option<String>,
    /// Normalized enclosure key, if any.
    pub enclosure_key: Option<String>,
    /// Fingerprint key, if any.
    pub fingerprint_key: Option<String>,
    /// Comparable hash.
    pub content_hash: String,
    /// Title.
    pub title: String,
    /// Missing streak.
    pub missing_streak: u32,
    /// Removal time, if detected.
    pub removed_from_feed_at: Option<OffsetDateTime>,
    /// Whether it is a candidate duplicate.
    pub duplicate_of_episode_id: Option<EpisodeId>,
    /// Whether the stored item was malformed.
    pub malformed: bool,
    /// When the episode was first stored (insert-only, carried on updates).
    pub first_seen_at: OffsetDateTime,
    /// Stored publication instant, for probable-duplicate matching.
    pub published_at: Option<OffsetDateTime>,
}

#[derive(FromRow)]
struct IndexRow {
    id: String,
    identity_key: String,
    identity_source: String,
    guid_key: Option<String>,
    enclosure_key: Option<String>,
    fingerprint_key: Option<String>,
    content_hash: String,
    title: String,
    missing_streak: i64,
    removed_from_feed_at: Option<String>,
    duplicate_of_episode_id: Option<String>,
    malformed: i64,
    first_seen_at: String,
    published_at: Option<String>,
}

#[derive(FromRow)]
struct EpisodeRow {
    id: String,
    podcast_id: String,
    guid: Option<String>,
    guid_is_permalink: Option<i64>,
    identity_key: String,
    identity_source: String,
    guid_key: Option<String>,
    enclosure_key: Option<String>,
    fingerprint_key: Option<String>,
    identity_reason: String,
    title: String,
    subtitle: Option<String>,
    sort_title: String,
    description_html: Option<String>,
    description_text: Option<String>,
    link: Option<String>,
    published_at: Option<String>,
    published_at_raw: Option<String>,
    published_at_quality: String,
    updated_at_source: Option<String>,
    duration_secs: Option<i64>,
    duration_raw: Option<String>,
    season: Option<i64>,
    episode_number: Option<i64>,
    episode_type: Option<String>,
    explicit: Option<i64>,
    artwork_url: Option<String>,
    author: Option<String>,
    content_hash: String,
    archive_state: String,
    skip_reason: Option<String>,
    malformed: i64,
    malformed_reason: Option<String>,
    duplicate_of_episode_id: Option<String>,
    duplicate_reasons: String,
    missing_streak: i64,
    first_seen_at: String,
    last_seen_in_feed_at: String,
    removed_from_feed_at: Option<String>,
    sort_at: String,
    source_metadata: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(FromRow)]
struct EnclosureRow {
    id: String,
    episode_id: String,
    url: String,
    mime_type: Option<String>,
    length_bytes: Option<i64>,
    is_primary: i64,
    kind: String,
    position: i64,
    bitrate: Option<i64>,
    height: Option<i64>,
    codecs: Option<String>,
    lang: Option<String>,
    title: Option<String>,
    integrity_type: Option<String>,
    integrity_value: Option<String>,
    sources: String,
}

impl EnclosureRow {
    fn into_model(self) -> Result<Enclosure> {
        let rid = self.id.as_str();
        Ok(Enclosure {
            id: row::id("enclosures", rid, rid)?,
            episode_id: row::id("enclosures", rid, &self.episode_id)?,
            url: row::req_url("enclosures", rid, &self.url)?,
            mime_type: self.mime_type.clone(),
            length_bytes: u64_from(self.length_bytes),
            is_primary: self.is_primary != 0,
            kind: row::parse_enum("enclosures", rid, "kind", &self.kind, EnclosureKind::parse)?,
            position: u32::try_from(self.position).unwrap_or(0),
            bitrate: u64_from(self.bitrate),
            height: u32_from(self.height),
            codecs: self.codecs.clone(),
            lang: self.lang.clone(),
            title: self.title.clone(),
            integrity_type: self.integrity_type.clone(),
            integrity_value: self.integrity_value.clone(),
            sources: row::json("enclosures", "sources", rid, &self.sources)?,
        })
    }
}

impl EpisodeRow {
    fn into_model(self, enclosures: Vec<Enclosure>, extras: EpisodeExtras) -> Result<Episode> {
        let rid = self.id.as_str();
        Ok(Episode {
            id: row::id(TABLE, rid, rid)?,
            podcast_id: row::id(TABLE, rid, &self.podcast_id)?,
            guid: self.guid.clone(),
            guid_is_permalink: bool_from(self.guid_is_permalink),
            identity: EpisodeIdentity {
                key: self.identity_key.clone(),
                source: row::parse_enum(
                    TABLE,
                    rid,
                    "identity_source",
                    &self.identity_source,
                    IdentitySource::parse,
                )?,
                guid_key: self.guid_key.clone(),
                enclosure_key: self.enclosure_key.clone(),
                fingerprint_key: self.fingerprint_key.clone(),
                reason: self.identity_reason.clone(),
            },
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            sort_title: self.sort_title.clone(),
            description_html: self.description_html.clone(),
            description_text: self.description_text.clone(),
            link: opt_url(self.link.as_deref()),
            published_at: row::opt_ts(TABLE, rid, self.published_at.as_deref())?,
            published_at_raw: self.published_at_raw.clone(),
            published_at_quality: row::parse_enum(
                TABLE,
                rid,
                "published_at_quality",
                &self.published_at_quality,
                DateQuality::parse,
            )?,
            updated_at_source: row::opt_ts(TABLE, rid, self.updated_at_source.as_deref())?,
            duration_secs: u32_from(self.duration_secs),
            duration_raw: self.duration_raw.clone(),
            season: u32_from(self.season),
            episode_number: u32_from(self.episode_number),
            episode_type: self.episode_type.clone(),
            explicit: bool_from(self.explicit),
            artwork_url: opt_url(self.artwork_url.as_deref()),
            author: self.author.clone(),
            content_hash: self.content_hash.clone(),
            archive_state: row::parse_enum(
                TABLE,
                rid,
                "archive_state",
                &self.archive_state,
                ArchiveState::parse,
            )?,
            skip_reason: self.skip_reason.clone(),
            malformed: self.malformed != 0,
            malformed_reason: self.malformed_reason.clone(),
            duplicate_of_episode_id: row::opt_id(
                TABLE,
                rid,
                self.duplicate_of_episode_id.as_deref(),
            )?,
            duplicate_reasons: row::json(TABLE, "duplicate_reasons", rid, &self.duplicate_reasons)?,
            missing_streak: u32::try_from(self.missing_streak).unwrap_or(0),
            first_seen_at: row::ts(TABLE, rid, &self.first_seen_at)?,
            last_seen_in_feed_at: row::ts(TABLE, rid, &self.last_seen_in_feed_at)?,
            removed_from_feed_at: row::opt_ts(TABLE, rid, self.removed_from_feed_at.as_deref())?,
            sort_at: row::ts(TABLE, rid, &self.sort_at)?,
            source_metadata: row::opt_json(
                TABLE,
                "source_metadata",
                rid,
                self.source_metadata.as_deref(),
            )?,
            enclosures,
            extras,
            created_at: row::ts(TABLE, rid, &self.created_at)?,
            updated_at: row::ts(TABLE, rid, &self.updated_at)?,
        })
    }
}

const COLUMNS: &str = "id, podcast_id, guid, guid_is_permalink, identity_key, identity_source, guid_key, \
    enclosure_key, fingerprint_key, identity_reason, title, subtitle, sort_title, description_html, \
    description_text, link, published_at, published_at_raw, published_at_quality, updated_at_source, \
    duration_secs, duration_raw, season, episode_number, episode_type, explicit, artwork_url, author, \
    content_hash, archive_state, skip_reason, malformed, malformed_reason, duplicate_of_episode_id, \
    duplicate_reasons, missing_streak, first_seen_at, last_seen_in_feed_at, removed_from_feed_at, sort_at, \
    source_metadata, created_at, updated_at";

const ENCLOSURE_COLUMNS: &str = "id, episode_id, url, mime_type, length_bytes, is_primary, kind, position, \
    bitrate, height, codecs, lang, title, integrity_type, integrity_value, sources";

/// Inserts or updates episodes by `(podcast_id, identity_key)`, then
/// synchronizes their enclosures and extras. `archive_state`,
/// `skip_reason`, `duplicate_of_episode_id` and `duplicate_reasons` are
/// written on insert only: the archive engine owns the first two, a
/// resolution (ADR 0051) the last two.
pub async fn upsert_all(conn: &mut SqliteConnection, episodes: &[Episode]) -> Result<()> {
    for e in episodes {
        sqlx::query(&format!(
            "INSERT INTO episodes ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
             ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, \
             ?38, ?39, ?40, ?41, ?42, ?43) \
             ON CONFLICT(podcast_id, identity_key) DO UPDATE SET \
             guid = excluded.guid, guid_is_permalink = excluded.guid_is_permalink, \
             identity_source = excluded.identity_source, guid_key = excluded.guid_key, \
             enclosure_key = excluded.enclosure_key, fingerprint_key = excluded.fingerprint_key, \
             identity_reason = excluded.identity_reason, title = excluded.title, subtitle = excluded.subtitle, \
             sort_title = excluded.sort_title, description_html = excluded.description_html, \
             description_text = excluded.description_text, link = excluded.link, published_at = excluded.published_at, \
             published_at_raw = excluded.published_at_raw, published_at_quality = excluded.published_at_quality, \
             updated_at_source = excluded.updated_at_source, duration_secs = excluded.duration_secs, \
             duration_raw = excluded.duration_raw, season = excluded.season, episode_number = excluded.episode_number, \
             episode_type = excluded.episode_type, explicit = excluded.explicit, artwork_url = excluded.artwork_url, \
             author = excluded.author, content_hash = excluded.content_hash, malformed = excluded.malformed, \
             malformed_reason = excluded.malformed_reason, missing_streak = excluded.missing_streak, \
             last_seen_in_feed_at = excluded.last_seen_in_feed_at, removed_from_feed_at = excluded.removed_from_feed_at, \
             sort_at = excluded.sort_at, source_metadata = excluded.source_metadata, updated_at = excluded.updated_at"
        ))
        .bind(e.id.to_string())
        .bind(e.podcast_id.to_string())
        .bind(&e.guid)
        .bind(e.guid_is_permalink.map(i64::from))
        .bind(&e.identity.key)
        .bind(e.identity.source.as_str())
        .bind(&e.identity.guid_key)
        .bind(&e.identity.enclosure_key)
        .bind(&e.identity.fingerprint_key)
        .bind(&e.identity.reason)
        .bind(&e.title)
        .bind(&e.subtitle)
        .bind(&e.sort_title)
        .bind(&e.description_html)
        .bind(&e.description_text)
        .bind(e.link.as_ref().map(Url::as_str))
        .bind(e.published_at.map(to_db_ts))
        .bind(&e.published_at_raw)
        .bind(e.published_at_quality.as_str())
        .bind(e.updated_at_source.map(to_db_ts))
        .bind(e.duration_secs.map(i64::from))
        .bind(&e.duration_raw)
        .bind(e.season.map(i64::from))
        .bind(e.episode_number.map(i64::from))
        .bind(&e.episode_type)
        .bind(e.explicit.map(i64::from))
        .bind(e.artwork_url.as_ref().map(Url::as_str))
        .bind(&e.author)
        .bind(&e.content_hash)
        .bind(e.archive_state.as_str())
        .bind(&e.skip_reason)
        .bind(i64::from(e.malformed))
        .bind(&e.malformed_reason)
        .bind(e.duplicate_of_episode_id.map(|i| i.to_string()))
        .bind(to_json(&e.duplicate_reasons))
        .bind(i64::from(e.missing_streak))
        .bind(to_db_ts(e.first_seen_at))
        .bind(to_db_ts(e.last_seen_in_feed_at))
        .bind(e.removed_from_feed_at.map(to_db_ts))
        .bind(to_db_ts(e.sort_at))
        .bind(e.source_metadata.as_ref().map(to_json))
        .bind(to_db_ts(e.created_at))
        .bind(to_db_ts(e.updated_at))
        .execute(&mut *conn)
        .await?;
        sync_enclosures(conn, e).await?;
        sync_extras(conn, e).await?;
    }
    Ok(())
}

async fn sync_enclosures(conn: &mut SqliteConnection, e: &Episode) -> Result<()> {
    for enc in &e.enclosures {
        sqlx::query(&format!(
            "INSERT INTO enclosures ({ENCLOSURE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
             ON CONFLICT(episode_id, url) DO UPDATE SET mime_type = excluded.mime_type, length_bytes = excluded.length_bytes, \
             is_primary = excluded.is_primary, kind = excluded.kind, position = excluded.position, bitrate = excluded.bitrate, \
             height = excluded.height, codecs = excluded.codecs, lang = excluded.lang, title = excluded.title, \
             integrity_type = excluded.integrity_type, integrity_value = excluded.integrity_value, sources = excluded.sources"
        ))
        .bind(enc.id.to_string())
        .bind(e.id.to_string())
        .bind(enc.url.as_str())
        .bind(&enc.mime_type)
        .bind(enc.length_bytes.map(i64_from_u64))
        .bind(i64::from(enc.is_primary))
        .bind(enc.kind.as_str())
        .bind(i64::from(enc.position))
        .bind(enc.bitrate.map(i64_from_u64))
        .bind(enc.height.map(i64::from))
        .bind(&enc.codecs)
        .bind(&enc.lang)
        .bind(&enc.title)
        .bind(&enc.integrity_type)
        .bind(&enc.integrity_value)
        .bind(to_json(&enc.sources))
        .execute(&mut *conn)
        .await?;
    }
    // Remove enclosures that vanished from the item.
    let keep: Vec<String> = e.enclosures.iter().map(|x| x.url.to_string()).collect();
    let placeholders = if keep.is_empty() {
        "''".to_owned()
    } else {
        (0..keep.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let sql =
        format!("DELETE FROM enclosures WHERE episode_id = ?1 AND url NOT IN ({placeholders})");
    let mut q = sqlx::query(&sql).bind(e.id.to_string());
    for url in &keep {
        q = q.bind(url);
    }
    q.execute(conn).await?;
    Ok(())
}

async fn sync_extras(conn: &mut SqliteConnection, e: &Episode) -> Result<()> {
    if e.extras.is_empty() {
        sqlx::query("DELETE FROM episode_extras WHERE episode_id = ?1")
            .bind(e.id.to_string())
            .execute(conn)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO episode_extras (episode_id, data) VALUES (?1, ?2) \
             ON CONFLICT(episode_id) DO UPDATE SET data = excluded.data",
        )
        .bind(e.id.to_string())
        .bind(to_json(&e.extras))
        .execute(conn)
        .await?;
    }
    Ok(())
}

/// The identity index of a podcast (every stored episode, light form).
pub async fn index(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
) -> Result<Vec<EpisodeIndex>> {
    let rows: Vec<IndexRow> = sqlx::query_as(
        "SELECT id, identity_key, identity_source, guid_key, enclosure_key, fingerprint_key, content_hash, title, \
         missing_streak, removed_from_feed_at, duplicate_of_episode_id, malformed, first_seen_at, published_at \
         FROM episodes WHERE podcast_id = ?1",
    )
    .bind(podcast_id.to_string())
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|r| {
            let rid = r.id.as_str();
            Ok(EpisodeIndex {
                id: row::id(TABLE, rid, rid)?,
                identity_key: r.identity_key.clone(),
                identity_source: row::parse_enum(
                    TABLE,
                    rid,
                    "identity_source",
                    &r.identity_source,
                    IdentitySource::parse,
                )?,
                guid_key: r.guid_key.clone(),
                enclosure_key: r.enclosure_key.clone(),
                fingerprint_key: r.fingerprint_key.clone(),
                content_hash: r.content_hash.clone(),
                title: r.title.clone(),
                missing_streak: u32::try_from(r.missing_streak).unwrap_or(0),
                removed_from_feed_at: row::opt_ts(TABLE, rid, r.removed_from_feed_at.as_deref())?,
                duplicate_of_episode_id: row::opt_id(
                    TABLE,
                    rid,
                    r.duplicate_of_episode_id.as_deref(),
                )?,
                malformed: r.malformed != 0,
                first_seen_at: row::ts(TABLE, rid, &r.first_seen_at)?,
                published_at: row::opt_ts(TABLE, rid, r.published_at.as_deref())?,
            })
        })
        .collect()
}

async fn attach(conn: &mut SqliteConnection, rows: Vec<EpisodeRow>) -> Result<Vec<Episode>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut enclosures: HashMap<String, Vec<Enclosure>> = HashMap::new();
    let mut extras: HashMap<String, EpisodeExtras> = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {ENCLOSURE_COLUMNS} FROM enclosures WHERE episode_id IN ({placeholders}) ORDER BY position, id"
        );
        let mut q = sqlx::query_as::<_, EnclosureRow>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for r in q.fetch_all(&mut *conn).await? {
            let key = r.episode_id.clone();
            enclosures.entry(key).or_default().push(r.into_model()?);
        }
        let sql = format!(
            "SELECT episode_id, data FROM episode_extras WHERE episode_id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, String)>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for (episode_id, data) in q.fetch_all(&mut *conn).await? {
            extras.insert(
                episode_id.clone(),
                row::json("episode_extras", "data", &episode_id, &data)?,
            );
        }
    }
    rows.into_iter()
        .map(|r| {
            let encs = enclosures.remove(&r.id).unwrap_or_default();
            let ext = extras.remove(&r.id).unwrap_or_default();
            r.into_model(encs, ext)
        })
        .collect()
}

/// Loads one episode with enclosures and extras.
pub async fn get(conn: &mut SqliteConnection, id: EpisodeId) -> Result<Option<Episode>> {
    let row: Option<EpisodeRow> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM episodes WHERE id = ?1"))
            .bind(id.to_string())
            .fetch_optional(&mut *conn)
            .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(attach(conn, vec![row]).await?.pop())
}

/// The episode a stable identity key names, within one podcast.
///
/// `(podcast_id, identity_key)` is unique, so this answers with at most
/// one row. It is what bridges a rebuild: after the database is lost and
/// the feed is added again, the episodes carry fresh identifiers, and the
/// identity key in a sidecar is the only thing that still points at the
/// same episode.
pub async fn find_by_identity(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    identity_key: &str,
) -> Result<Option<Episode>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT id FROM episodes WHERE podcast_id = ?1 AND identity_key = ?2")
            .bind(podcast_id.to_string())
            .bind(identity_key)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((id,)) = row else { return Ok(None) };
    get(conn, row::id("episodes", &id, &id)?).await
}

/// Loads several episodes (order unspecified).
pub async fn get_many(conn: &mut SqliteConnection, ids: &[EpisodeId]) -> Result<Vec<Episode>> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT {COLUMNS} FROM episodes WHERE id IN ({placeholders})");
        let mut q = sqlx::query_as::<_, EpisodeRow>(&sql);
        for id in chunk {
            q = q.bind(id.to_string());
        }
        let rows = q.fetch_all(&mut *conn).await?;
        out.extend(attach(conn, rows).await?);
    }
    Ok(out)
}

/// A page of episodes, newest first, using keyset pagination on
/// `(sort_at, id)`. `after` is the last row of the previous page.
pub async fn page(
    conn: &mut SqliteConnection,
    podcast_id: PodcastId,
    after: Option<(OffsetDateTime, EpisodeId)>,
    limit: u32,
) -> Result<Vec<Episode>> {
    let rows: Vec<EpisodeRow> = match after {
        None => {
            sqlx::query_as(&format!(
                "SELECT {COLUMNS} FROM episodes WHERE podcast_id = ?1 ORDER BY sort_at DESC, id DESC LIMIT ?2"
            ))
            .bind(podcast_id.to_string())
            .bind(i64::from(limit))
            .fetch_all(&mut *conn)
            .await?
        }
        Some((sort_at, id)) => {
            sqlx::query_as(&format!(
                "SELECT {COLUMNS} FROM episodes WHERE podcast_id = ?1 AND (sort_at < ?2 OR (sort_at = ?2 AND id < ?3)) \
                 ORDER BY sort_at DESC, id DESC LIMIT ?4"
            ))
            .bind(podcast_id.to_string())
            .bind(to_db_ts(sort_at))
            .bind(id.to_string())
            .bind(i64::from(limit))
            .fetch_all(&mut *conn)
            .await?
        }
    };
    attach(conn, rows).await
}

/// Episode counts for each podcast in `ids`: all, and not detected as removed.
///
/// One `GROUP BY` for a whole page. The per-podcast version was a full scan of
/// that podcast's episodes, run once per row of the podcast list, which made
/// listing a library quadratic in its size. Rides `idx_episodes_sort`, whose
/// leading column is `podcast_id`. A podcast with no episodes is absent from
/// the map rather than present as `(0, 0)`; the caller defaults it.
pub async fn counts_many(
    conn: &mut SqliteConnection,
    ids: &[PodcastId],
) -> Result<HashMap<PodcastId, (u64, u64)>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    for chunk in ids.chunks(CHUNK) {
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT podcast_id, count(*), \
             sum(CASE WHEN removed_from_feed_at IS NULL THEN 1 ELSE 0 END) \
             FROM episodes WHERE podcast_id IN ({placeholders}) GROUP BY podcast_id"
        );
        let mut q = sqlx::query_as::<_, (String, i64, i64)>(&sql);
        for id in chunk {
            q = q.bind(id.to_string());
        }
        for (podcast_id, all, present) in q.fetch_all(&mut *conn).await? {
            let id = row::id(TABLE, &podcast_id, &podcast_id)?;
            out.insert(
                id,
                (
                    u64::try_from(all).unwrap_or(0),
                    u64::try_from(present).unwrap_or(0),
                ),
            );
        }
    }
    Ok(out)
}

/// Number of episodes of a podcast (all, and not detected as removed).
pub async fn counts(conn: &mut SqliteConnection, podcast_id: PodcastId) -> Result<(u64, u64)> {
    let (all, present): (i64, i64) = sqlx::query_as(
        "SELECT count(*), sum(CASE WHEN removed_from_feed_at IS NULL THEN 1 ELSE 0 END) FROM episodes WHERE podcast_id = ?1",
    )
    .bind(podcast_id.to_string())
    .fetch_one(conn)
    .await?;
    Ok((
        u64::try_from(all).unwrap_or(0),
        u64::try_from(present).unwrap_or(0),
    ))
}

async fn update_in_chunks(
    conn: &mut SqliteConnection,
    sql_prefix: &str,
    ids: &[EpisodeId],
    first_bind: Option<String>,
) -> Result<u64> {
    let mut total = 0;
    for chunk in ids.chunks(CHUNK) {
        let offset = usize::from(first_bind.is_some());
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{}", i + offset))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("{sql_prefix} WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        if let Some(b) = &first_bind {
            q = q.bind(b.clone());
        }
        for id in chunk {
            q = q.bind(id.to_string());
        }
        total += q.execute(&mut *conn).await?.rows_affected();
    }
    Ok(total)
}

/// Marks episodes as seen in the feed now (resets the missing streak and
/// clears a previous removal).
pub async fn mark_seen(
    conn: &mut SqliteConnection,
    ids: &[EpisodeId],
    now: OffsetDateTime,
) -> Result<u64> {
    update_in_chunks(
        conn,
        "UPDATE episodes SET last_seen_in_feed_at = ?1, missing_streak = 0, removed_from_feed_at = NULL, updated_at = ?1",
        ids,
        Some(to_db_ts(now)),
    )
    .await
}

/// Increments the missing streak of episodes absent from a complete fetch.
pub async fn mark_missing(
    conn: &mut SqliteConnection,
    ids: &[EpisodeId],
    now: OffsetDateTime,
) -> Result<u64> {
    update_in_chunks(
        conn,
        "UPDATE episodes SET missing_streak = missing_streak + 1, updated_at = ?1",
        ids,
        Some(to_db_ts(now)),
    )
    .await
}

/// Records removal detection (never deletes).
pub async fn mark_removed(
    conn: &mut SqliteConnection,
    ids: &[EpisodeId],
    now: OffsetDateTime,
) -> Result<u64> {
    update_in_chunks(
        conn,
        "UPDATE episodes SET removed_from_feed_at = ?1, updated_at = ?1",
        ids,
        Some(to_db_ts(now)),
    )
    .await
}

/// Sets the denormalized archive state of one episode (owned by the
/// download engine).
pub async fn set_archive_state(
    conn: &mut SqliteConnection,
    id: EpisodeId,
    state: ArchiveState,
    skip_reason: Option<&str>,
    now: OffsetDateTime,
) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE episodes SET archive_state = ?1, skip_reason = ?2, updated_at = ?3 WHERE id = ?4",
    )
    .bind(state.as_str())
    .bind(skip_reason)
    .bind(to_db_ts(now))
    .bind(id.to_string())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// A page of candidate duplicates, newest first; `after` is the last id of
/// the previous page.
///
/// No index names candidates: they are rare, one podcast's are found among
/// its own rows, and the whole library's in one walk of the primary key
/// spread over the pages.
pub async fn duplicates(
    conn: &mut SqliteConnection,
    podcast_id: Option<PodcastId>,
    after: Option<EpisodeId>,
    limit: u32,
) -> Result<Vec<Episode>> {
    let mut where_parts = vec!["duplicate_of_episode_id IS NOT NULL".to_owned()];
    if podcast_id.is_some() {
        where_parts.push(format!("podcast_id = ?{}", where_parts.len()));
    }
    if after.is_some() {
        where_parts.push(format!("id < ?{}", where_parts.len()));
    }
    let sql = format!(
        "SELECT {COLUMNS} FROM episodes WHERE {} ORDER BY id DESC LIMIT ?{}",
        where_parts.join(" AND "),
        where_parts.len()
    );
    let mut q = sqlx::query_as::<_, EpisodeRow>(&sql);
    if let Some(podcast_id) = podcast_id {
        q = q.bind(podcast_id.to_string());
    }
    if let Some(after) = after {
        q = q.bind(after.to_string());
    }
    let rows = q.bind(i64::from(limit)).fetch_all(&mut *conn).await?;
    attach(conn, rows).await
}

/// Merges candidate duplicate `candidate` into `survivor` and deletes the
/// candidate's row (ADR 0051); never touches a file.
///
/// With `adopt`, the survivor takes the candidate's GUID, signal keys and
/// presence in the feed, and `adopt` becomes its identity reason. Returns
/// whether the candidate existed.
pub async fn merge_into(
    conn: &mut SqliteConnection,
    candidate: EpisodeId,
    survivor: EpisodeId,
    adopt: Option<&str>,
    now: OffsetDateTime,
) -> Result<bool> {
    let (candidate, survivor, now) = (candidate.to_string(), survivor.to_string(), to_db_ts(now));
    // Everything before the DELETE reads or moves rows that its cascade
    // would otherwise take with it: the change log (ADR 0015 keeps it),
    // the candidates that named this one, and the columns adopted.
    sqlx::query("UPDATE episode_changes SET episode_id = ?2 WHERE episode_id = ?1")
        .bind(&candidate)
        .bind(&survivor)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE episodes SET duplicate_of_episode_id = ?2, \
         skip_reason = CASE WHEN skip_reason = 'duplicate of ' || ?1 \
                       THEN 'duplicate of ' || ?2 ELSE skip_reason END, updated_at = ?3 \
         WHERE podcast_id = (SELECT podcast_id FROM episodes WHERE id = ?1) \
           AND duplicate_of_episode_id = ?1",
    )
    .bind(&candidate)
    .bind(&survivor)
    .bind(&now)
    .execute(&mut *conn)
    .await?;
    if let Some(reason) = adopt {
        sqlx::query(
            "UPDATE episodes SET (guid, guid_is_permalink, guid_key, enclosure_key, fingerprint_key, \
             missing_streak, last_seen_in_feed_at, removed_from_feed_at) = \
             (SELECT guid, guid_is_permalink, guid_key, enclosure_key, fingerprint_key, \
              missing_streak, last_seen_in_feed_at, removed_from_feed_at FROM episodes WHERE id = ?1), \
             identity_reason = ?3, updated_at = ?4 WHERE id = ?2",
        )
        .bind(&candidate)
        .bind(&survivor)
        .bind(reason)
        .bind(&now)
        .execute(&mut *conn)
        .await?;
    }
    let n = sqlx::query("DELETE FROM episodes WHERE id = ?1")
        .bind(&candidate)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    Ok(n == 1)
}

/// Makes candidate duplicate `id` an episode of its own (ADR 0051): the
/// link and reasons are cleared, and a skip that was the candidacy is
/// lifted. Returns whether `id` was a candidate.
pub async fn separate(
    conn: &mut SqliteConnection,
    id: EpisodeId,
    now: OffsetDateTime,
) -> Result<bool> {
    // SQLite evaluates every SET expression against the row as it was.
    let n = sqlx::query(
        "UPDATE episodes SET duplicate_of_episode_id = NULL, duplicate_reasons = '[]', \
         archive_state = CASE WHEN archive_state = ?2 \
                              AND skip_reason = 'duplicate of ' || duplicate_of_episode_id \
                         THEN ?3 ELSE archive_state END, \
         skip_reason = CASE WHEN archive_state = ?2 \
                            AND skip_reason = 'duplicate of ' || duplicate_of_episode_id \
                       THEN NULL ELSE skip_reason END, \
         updated_at = ?4 \
         WHERE id = ?1 AND duplicate_of_episode_id IS NOT NULL",
    )
    .bind(id.to_string())
    .bind(ArchiveState::Skipped.as_str())
    .bind(ArchiveState::Expected.as_str())
    .bind(to_db_ts(now))
    .execute(conn)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// A fresh enclosure id helper for tests and the engine.
#[must_use]
pub fn new_enclosure_id() -> EnclosureId {
    EnclosureId::new()
}

/// A complete episode with one enclosure (`https://cdn.test/<key>.mp3`),
/// for tests of this and dependent crates.
#[cfg(any(test, feature = "testing"))]
#[must_use]
pub fn sample(podcast_id: PodcastId, key: &str, title: &str, published: OffsetDateTime) -> Episode {
    let now = OffsetDateTime::now_utc();
    let id = EpisodeId::new();
    Episode {
        id,
        podcast_id,
        guid: Some(key.to_owned()),
        guid_is_permalink: Some(false),
        identity: EpisodeIdentity {
            key: format!("guid:{key}"),
            source: IdentitySource::Guid,
            guid_key: Some(key.to_owned()),
            enclosure_key: Some(format!("https://cdn.test/{key}.mp3")),
            fingerprint_key: Some(format!("fp{key}")),
            reason: "unique guid".into(),
        },
        title: title.to_owned(),
        subtitle: None,
        sort_title: uguisu_core::model::sort_title(title),
        description_html: Some("<p>Hi</p>".into()),
        description_text: Some("Hi".into()),
        link: None,
        published_at: Some(published),
        published_at_raw: Some(published.to_string()),
        published_at_quality: DateQuality::Exact,
        updated_at_source: None,
        duration_secs: Some(1800),
        duration_raw: Some("30:00".into()),
        season: Some(1),
        episode_number: Some(1),
        episode_type: Some("full".into()),
        explicit: Some(false),
        artwork_url: None,
        author: None,
        content_hash: "h1".into(),
        archive_state: ArchiveState::Expected,
        skip_reason: None,
        malformed: false,
        malformed_reason: None,
        duplicate_of_episode_id: None,
        duplicate_reasons: vec![],
        missing_streak: 0,
        first_seen_at: now,
        last_seen_in_feed_at: now,
        removed_from_feed_at: None,
        sort_at: published,
        source_metadata: Some(serde_json::json!({"guid": key})),
        enclosures: vec![Enclosure {
            id: EnclosureId::new(),
            episode_id: id,
            url: Url::parse(&format!("https://cdn.test/{key}.mp3"))
                .unwrap_or_else(|_| unreachable!()),
            mime_type: Some("audio/mpeg".into()),
            length_bytes: Some(1000),
            is_primary: true,
            kind: EnclosureKind::Audio,
            position: 0,
            bitrate: None,
            height: None,
            codecs: None,
            lang: None,
            title: None,
            integrity_type: None,
            integrity_value: None,
            sources: vec![],
        }],
        extras: EpisodeExtras::default(),
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Storage;
    use time::macros::datetime;
    use uguisu_core::model::TranscriptRef;

    #[tokio::test]
    async fn upsert_is_idempotent_and_keeps_enclosure_ids() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        let e1 = sample(p.id, "a", "Episode A", datetime!(2026-01-02 00:00:00 UTC));
        let mut e2 = sample(p.id, "b", "Episode B", datetime!(2026-01-01 00:00:00 UTC));
        e2.extras.transcripts.push(TranscriptRef {
            url: "https://t".into(),
            mime_type: Some("text/vtt".into()),
            language: None,
            rel: None,
        });
        upsert_all(&mut tx, &[e1.clone(), e2.clone()])
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut r = s.reader().await.unwrap();
        let idx = index(&mut r, p.id).await.unwrap();
        assert_eq!(idx.len(), 2);
        let stored = get(&mut r, e2.id).await.unwrap().unwrap();
        assert_eq!(stored.extras.transcripts.len(), 1);
        assert_eq!(stored.enclosures.len(), 1);
        let enc_id = stored.enclosures[0].id;

        // Same identity again with a changed title and a second enclosure: no new row, id kept.
        let mut again = e2.clone();
        again.title = "Episode B (updated)".into();
        again.content_hash = "h2".into();
        again.extras = EpisodeExtras::default();
        again.enclosures.push(Enclosure {
            id: EnclosureId::new(),
            episode_id: again.id,
            url: Url::parse("https://cdn.test/b.m4a").unwrap(),
            mime_type: Some("audio/mp4".into()),
            length_bytes: None,
            is_primary: false,
            kind: EnclosureKind::Audio,
            position: 1,
            bitrate: None,
            height: None,
            codecs: None,
            lang: None,
            title: None,
            integrity_type: None,
            integrity_value: None,
            sources: vec![],
        });
        let mut tx = s.begin().await.unwrap();
        upsert_all(&mut tx, &[again.clone()]).await.unwrap();
        tx.commit().await.unwrap();
        let stored = get(&mut r, e2.id).await.unwrap().unwrap();
        assert_eq!(stored.title, "Episode B (updated)");
        assert_eq!(stored.content_hash, "h2");
        assert_eq!(stored.enclosures.len(), 2);
        assert_eq!(
            stored.enclosures[0].id, enc_id,
            "existing enclosure keeps its id"
        );
        assert!(stored.extras.is_empty());
        assert_eq!(counts(&mut r, p.id).await.unwrap(), (2, 2));

        // Removing the second enclosure again deletes only that row.
        let mut tx = s.begin().await.unwrap();
        upsert_all(&mut tx, &[e2.clone()]).await.unwrap();
        tx.commit().await.unwrap();
        let stored = get(&mut r, e2.id).await.unwrap().unwrap();
        assert_eq!(stored.enclosures.len(), 1);
        assert_eq!(stored.enclosures[0].id, enc_id);
    }

    #[tokio::test]
    async fn upsert_keeps_duplicate_columns() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let original = sample(p.id, "a", "Episode A", datetime!(2026-01-01 00:00:00 UTC));
        let mut candidate = sample(p.id, "b", "Episode A", datetime!(2026-01-01 00:00:00 UTC));
        candidate.duplicate_of_episode_id = Some(original.id);
        candidate.duplicate_reasons = vec!["same_title".into()];
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        upsert_all(&mut tx, &[original, candidate.clone()])
            .await
            .unwrap();
        let mut refreshed = candidate.clone();
        refreshed.title = "Episode A (new)".into();
        refreshed.duplicate_of_episode_id = None;
        refreshed.duplicate_reasons = vec![];
        upsert_all(&mut tx, &[refreshed]).await.unwrap();
        tx.commit().await.unwrap();
        let stored = get(&mut s.reader().await.unwrap(), candidate.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.title, "Episode A (new)");
        assert_eq!(
            stored.duplicate_of_episode_id,
            candidate.duplicate_of_episode_id
        );
        assert_eq!(stored.duplicate_reasons, ["same_title"]);
    }

    fn candidate_of(original: &Episode, key: &str) -> Episode {
        let mut c = sample(
            original.podcast_id,
            key,
            &original.title,
            datetime!(2026-01-01 00:00:00 UTC),
        );
        c.duplicate_of_episode_id = Some(original.id);
        c.duplicate_reasons = vec!["same_title".into()];
        c.archive_state = ArchiveState::Skipped;
        c.skip_reason = Some(format!("duplicate of {}", original.id));
        c
    }

    async fn store(s: &Storage, podcast: &uguisu_core::model::Podcast, episodes: &[Episode]) {
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, podcast).await.unwrap();
        upsert_all(&mut tx, episodes).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn count(s: &Storage, sql: &str, id: EpisodeId) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id.to_string())
            .fetch_one(&mut *s.reader().await.unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn candidates_page_newest_first() {
        let s = Storage::open_temp().await.unwrap();
        let (show, other) = (
            crate::podcasts::sample("Show"),
            crate::podcasts::sample("Other"),
        );
        let o = sample(show.id, "a", "A", datetime!(2026-01-01 00:00:00 UTC));
        let mine: Vec<Episode> = ["b", "c", "d"]
            .iter()
            .map(|k| candidate_of(&o, k))
            .collect();
        let o2 = sample(other.id, "x", "X", datetime!(2026-01-01 00:00:00 UTC));
        let theirs = candidate_of(&o2, "y");
        store(&s, &show, &[vec![o], mine.clone()].concat()).await;
        store(&s, &other, &[o2, theirs.clone()]).await;
        let mut r = s.reader().await.unwrap();

        let mut expected: Vec<EpisodeId> = mine.iter().map(|e| e.id).collect();
        expected.sort_by(|a, b| b.cmp(a));
        let first = duplicates(&mut r, Some(show.id), None, 2).await.unwrap();
        let rest = duplicates(&mut r, Some(show.id), Some(first[1].id), 2)
            .await
            .unwrap();
        let paged: Vec<EpisodeId> = first.iter().chain(&rest).map(|e| e.id).collect();
        assert_eq!(paged, expected);
        assert!(
            duplicates(&mut r, Some(show.id), Some(rest[0].id), 2)
                .await
                .unwrap()
                .is_empty()
        );
        let all = duplicates(&mut r, None, None, 10).await.unwrap();
        assert_eq!(all.len(), 4);
        assert!(all.iter().any(|e| e.id == theirs.id));
        assert!(all.windows(2).all(|w| w[0].id > w[1].id));
    }

    #[tokio::test]
    async fn merge_keeps_history_then_cascades() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let mut o = sample(p.id, "a", "A", datetime!(2026-01-01 00:00:00 UTC));
        o.missing_streak = 3;
        o.removed_from_feed_at = Some(datetime!(2026-01-05 00:00:00 UTC));
        let adopted = candidate_of(&o, "b");
        let kept_apart = candidate_of(&o, "c");
        store(&s, &p, &[o.clone(), adopted.clone(), kept_apart.clone()]).await;
        let now = OffsetDateTime::now_utc();
        let mut tx = s.begin().await.unwrap();
        crate::changes::insert_all(
            &mut tx,
            &[uguisu_core::model::EpisodeChange {
                id: uguisu_core::ids::ChangeId::new(),
                episode_id: adopted.id,
                podcast_id: p.id,
                fetch_id: None,
                changed_at: now,
                field: "title".into(),
                old_value: Some("A".into()),
                new_value: Some("A!".into()),
            }],
        )
        .await
        .unwrap();
        assert!(
            merge_into(&mut tx, adopted.id, o.id, Some("merged candidate b"), now)
                .await
                .unwrap()
        );
        assert!(
            merge_into(&mut tx, kept_apart.id, o.id, None, now)
                .await
                .unwrap()
        );
        assert!(
            !merge_into(&mut tx, adopted.id, o.id, None, now)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();

        let mut r = s.reader().await.unwrap();
        assert!(get(&mut r, adopted.id).await.unwrap().is_none());
        for sql in [
            "SELECT count(*) FROM enclosures WHERE episode_id = ?1",
            "SELECT count(*) FROM episode_search_ids WHERE episode_id = ?1",
        ] {
            assert_eq!(count(&s, sql, adopted.id).await, 0, "{sql}");
        }
        let log = crate::changes::list_for_episode(&mut r, o.id)
            .await
            .unwrap();
        assert_eq!(log.len(), 1, "the candidate's history moved: {log:?}");
        let merged = get(&mut r, o.id).await.unwrap().unwrap();
        assert_eq!(merged.identity.key, "guid:a");
        assert_eq!(merged.identity.guid_key.as_deref(), Some("b"));
        assert_eq!(
            merged.identity.enclosure_key,
            adopted.identity.enclosure_key
        );
        assert_eq!(merged.identity.reason, "merged candidate b");
        assert_eq!(
            (merged.missing_streak, merged.removed_from_feed_at),
            (0, None)
        );
        assert_eq!(merged.enclosures.len(), 1, "its own enclosure stays");
    }

    #[tokio::test]
    async fn merge_repoints_a_chain() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let o = sample(p.id, "a", "A", datetime!(2026-01-01 00:00:00 UTC));
        let middle = candidate_of(&o, "b");
        let last = candidate_of(&middle, "c");
        store(&s, &p, &[o.clone(), middle.clone(), last.clone()]).await;
        let mut tx = s.begin().await.unwrap();
        merge_into(&mut tx, middle.id, o.id, None, OffsetDateTime::now_utc())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let last = get(&mut s.reader().await.unwrap(), last.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last.duplicate_of_episode_id, Some(o.id));
        assert_eq!(last.skip_reason, Some(format!("duplicate of {}", o.id)));
    }

    #[tokio::test]
    async fn separate_keeps_an_archived_state() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let o = sample(p.id, "a", "A", datetime!(2026-01-01 00:00:00 UTC));
        let skipped = candidate_of(&o, "b");
        let mut archived = candidate_of(&o, "c");
        archived.archive_state = ArchiveState::Archived;
        archived.skip_reason = None;
        store(&s, &p, &[o.clone(), skipped.clone(), archived.clone()]).await;
        let now = OffsetDateTime::now_utc();
        let mut tx = s.begin().await.unwrap();
        assert!(separate(&mut tx, skipped.id, now).await.unwrap());
        assert!(separate(&mut tx, archived.id, now).await.unwrap());
        assert!(
            !separate(&mut tx, o.id, now).await.unwrap(),
            "not a candidate"
        );
        tx.commit().await.unwrap();

        let mut r = s.reader().await.unwrap();
        let skipped = get(&mut r, skipped.id).await.unwrap().unwrap();
        assert_eq!(
            (
                skipped.archive_state,
                skipped.skip_reason,
                skipped.duplicate_of_episode_id
            ),
            (ArchiveState::Expected, None, None)
        );
        assert!(skipped.duplicate_reasons.is_empty());
        let archived = get(&mut r, archived.id).await.unwrap().unwrap();
        assert_eq!(archived.archive_state, ArchiveState::Archived);
        assert_eq!(archived.duplicate_of_episode_id, None);
    }

    #[tokio::test]
    async fn paging_handles_null_published_dates_and_streaks() {
        let s = Storage::open_temp().await.unwrap();
        let p = crate::podcasts::sample("Show");
        let mut tx = s.begin().await.unwrap();
        crate::podcasts::insert(&mut tx, &p).await.unwrap();
        let mut eps = Vec::new();
        for i in 0..7u32 {
            let mut e = sample(
                p.id,
                &format!("k{i}"),
                &format!("E{i}"),
                datetime!(2026-01-01 00:00:00 UTC) + time::Duration::days(i64::from(i)),
            );
            if i == 3 {
                e.published_at = None;
                e.published_at_quality = DateQuality::Invalid;
                e.sort_at = e.first_seen_at;
            }
            eps.push(e);
        }
        upsert_all(&mut tx, &eps).await.unwrap();
        tx.commit().await.unwrap();
        let mut r = s.reader().await.unwrap();
        let first = page(&mut r, p.id, None, 3).await.unwrap();
        assert_eq!(first.len(), 3);
        assert!(first[0].sort_at >= first[1].sort_at);
        let last = first.last().unwrap();
        let second = page(&mut r, p.id, Some((last.sort_at, last.id)), 10)
            .await
            .unwrap();
        assert_eq!(second.len(), 4);
        let mut all: Vec<EpisodeId> = first.iter().chain(second.iter()).map(|e| e.id).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 7, "no duplicates or gaps across pages");
        assert!(
            first
                .iter()
                .chain(second.iter())
                .any(|e| e.published_at.is_none()),
            "the undated episode is paged too (sorted by first_seen_at)"
        );

        let ids: Vec<EpisodeId> = eps.iter().take(2).map(|e| e.id).collect();
        let now = OffsetDateTime::now_utc();
        let mut tx = s.begin().await.unwrap();
        assert_eq!(mark_missing(&mut tx, &ids, now).await.unwrap(), 2);
        assert_eq!(mark_missing(&mut tx, &ids, now).await.unwrap(), 2);
        assert_eq!(mark_removed(&mut tx, &ids[..1], now).await.unwrap(), 1);
        tx.commit().await.unwrap();
        let idx = index(&mut r, p.id).await.unwrap();
        let a = idx.iter().find(|x| x.id == ids[0]).unwrap();
        assert_eq!(a.missing_streak, 2);
        assert!(a.removed_from_feed_at.is_some());
        assert_eq!(counts(&mut r, p.id).await.unwrap(), (7, 6));
        let mut tx = s.begin().await.unwrap();
        assert_eq!(mark_seen(&mut tx, &ids, now).await.unwrap(), 2);
        tx.commit().await.unwrap();
        let idx = index(&mut r, p.id).await.unwrap();
        let a = idx.iter().find(|x| x.id == ids[0]).unwrap();
        assert_eq!(a.missing_streak, 0);
        assert!(a.removed_from_feed_at.is_none());
        let many = get_many(&mut r, &ids).await.unwrap();
        assert_eq!(many.len(), 2);
    }
}
