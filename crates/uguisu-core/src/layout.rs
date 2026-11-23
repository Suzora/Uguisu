//! Moves an archive from the layout of formats 1 to 4 to the one of format 5.
//!
//! Before format 5 every name was a slug with a hash (`telekaddi-…-b318aa16`) and each episode
//! had one directory holding its media file next to its metadata. Format 5 names podcasts and
//! episodes as people write them and keeps the media files of a podcast together in
//! `episodes/`, the rest in `metadata/<episode>/`.
//!
//! Files are only renamed, never copied. Every target is worked out from the database alone, in
//! the order of the ids, so a run that was interrupted works out the same targets again and
//! finishes the job: a step whose source is gone and whose target exists is taken as done.

use std::path::Path;

use anyhow::{Result, bail};

use crate::archive;
use crate::service::Uguisu;
use crate::util::unique_name;

/// One rename, relative to the archive.
struct Move {
    from: String,
    to: String,
}

/// A media file of an episode: its id, its path before and after, and its checksum.
struct MediaMove {
    id: i64,
    from: String,
    to: String,
    blake3: String,
}

impl Uguisu {
    /// Brings the directories and files of every podcast into the layout of format 5 and
    /// records the new paths. Must run before anything else writes to the archive.
    pub(crate) async fn move_to_named_layout(&self) -> Result<()> {
        type PodcastRow = (i64, String, Option<String>, String);
        let podcasts: Vec<PodcastRow> =
            sqlx::query_as("SELECT id, dir, author, title FROM podcasts ORDER BY id")
                .fetch_all(self.pool())
                .await?;
        let mut names: Vec<String> = Vec::new();
        for (id, old_dir, author, title) in podcasts {
            let name = unique_name(&archive::podcast_name(author.as_deref(), &title), |n| {
                names.iter().any(|t| t.to_lowercase() == n.to_lowercase())
            });
            names.push(name.clone());
            self.move_podcast(id, &old_dir, &name).await?;
        }
        Ok(())
    }

    async fn move_podcast(&self, podcast_id: i64, old_dir: &str, new_dir: &str) -> Result<()> {
        // The podcast's directory first; everything below is then addressed by its new name.
        self.rename_once(&Move {
            from: old_dir.to_string(),
            to: new_dir.to_string(),
        })
        .await?;
        let rebase = |path: &str| match path.strip_prefix(&format!("{old_dir}/")) {
            Some(rest) => format!("{new_dir}/{rest}"),
            None => path.to_string(),
        };

        type EpisodeRow = (i64, String, Option<i64>, String);
        let episodes: Vec<EpisodeRow> = sqlx::query_as(
            "SELECT id, dir, published_at, title FROM episodes WHERE podcast_id = ? ORDER BY id",
        )
        .bind(podcast_id)
        .fetch_all(self.pool())
        .await?;
        let mut names: Vec<String> = Vec::new();
        let mut episode_dirs = Vec::new();
        let mut media_moves = Vec::new();
        let mut attachment_moves = Vec::new();
        for (episode_id, old_name, published_at, title) in episodes {
            let name = unique_name(&archive::episode_name(published_at, &title), |n| {
                names.iter().any(|t| t.to_lowercase() == n.to_lowercase())
            });
            names.push(name.clone());
            let old_episode_dir = format!("{new_dir}/episodes/{old_name}");
            let metadata = archive::metadata_dir(new_dir, &name);
            self.rename_once(&Move {
                from: old_episode_dir.clone(),
                to: metadata.clone(),
            })
            .await?;
            // A path below the old episode directory, now below the metadata directory.
            let moved = |path: &str| {
                let path = rebase(path);
                match path.strip_prefix(&format!("{old_episode_dir}/")) {
                    Some(rest) => format!("{metadata}/{rest}"),
                    None => path,
                }
            };

            // The newest intact media file is the current one and goes to `episodes/`; the
            // others are earlier versions.
            type MediaRow = (i64, String, String, String);
            let media: Vec<MediaRow> = sqlx::query_as(
                "SELECT id, path, blake3, status FROM media_files WHERE episode_id = ? ORDER BY id",
            )
            .bind(episode_id)
            .fetch_all(self.pool())
            .await?;
            let current = media
                .iter()
                .rev()
                .find(|(_, _, _, status)| status == "ok")
                .or(media.last())
                .map(|(id, ..)| *id);
            let mut version = 0;
            for (id, path, blake3, _) in &media {
                let from = moved(path);
                let ext = path.rsplit_once('.').map_or("bin", |(_, ext)| ext);
                let to = if Some(*id) == current {
                    archive::media_path(new_dir, &name, ext)
                } else {
                    version += 1;
                    format!("{metadata}/versions/v{version}.{ext}")
                };
                if from != to {
                    self.rename_once(&Move {
                        from: from.clone(),
                        to: to.clone(),
                    })
                    .await?;
                }
                media_moves.push(MediaMove {
                    id: *id,
                    from,
                    to,
                    blake3: blake3.clone(),
                });
            }

            let attachments: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, path FROM attachments WHERE episode_id = ? AND path IS NOT NULL",
            )
            .bind(episode_id)
            .fetch_all(self.pool())
            .await?;
            for (id, path) in attachments {
                attachment_moves.push((id, moved(&path)));
            }
            episode_dirs.push((episode_id, name));
        }

