//! An older database upgrades without losing anything — Phase 3 (migration
//! 0001 only), Phase 4 (0001 + 0002 with download rows), Phase 5
//! (0001-0003 with an archive record), Phase 6 (0001-0004 with artwork
//! and a manifest) and Phase 7 (0001-0005 with settings and a paused
//! scheduler) all reach the current schema — and a fresh database carries
//! every table. What a real build wrote is `uguisu-cli`'s `upgrade` test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlx::migrate::{Migrate, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Row};
use uguisu_core::archive::{ArchiveOrigin, TagState, VerificationState};
use uguisu_core::model::ArchiveState;
use uguisu_storage::Storage;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const P: &str = "01J0000000000000000000000P";
const S: &str = "01J0000000000000000000000S";
const E: &str = "01J0000000000000000000000E";
const N: &str = "01J0000000000000000000000N";
const V: &str = "01J0000000000000000000000V";
const J: &str = "01J0000000000000000000000J";
const A: &str = "01J0000000000000000000000A";
const F: &str = "01J0000000000000000000000F";
const W: &str = "01J0000000000000000000000W";

async fn phase3_only(path: &std::path::Path) {
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    conn.ensure_migrations_table().await.unwrap();
    let first = MIGRATOR
        .iter()
        .find(|m| m.version == 1)
        .expect("migration 0001");
    conn.apply(first).await.unwrap();
    // A podcast, a source, an episode with an enclosure and an event, as
    // Phase 3 wrote them.
    let now = "2026-09-17T10:00:00Z";
    sqlx::query(
        "INSERT INTO podcasts (id, title, sort_title, categories, feed_kind, status, metadata_hash, created_at, updated_at) \
         VALUES (?2, 'Show', 'show', '[]', 'rss2', 'active', 'h', ?1, ?1)",
    )
    .bind(now)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO podcast_sources (id, podcast_id, feed_url, provider, is_current, fetch_state, discovered_at, created_at, updated_at) \
         VALUES (?2, ?3, 'https://feeds.example/a.xml', 'manual', 1, 'fetched', ?1, ?1, ?1)",
    )
    .bind(now)
    .bind(S)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO episodes (id, podcast_id, identity_key, identity_source, title, sort_title, content_hash, first_seen_at, last_seen_in_feed_at, sort_at, created_at, updated_at) \
         VALUES (?2, ?3, 'guid:x', 'guid', 'Ep', 'ep', 'c', ?1, ?1, ?1, ?1, ?1)",
    )
    .bind(now)
    .bind(E)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO enclosures (id, episode_id, url, is_primary, kind, position, sources) \
         VALUES (?1, ?2, 'https://cdn.example/a.mp3', 1, 'audio', 0, '[]')",
    )
    .bind(N)
    .bind(E)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO events (id, occurred_at, kind, podcast_id, payload) \
         VALUES (?2, ?1, 'podcast.added', ?3, ?4)",
    )
    .bind(now)
    .bind(V)
    .bind(P)
    .bind(format!(
        "{{\"kind\":\"podcast.added\",\"title\":\"Show\",\"feed_url\":\"https://feeds.example/a.xml\",\"source_id\":\"{S}\"}}"
    ))
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
async fn phase3_database_upgrades_keeping_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uguisu.db");
    phase3_only(&path).await;

    let s = Storage::open_path(&path).await.unwrap();
    let mut r = s.reader().await.unwrap();
    let versions: Vec<i64> = sqlx::query("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut *r)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<i64, _>(0))
        .collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

    let tables: Vec<String> = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'download_%' ORDER BY name",
    )
    .fetch_all(&mut *r)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>(0))
    .collect();
    assert_eq!(
        tables,
        vec!["download_attempts", "download_control", "download_jobs"]
    );
    let archive_tables: Vec<String> = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'archive_%' ORDER BY name",
    )
    .fetch_all(&mut *r)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>(0))
    .collect();
    assert_eq!(
        archive_tables,
        vec!["archive_files", "archive_manifests", "archive_policies"]
    );

    let podcasts: i64 = sqlx::query_scalar("SELECT count(*) FROM podcasts")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(podcasts, 1);
    // Migration 0009 counted the episodes that were already there.
    let (counted, episodes): (i64, i64) =
        sqlx::query_as("SELECT episode_count, (SELECT count(*) FROM episodes) FROM podcasts")
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert!(episodes > 0);
    assert_eq!(counted, episodes);
    let episode_state: String =
        sqlx::query_scalar("SELECT archive_state FROM episodes WHERE id = ?1")
            .bind(E)
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert_eq!(episode_state, ArchiveState::Expected.as_str());
    let enclosures: i64 = sqlx::query_scalar("SELECT count(*) FROM enclosures")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(enclosures, 1);
    let events = uguisu_storage::events::list_after(&mut r, None, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name(), "podcast.added");

    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM download_jobs")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
    let control = uguisu_storage::downloads::control_get(&mut r)
        .await
        .unwrap();
    assert!(!control.paused);

    let fk: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *r)
        .await
        .unwrap();
    assert!(fk.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(r);

    // Opening again is a no-op.
    s.close().await;
    let s = Storage::open_path(&path).await.unwrap();
    s.close().await;
}

