//! Database maintenance (ADR 0056): the schema this process migrated, a
//! consistent copy, the integrity checks and `VACUUM`. Each runs only when
//! asked for.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_archive::layout;
use uguisu_core::UguisuError;
use uguisu_storage::maintenance;

use crate::Engine;

/// Lines a check keeps of each kind of finding.
const FINDINGS: u32 = 100;

/// Where a backup without a destination goes, under the data directory.
pub const BACKUPS_DIR: &str = "backups";

/// A copy of the database (ADR 0056).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DbBackup {
    /// The copy, on the machine that wrote it.
    pub path: String,
    /// Its size.
    pub bytes: u64,
}

/// What `PRAGMA integrity_check` and `PRAGMA foreign_key_check` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DbCheck {
    /// Neither check found anything.
    pub ok: bool,
    /// `integrity_check`'s findings, up to a hundred.
    pub integrity: Vec<String>,
    /// Rows naming a parent that is not there, up to a hundred.
    pub foreign_keys: Vec<String>,
}

/// The database's size before and after a `VACUUM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DbVacuum {
    /// Bytes before.
    pub bytes_before: u64,
    /// Bytes after.
    pub bytes_after: u64,
}

impl Engine {
    /// Writes a consistent copy of the database to `dest`, or under
    /// `<data dir>/backups/` with a name of its own. An existing file is
    /// refused, never overwritten.
    pub async fn backup_database(&self, dest: Option<&Path>) -> Result<DbBackup, UguisuError> {
        let dest = if let Some(path) = dest {
            path.to_path_buf()
        } else {
            let t = OffsetDateTime::now_utc();
            self.backups_dir()?.join(format!(
                "uguisu-{:04}{:02}{:02}T{:02}{:02}{:02}Z.db",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            ))
        };
        if dest.exists() {
            return Err(UguisuError::Conflict(format!(
                "{} exists; a backup never overwrites a file",
                dest.display()
            )));
        }
        let parent = dest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let io = |what: &Path, e: std::io::Error| UguisuError::Io {
            path: what.display().to_string(),
            detail: e.to_string(),
        };
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        let mut tmp = dest.clone().into_os_string();
        tmp.push(layout::tmp_suffix());
        let tmp = PathBuf::from(tmp);
        {
            let mut conn = self.storage().reader().await?;
            maintenance::copy_into(&mut conn, &tmp).await?;
        }
        // A copy that failed leaves its `.tmp` behind: nothing here deletes.
        // Write access: Windows refuses to flush a handle opened read-only.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&tmp)
            .map_err(|e| io(&tmp, e))?;
        file.sync_all().map_err(|e| io(&tmp, e))?;
        let bytes = file.metadata().map_err(|e| io(&tmp, e))?.len();
        drop(file);
        std::fs::rename(&tmp, &dest).map_err(|e| io(&dest, e))?;
        crate::archive::sync_dir(Some(parent));
        tracing::info!(path = %dest.display(), bytes, "database backed up");
        Ok(DbBackup {
            path: dest.display().to_string(),
            bytes,
        })
    }

    /// Runs `PRAGMA integrity_check` and `PRAGMA foreign_key_check`. Reads
    /// every page; changes nothing.
    pub async fn check_database(&self) -> Result<DbCheck, UguisuError> {
        let mut conn = self.storage().reader().await?;
        let (integrity, foreign_keys) = maintenance::check(&mut conn, FINDINGS).await?;
        Ok(DbCheck {
            ok: integrity.is_empty() && foreign_keys.is_empty(),
            integrity,
            foreign_keys,
        })
    }

    /// Rebuilds the database file without its free pages. Writes wait for it.
    pub async fn vacuum_database(&self) -> Result<DbVacuum, UguisuError> {
        let mut conn = self.storage().writer().await?;
        let bytes_before = maintenance::size(&mut conn).await?;
        maintenance::vacuum(&mut conn).await?;
        let bytes_after = maintenance::size(&mut conn).await?;
        tracing::info!(bytes_before, bytes_after, "database vacuumed");
        Ok(DbVacuum {
            bytes_before,
            bytes_after,
        })
    }

    fn backups_dir(&self) -> Result<PathBuf, UguisuError> {
        self.storage()
            .path()
            .parent()
            .map(|dir| dir.join(BACKUPS_DIR))
            .ok_or_else(|| UguisuError::Config("the database has no directory".to_owned()))
    }
}