        // Checksum files follow their media files. The old ones lie in the metadata directory
        // under the old names.
        // The old ones go first: an old name can be the new name of another file's checksum.
        let new_names: Vec<String> = media_moves
            .iter()
            .map(|m| archive::checksum_location(&m.to).0)
            .collect();
        for m in &media_moves {
            let old = format!("{}.blake3", m.from);
            if !new_names.contains(&old) {
                let _ = tokio::fs::remove_file(self.archive_path(&old)).await;
            }
        }
        for m in &media_moves {
            self.write_checksum(&m.to, &m.blake3).await?;
        }

        // Only now the database: a run that stops before this point starts over from the old
        // paths and finds the renames done.
        let mut tx = self.pool().begin().await?;
        sqlx::query("UPDATE podcasts SET dir = ? WHERE id = ?")
            .bind(new_dir)
            .bind(podcast_id)
            .execute(&mut *tx)
            .await?;
        let snapshots: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, path FROM feed_snapshots WHERE podcast_id = ?")
                .bind(podcast_id)
                .fetch_all(&mut *tx)
                .await?;
        for (id, path) in snapshots {
            sqlx::query("UPDATE feed_snapshots SET path = ? WHERE id = ?")
                .bind(rebase(&path))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        for (id, name) in &episode_dirs {
            sqlx::query("UPDATE episodes SET dir = ? WHERE id = ?")
                .bind(name)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        // Paths are unique; a temporary prefix keeps two files from swapping places mid-way.
        for m in &media_moves {
            sqlx::query("UPDATE media_files SET path = ? WHERE id = ?")
                .bind(format!("\u{0}{}", m.to))
                .bind(m.id)
                .execute(&mut *tx)
                .await?;
        }
        for m in &media_moves {
            sqlx::query("UPDATE media_files SET path = ? WHERE id = ?")
                .bind(&m.to)
                .bind(m.id)
                .execute(&mut *tx)
                .await?;
        }
        for (id, path) in &attachment_moves {
            sqlx::query("UPDATE attachments SET path = ? WHERE id = ?")
                .bind(path)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        // The old `episodes/<episode>/` directories are empty now, unless something nobody
        // recorded was left in one.
        Ok(())
    }

    /// Renames once: a source that is gone with its target in place counts as done; source
    /// and target both present is a conflict that is left for a person to look at.
    async fn rename_once(&self, step: &Move) -> Result<()> {
        if step.from == step.to {
            return Ok(());
        }
        let from = self.archive_path(&step.from);
        let to = self.archive_path(&step.to);
        let from_exists = exists(&from).await?;
        let to_exists = exists(&to).await?;
        match (from_exists, to_exists) {
            (true, false) => {
                if let Some(parent) = to.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::rename(&from, &to).await?;
                Ok(())
            }
            (false, _) => Ok(()),
            (true, true) => {
                // A case-only rename on a file system that ignores case: same file.
                if step.from.to_lowercase() == step.to.to_lowercase() {
                    tokio::fs::rename(&from, &to).await?;
                    return Ok(());
                }
                bail!(
                    "cannot move {} to {}: both exist",
                    from.display(),
                    to.display()
                )
            }
        }
    }
}

async fn exists(path: &Path) -> Result<bool> {
    Ok(tokio::fs::try_exists(path).await?)
}