#[tokio::test]
async fn a_fresh_database_has_every_table() {
    let s = Storage::open_temp().await.unwrap();
    let mut r = s.reader().await.unwrap();
    let tables: Vec<String> =
        // FTS5 keeps its own shadow tables (`..._fts_data`, `_idx`,
        // `_content`, `_docsize`, `_config`) beside the virtual table. They
        // are an implementation detail of the index, so they are filtered
        // out here rather than pinned - the virtual tables themselves are
        // asserted.
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '_sqlx%' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%\\_fts\\_%' ESCAPE '\\' ORDER BY name")
            .fetch_all(&mut *r)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get::<String, _>(0))
            .collect();
    assert_eq!(
        tables,
        vec![
            "archive_files",
            "archive_manifests",
            "archive_policies",
            "auth_credential",
            "auth_sessions",
            "auth_tokens",
            "discovery_cache",
            "discovery_records",
            "download_attempts",
            "download_control",
            "download_jobs",
            "enclosures",
            "episode_changes",
            "episode_extras",
            "episode_search_ids",
            "episodes",
            "episodes_fts",
            "events",
            "feed_fetches",
            "podcast_artwork",
            "podcast_sources",
            "podcasts",
            "podcasts_fts",
            "scheduler_control",
            "search_index_state",
            "settings",
        ]
    );
    let control = uguisu_storage::downloads::control_get(&mut r)
        .await
        .unwrap();
    assert!(!control.paused);
    drop(r);
    s.close().await;
}

