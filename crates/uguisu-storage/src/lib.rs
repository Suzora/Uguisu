//! Persistence.
//!
//! SQLite in WAL mode through `sqlx` (ADR 0002): embedded forward-only
//! migrations, one writer pool (size 1) whose transactions start with
//! `BEGIN IMMEDIATE`, one read-only reader pool, and repository functions
//! for every entity in `docs/DATA_MODEL.md`. The database is an
//! index over the archive and can be rebuilt from sidecars.
//!
//! Repositories take `&mut SqliteConnection`: inside a transaction pass
//! `&mut *tx`, for reads pass a connection from [`Storage::reader`]. Nothing
//! here reaches for the pool while a transaction is open, which keeps the
//! single-writer discipline honest.
//!
//! Timestamps are stored as UTC RFC 3339 text with second precision
//! ([`to_db_ts`]) so that text order equals time order.

pub mod archive_files;
pub mod archive_policies;
pub mod artwork;
pub mod auth;
pub mod changes;
pub mod discovery;
pub mod downloads;
pub mod episodes;
pub mod events;
pub mod fetches;
pub mod maintenance;
pub mod manifests;
pub mod podcasts;
pub mod podgrab;
mod row;
pub mod scheduler;
pub mod search;
pub mod settings;
pub mod sources;
mod ts;

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use sqlx::SqliteConnection;
use sqlx::pool::PoolConnection;
use sqlx::sqlite::{
    Sqlite, SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
    SqliteSynchronous,
};
pub use ts::{parse_db_ts, to_db_ts};
use uguisu_core::UguisuError;
use uguisu_core::config::DataConfig;

/// A writer transaction (`BEGIN IMMEDIATE`).
pub type Tx = sqlx::Transaction<'static, Sqlite>;

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Database driver error.
    #[error("database: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Migration failure.
    #[error("migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// Data directory problem.
    #[error("data directory {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// A stored row does not decode into the model.
    #[error("corrupt row in {table} ({id}): {detail}")]
    Corrupt {
        /// Table.
        table: &'static str,
        /// Row id.
        id: String,
        /// What was wrong.
        detail: String,
    },
    /// A JSON column does not parse.
    #[error("json in {table}.{column} ({id}): {source}")]
    Json {
        /// Table.
        table: &'static str,
        /// Column.
        column: &'static str,
        /// Row id.
        id: String,
        /// Cause.
        source: serde_json::Error,
    },
    /// Configuration (data directory) problem.
    #[error("{0}")]
    Config(String),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, StorageError>;

impl From<StorageError> for UguisuError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::Config(m) => Self::Config(m),
            StorageError::Sqlx(sqlx::Error::RowNotFound) => Self::NotFound {
                entity: "row".to_owned(),
                id: String::new(),
            },
            other => Self::Storage(other.to_string()),
        }
    }
}

/// An open database.
#[derive(Debug, Clone)]
pub struct Storage {
    writer: SqlitePool,
    reader: SqlitePool,
    path: PathBuf,
    schema: SchemaChange,
}

/// The schema a database had when it was opened, and what opening it applied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchemaChange {
    /// The newest migration it had; `None` for a new database.
    pub found: Option<i64>,
    /// The newest migration it has now.
    pub now: i64,
    /// The migrations this open applied, oldest first.
    pub applied: Vec<i64>,
}

impl Storage {
    /// Opens (creating if needed) the database in the configured data
    /// directory and applies pending migrations.
    pub async fn open(cfg: &DataConfig) -> Result<Self> {
        let dir = cfg
            .data_dir()
            .map_err(|e| StorageError::Config(e.to_string()))?;
        std::fs::create_dir_all(&dir).map_err(|source| StorageError::Io {
            path: dir.clone(),
            source,
        })?;
        Self::open_path(&dir.join(DataConfig::DATABASE_FILE)).await
    }