/// A Phase-4 database: migrations 0001 and 0002, with a completed download
/// job and its attempt, as the download engine wrote them.
async fn phase4_with_downloads(path: &std::path::Path) {
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    conn.ensure_migrations_table().await.unwrap();
    for version in [1, 2] {
        let m = MIGRATOR
            .iter()
            .find(|m| m.version == version)
            .expect("migration");
        conn.apply(m).await.unwrap();
    }
    let now = "2026-09-18T10:00:00Z";
    sqlx::query(
        "INSERT INTO podcasts (id, title, sort_title, categories, feed_kind, status, metadata_hash, created_at, updated_at) \
         VALUES (?2, 'Show', 'show', '[]', 'rss2', 'active', 'h', ?1, ?1)",
    )
    .bind(now)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO episodes (id, podcast_id, identity_key, identity_source, title, sort_title, content_hash, archive_state, first_seen_at, last_seen_in_feed_at, sort_at, created_at, updated_at) \
         VALUES (?2, ?3, 'guid:x', 'guid', 'Ep', 'ep', 'c', 'archived', ?1, ?1, ?1, ?1, ?1)",
    )
    .bind(now)
    .bind(E)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO download_jobs (id, episode_id, podcast_id, source_url, host_key, state, state_reason, \
            priority, attempt_count, max_attempts, bytes_downloaded, total_bytes, part_path, target_path, \
            hash_algo, hash_value, created_at, updated_at, finished_at) \
         VALUES (?2, ?3, ?4, 'https://cdn.example/a.mp3', 'https://cdn.example:443', 'completed', NULL, \
            1, 1, 8, 1024, 1024, ?4 || '/.uguisu-tmp/j.part', ?4 || '/' || ?3 || '.mp3', \
            'sha256', 'abc', ?1, ?1, ?1)",
    )
    .bind(now)
    .bind(J)
    .bind(E)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO download_attempts (id, job_id, attempt_no, started_at, finished_at, source_url, \
            range_start, http_status, bytes_received, duration_ms, outcome) \
         VALUES (?2, ?3, 1, ?1, ?1, 'https://cdn.example/a.mp3', 0, 200, 1024, 50, 'completed')",
    )
    .bind(now)
    .bind(A)
    .bind(J)
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
async fn phase4_database_upgrades_keeping_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uguisu.db");
    phase4_with_downloads(&path).await;

    let s = Storage::open_path(&path).await.unwrap();
    let mut r = s.reader().await.unwrap();
    let versions: Vec<i64> = sqlx::query("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut *r)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<i64, _>(0))
        .collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

    // The Phase-4 rows are untouched: Phase 5 only adds tables.
    let job = uguisu_storage::downloads::get_job(&mut r, J.parse().unwrap())
        .await
        .unwrap()
        .expect("the completed job survived");
    assert_eq!(job.state, uguisu_core::download::DownloadState::Completed);
    assert_eq!(job.hash_value.as_deref(), Some("abc"));
    assert_eq!(job.bytes_downloaded, 1024);
    let attempts = uguisu_storage::downloads::list_attempts(&mut r, job.id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    let episode_state: String =
        sqlx::query_scalar("SELECT archive_state FROM episodes WHERE id = ?1")
            .bind(E)
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert_eq!(episode_state, ArchiveState::Archived.as_str());

    // The archive tables exist and are empty: registration is the engine's
    // job, not the migration's.
    let files = uguisu_storage::archive_files::list(
        &mut r,
        &uguisu_storage::archive_files::ArchiveFilter::default(),
        None,
        10,
    )
    .await
    .unwrap();
    assert!(files.is_empty());
    let pending = uguisu_storage::archive_files::completed_unregistered(&mut r, 10)
        .await
        .unwrap();
    assert_eq!(
        pending,
        vec![E.parse::<uguisu_core::ids::EpisodeId>().unwrap()],
        "the completed job is what reconciliation will register"
    );
    assert!(
        uguisu_storage::archive_policies::list(&mut r)
            .await
            .unwrap()
            .is_empty()
    );

    let fk: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *r)
        .await
        .unwrap();
    assert!(fk.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(r);
    s.close().await;
}

/// A Phase-5 database: migrations 0001-0003, with a registered archive
/// record whose hash and size are the bytes that were downloaded — which
/// is exactly what Phase 6's provenance columns have to inherit.
async fn phase5_with_archive(path: &std::path::Path) {
    phase4_with_downloads(path).await;
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let third = MIGRATOR
        .iter()
        .find(|m| m.version == 3)
        .expect("migration 0003");
    conn.apply(third).await.unwrap();
    let now = "2026-09-19T10:00:00Z";
    sqlx::query(
        "INSERT INTO archive_files (id, episode_id, podcast_id, relative_path, size_bytes, \
            content_type, sniffed_type, hash_algo, hash_value, mtime_unix, verification_state, \
            verification_reason, verified_at, registered_at, created_at, updated_at) \
         VALUES (?2, ?3, ?4, 'Show/2026/2026-09-19 - Ep.mp3', 1024, 'audio/mpeg', 'mp3', \
            'sha256', 'abc', 1700000000, 'verified', 'hash_match', ?1, ?1, ?1, ?1)",
    )
    .bind(now)
    .bind(F)
    .bind(E)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO archive_policies (podcast_id, mode, max_backlog, max_age_days, priority, updated_at) \
         VALUES (?2, 'auto', 5, 30, 2, ?1)",
    )
    .bind(now)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
async fn phase5_upgrade_backfills_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uguisu.db");
    phase5_with_archive(&path).await;

    let s = Storage::open_path(&path).await.unwrap();
    let mut r = s.reader().await.unwrap();
    let versions: Vec<i64> = sqlx::query("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut *r)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<i64, _>(0))
        .collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

    // The Phase-5 finding survives: an upgrade is not a verification, and
    // it must never quietly turn a checked record into an unchecked one.
    let f = uguisu_storage::archive_files::get(&mut r, F.parse().unwrap())
        .await
        .unwrap()
        .expect("the archive record survived");
    assert_eq!(f.verification_state, VerificationState::Verified);
    assert_eq!(f.verification_reason.as_deref(), Some("hash_match"));
    assert_eq!(f.hash_value, "abc");
    assert_eq!(f.size_bytes, 1024);

    // For a pre-Phase-6 row the bytes on disk *are* the bytes downloaded:
    // nothing had ever rewritten an archived file.
    assert_eq!(f.source_hash_value.as_deref(), Some("abc"));
    assert_eq!(f.source_hash_algo.as_deref(), Some("sha256"));
    assert_eq!(f.source_size_bytes, Some(1024));
    assert_eq!(f.origin, ArchiveOrigin::Download);
    assert_eq!(f.tag_state, TagState::Untagged);
    assert_eq!(f.tag_mode, None);
    assert_eq!(f.tagged_at, None);
    assert_eq!(
        f.sidecar_written_at, None,
        "no sidecar was ever written for it; the engine will write one"
    );
    assert_eq!(
        uguisu_storage::archive_files::without_sidecar(&mut r, None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        uguisu_storage::archive_files::tagging_interrupted(&mut r, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // The Phase-5 policy is untouched, and the new tables start empty:
    // populating them is the engine's job, not the migration's.
    let policies = uguisu_storage::archive_policies::list(&mut r)
        .await
        .unwrap();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].max_backlog, Some(5));
    assert!(
        uguisu_storage::manifests::list(&mut r)
            .await
            .unwrap()
            .is_empty(),
        "a manifest is written, never migrated into existence"
    );
    assert!(
        uguisu_storage::artwork::current(&mut r, P.parse().unwrap())
            .await
            .unwrap()
            .is_none()
    );

    let fk: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *r)
        .await
        .unwrap();
    assert!(fk.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(r);

    // Opening again is a no-op: the backfill does not run twice.
    s.close().await;
    let s = Storage::open_path(&path).await.unwrap();
    let mut r = s.reader().await.unwrap();
    let again = uguisu_storage::archive_files::get(&mut r, F.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, f);
    drop(r);
    s.close().await;
}

/// A Phase-6 database: migrations 0001–0004, with the artwork and manifest
/// rows that phase added on top of the Phase-5 archive record.
async fn phase6_with_assets(path: &std::path::Path) {
    phase5_with_archive(path).await;
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let fourth = MIGRATOR
        .iter()
        .find(|m| m.version == 4)
        .expect("migration 0004");
    conn.apply(fourth).await.unwrap();
    let now = "2026-09-20T10:00:00Z";
    sqlx::query(
        "INSERT INTO podcast_artwork (id, podcast_id, source_url, relative_path, format, \
            content_type, size_bytes, hash_algo, hash_value, is_current, retrieved_at, \
            created_at, updated_at) \
         VALUES (?2, ?3, 'https://example.invalid/art.png', \
            '.uguisu/artwork/p/deadbeef.png', 'png', 'image/png', 64, 'sha256', 'deadbeef', \
            1, ?1, ?1, ?1)",
    )
    .bind(now)
    .bind(W)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO archive_manifests (podcast_id, relative_path, entries, hash_value, stale, \
            generated_at, updated_at) \
         VALUES (?2, '.uguisu/manifests/p/manifest.sha256', 1, 'cafe', 0, ?1, ?1)",
    )
    .bind(now)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
async fn phase6_upgrade_starts_index_stale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uguisu.db");
    phase6_with_assets(&path).await;

    let s = Storage::open_path(&path).await.unwrap();
    let mut r = s.reader().await.unwrap();
    let versions: Vec<i64> = sqlx::query("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut *r)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<i64, _>(0))
        .collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

    // Everything Phase 6 wrote is still there and still says what it said.
    let f = uguisu_storage::archive_files::get(&mut r, F.parse().unwrap())
        .await
        .unwrap()
        .expect("the archive record survived");
    assert_eq!(f.verification_state, VerificationState::Verified);
    assert_eq!(f.source_hash_value.as_deref(), Some("abc"));
    let art: i64 = sqlx::query_scalar("SELECT count(*) FROM podcast_artwork WHERE is_current = 1")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(art, 1);
    let manifests: i64 =
        sqlx::query_scalar("SELECT count(*) FROM archive_manifests WHERE stale = 0")
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert_eq!(
        manifests, 1,
        "a fresh manifest is not invalidated by an upgrade"
    );

    // The migration creates the index and leaves it empty on purpose: a
    // 100k-row build inside `Engine::open` would be tens of seconds that an
    // interrupted start rolls back and repeats. The engine fills it later.
    let state: String = sqlx::query_scalar("SELECT state FROM search_index_state WHERE id = 1")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(state, "stale");
    let indexed: i64 = sqlx::query_scalar("SELECT count(*) FROM episodes_fts")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(
        indexed, 0,
        "an existing episode is not indexed by the migration itself"
    );
    let episodes: i64 = sqlx::query_scalar("SELECT count(*) FROM episodes")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(episodes, 1, "and it is still there to be indexed");

    // The scheduler starts unpaused, with no maintenance run recorded, so
    // the first daily tick happens on its own schedule rather than at boot.
    let (paused, last): (i64, Option<String>) =
        sqlx::query_as("SELECT paused, last_maintenance_at FROM scheduler_control WHERE id = 1")
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert_eq!(paused, 0);
    assert_eq!(last, None);

    let fk: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *r)
        .await
        .unwrap();
    assert!(fk.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(r);
    s.close().await;
}

async fn phase7_with_service(path: &std::path::Path) {
    phase6_with_assets(path).await;
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let fifth = MIGRATOR
        .iter()
        .find(|m| m.version == 5)
        .expect("migration 0005");
    conn.apply(fifth).await.unwrap();
    let now = "2026-09-20T12:00:00Z";
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at, updated_by) \
         VALUES ('UGUISU_ARCHIVE_MAX_AGE_DAYS', '30', ?1, 'api')",
    )
    .bind(now)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE scheduler_control SET paused = 1, paused_reason = 'travelling', paused_at = ?1, \
         updated_at = ?1 WHERE id = 1",
    )
    .bind(now)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_records (id, input, feed_url, status, podcast_id, resolved_at, created_at) \
         VALUES ('01J0000000000000000000000D', 'show', 'https://feeds.example/a.xml', 'resolved', ?2, ?1, ?1)",
    )
    .bind(now)
    .bind(P)
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
async fn phase7_upgrade_keeps_the_service_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uguisu.db");
    phase7_with_service(&path).await;

    let s = Storage::open_path(&path).await.unwrap();
    let mut r = s.reader().await.unwrap();
    let versions: Vec<i64> = sqlx::query("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut *r)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<i64, _>(0))
        .collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'UGUISU_ARCHIVE_MAX_AGE_DAYS'")
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert_eq!(stored, "30");
    let (paused, reason): (i64, Option<String>) =
        sqlx::query_as("SELECT paused, paused_reason FROM scheduler_control WHERE id = 1")
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert_eq!((paused, reason.as_deref()), (1, Some("travelling")));
    let records: i64 =
        sqlx::query_scalar("SELECT count(*) FROM discovery_records WHERE podcast_id = ?1")
            .bind(P)
            .fetch_one(&mut *r)
            .await
            .unwrap();
    assert_eq!(records, 1);

    // Authentication arrives off: no credential, no session, no token, so an
    // upgraded installation opens exactly as it did before.
    for table in ["auth_credential", "auth_sessions", "auth_tokens"] {
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *r)
            .await
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *r)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(r);
    s.close().await;
}