    /// Opens (creating if needed) a database file and applies migrations.
    pub async fn open_path(path: &Path) -> Result<Self> {
        let base = SqliteConnectOptions::new()
            .filename(path)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            // Without this, a row removed by `ON DELETE CASCADE` does not
            // fire the child table's triggers — so deleting a podcast would
            // leave its episodes in the search index (migration 0005).
            // SQLite defaults it off for compatibility with schemas written
            // before recursion existed; ours has no trigger that recurses.
            .pragma("recursive_triggers", "ON")
            .busy_timeout(Duration::from_secs(5));
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(base.clone().create_if_missing(true))
            .await?;
        let before = {
            let mut conn = writer.acquire().await?;
            maintenance::applied_versions(&mut conn).await?
        };
        sqlx::migrate!("./migrations").run(&writer).await?;
        let after = {
            let mut conn = writer.acquire().await?;
            maintenance::applied_versions(&mut conn).await?
        };
        let schema = SchemaChange {
            found: before.last().copied(),
            now: after.last().copied().unwrap_or(0),
            applied: after.into_iter().filter(|v| !before.contains(v)).collect(),
        };
        if !schema.applied.is_empty() {
            tracing::info!(from = ?schema.found, to = schema.now, "database migrated");
        }
        let reader = SqlitePoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(base.read_only(true))
            .await?;
        tracing::debug!(path = %path.display(), "database open");
        Ok(Self {
            writer,
            reader,
            path: path.to_path_buf(),
            schema,
        })
    }

    /// Opens a fresh database in a unique temporary file (for tests and
    /// benchmarks). The file is not removed automatically.
    pub async fn open_temp() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "uguisu-{}-{}.db",
            std::process::id(),
            uguisu_core::EventId::new()
        ));
        Self::open_path(&path).await
    }

    /// Path of the database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The schema this database had when it was opened, and what opening
    /// it applied.
    #[must_use]
    pub fn schema(&self) -> &SchemaChange {
        &self.schema
    }

    /// Starts a writer transaction (`BEGIN IMMEDIATE`, so the write lock is
    /// taken up front and never has to upgrade).
    pub async fn begin(&self) -> Result<Tx> {
        Ok(self.writer.begin_with("BEGIN IMMEDIATE").await?)
    }

    /// A read-only connection.
    pub async fn reader(&self) -> Result<PoolConnection<Sqlite>> {
        Ok(self.reader.acquire().await?)
    }

    /// The writer connection for a single autocommit statement.
    pub async fn writer(&self) -> Result<PoolConnection<Sqlite>> {
        Ok(self.writer.acquire().await?)
    }

    /// Closes both pools.
    pub async fn close(&self) {
        self.reader.close().await;
        self.writer.close().await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn opening_twice_separates_the_pools() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("uguisu.db");
        let cfg = DataConfig {
            data_dir: Some(dir.path().join("nested")),
            media_dir: None,
        };
        let s = Storage::open(&cfg).await.unwrap();
        assert!(path.exists());
        let mut r = s.reader().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM podcasts")
            .fetch_one(&mut *r)
            .await
            .unwrap();
        assert_eq!(n, 0);
        let err = sqlx::query(
            "INSERT INTO events (id, occurred_at, kind, payload) VALUES ('x','y','z','{}')",
        )
        .execute(&mut *r)
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("readonly") || err.to_string().contains("read-only"),
            "reader pool must be read-only: {err}"
        );
        drop(r);
        s.close().await;
        // Re-open: migrations are idempotent.
        let s2 = Storage::open(&cfg).await.unwrap();
        let mut w = s2.writer().await.unwrap();
        let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&mut *w)
            .await
            .unwrap();
        assert_eq!(
            applied, 8,
            "0001_phase3 through 0008_phase10_source_changed, all of them applied once"
        );
        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *w)
            .await
            .unwrap();
        assert_eq!(journal.to_lowercase(), "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *w)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn rolled_back_transaction_leaves_no_trace() {
        let s = Storage::open_temp().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        sqlx::query("INSERT INTO events (id, occurred_at, kind, payload) VALUES ('a','2026-01-01T00:00:00Z','k','{}')")
            .execute(&mut *tx)
            .await
            .unwrap();
        drop(tx);
        let mut r = s.reader().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&mut *r)
            .await
            .unwrap();
        assert_eq!(n, 0);
        let mut tx = s.begin().await.unwrap();
        sqlx::query("INSERT INTO events (id, occurred_at, kind, payload) VALUES ('a','2026-01-01T00:00:00Z','k','{}')")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&mut *r)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }
}