/// Seeds one show with one episode through raw SQL, so the assertions
/// below are about the triggers and not about a repository.
async fn seed_one_show(s: &Storage) {
    let mut tx = s.begin().await.unwrap();
    let now = "2026-09-21T10:00:00Z";
    sqlx::query(
        "INSERT INTO podcasts (id, title, sort_title, author, description_text, feed_kind, \
            metadata_hash, created_at, updated_at) \
         VALUES (?2, 'Die Pizza-Akte', 'pizza-akte', 'Die Autorin', \
            'Eine Sendung über Käse und Ketchup.', 'rss', '', ?1, ?1)",
    )
    .bind(now)
    .bind(P)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO episodes (id, podcast_id, identity_key, identity_source, title, sort_title, \
            description_text, content_hash, first_seen_at, last_seen_in_feed_at, sort_at, \
            created_at, updated_at) \
         VALUES (?3, ?2, 'guid:one', 'guid', 'Folge 141: Käse & Ketchup', 'folge 141', \
            'Teil eins der Pizza-Akte.', 'h', ?1, ?1, ?1, ?1, ?1)",
    )
    .bind(now)
    .bind(P)
    .bind(E)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn count(s: &Storage, sql: &str, term: Option<&str>) -> i64 {
    let mut r = s.reader().await.unwrap();
    let q = sqlx::query_scalar(sql);
    let q = if let Some(t) = term {
        q.bind(t.to_owned())
    } else {
        q
    };
    q.fetch_one(&mut *r).await.unwrap()
}

const EP_MATCH: &str = "SELECT count(*) FROM episodes_fts WHERE episodes_fts MATCH ?1";
const EP_ROWS: &str = "SELECT count(*) FROM episodes_fts";

/// The search index is maintained by triggers, so it holds for any writer —
/// including raw SQL that never went through a repository.
#[tokio::test]
async fn triggers_index_without_the_repositories() {
    let s = Storage::open_temp().await.unwrap();
    seed_one_show(&s).await;

    assert_eq!(
        count(&s, EP_MATCH, Some("\"ketchup\"")).await,
        1,
        "the insert trigger indexed the episode"
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM podcasts_fts WHERE podcasts_fts MATCH ?1",
            Some("\"pizza\"")
        )
        .await,
        1
    );
    // Diacritics fold: `remove_diacritics 2` is what lets "kase" find
    // "Käse" without the user knowing where the umlaut was.
    assert_eq!(count(&s, EP_MATCH, Some("\"kase\"")).await, 1);

    // A title change replaces the row rather than adding one.
    let mut tx = s.begin().await.unwrap();
    sqlx::query("UPDATE episodes SET title = 'Folge 141: Der Nachschlag' WHERE id = ?1")
        .bind(E)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count(&s, EP_ROWS, None).await, 1, "updated, not duplicated");
    assert_eq!(count(&s, EP_MATCH, Some("\"nachschlag\"")).await, 1);

    // A write that touches no text column costs no index write. The narrow
    // updaters (archive_state, missing_streak, …) run on every refresh, so
    // this is the one that has to stay true.
    let before = count(&s, EP_ROWS, None).await;
    let mut tx = s.begin().await.unwrap();
    sqlx::query("UPDATE episodes SET archive_state = 'archived', updated_at = ?1 WHERE id = ?2")
        .bind("2026-09-21T11:00:00Z")
        .bind(E)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(before, count(&s, EP_ROWS, None).await);
    s.close().await;
}

/// Deleting a podcast cascades to its episodes, and the cascade has to
/// reach the index — which it only does with `recursive_triggers` on, and
/// only because the cleanup hangs off the id map rather than off the
/// episode (the map row is gone by the time the episode trigger runs).
#[tokio::test]
async fn deleting_a_podcast_empties_the_index() {
    let s = Storage::open_temp().await.unwrap();
    seed_one_show(&s).await;
    assert_eq!(count(&s, EP_ROWS, None).await, 1);

    let mut tx = s.begin().await.unwrap();
    sqlx::query("DELETE FROM podcasts WHERE id = ?1")
        .bind(P)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    for table in ["episodes_fts", "podcasts_fts", "episode_search_ids"] {
        let left = count(&s, &format!("SELECT count(*) FROM {table}"), None).await;
        assert_eq!(left, 0, "{table} still holds a deleted podcast's rows");
    }
    s.close().await;
}

/// What an older build does with a database a newer one migrated: it refuses
/// to open it and changes nothing, so a downgrade cannot run old code over a
/// schema it does not know. The message is the literal TROUBLESHOOTING.md
/// quotes.
#[tokio::test]
async fn newer_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uguisu.db");
    Storage::open_path(&path).await.unwrap().close().await;
    let newer = {
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
        let newest: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, 'from a newer build', 1, x'00', 0)",
        )
        .bind(newest + 1)
        .execute(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();
        newest + 1
    };
    let before = std::fs::read(&path).unwrap();

    let refused = Storage::open_path(&path).await.map(|_| ()).unwrap_err();
    assert_eq!(
        refused.to_string(),
        format!(
            "migration: migration {newer} was previously applied but is missing in the resolved migrations"
        )
    );
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing was written");
    // In WAL mode a write lands in the -wal first: the clean close above
    // left none, so a refused open that only read leaves it without frames.
    let wal = std::fs::metadata(path.with_extension("db-wal")).map_or(0, |m| m.len());
    assert_eq!(wal, 0, "nothing was written to the WAL");
}
