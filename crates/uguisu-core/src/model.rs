use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct Podcast {
    pub id: i64,
    pub feed_url: String,
    pub original_feed_url: String,
    pub podcast_guid: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub link: Option<String>,
    pub image_url: Option<String>,
    pub language: Option<String>,
    pub dir: String,
    pub cover_file: Option<String>,
    pub feed_token: String,
    pub include_removed: bool,
    pub keep_versions: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at: i64,
    pub last_fetched_at: Option<i64>,
    pub last_fetch_error: Option<String>,
}

/// A podcast plus the numbers shown in the library.
#[derive(Debug, Clone, FromRow)]
pub struct PodcastSummary {
    #[sqlx(flatten)]
    pub podcast: Podcast,
    pub episode_count: i64,
    pub archived_count: i64,
    pub archived_bytes: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct Episode {
    pub id: i64,
    pub podcast_id: i64,
    pub guid: String,
    pub title: String,
    pub description: Option<String>,
    pub link: Option<String>,
    pub published_at: Option<i64>,
    pub duration: Option<i64>,
    pub enclosure_url: Option<String>,
    pub enclosure_type: Option<String>,
    pub enclosure_length: Option<i64>,
    pub image_url: Option<String>,
    pub season: Option<i64>,
    pub episode_no: Option<i64>,
    pub dir: String,
    pub meta_hash: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub removed_at: Option<i64>,
    pub download_attempts: i64,
    pub download_error: Option<String>,
    pub next_attempt_at: i64,
    pub refetch: bool,
}

/// An episode joined with its newest intact media file, if any.
#[derive(Debug, Clone, FromRow)]
pub struct EpisodeView {
    #[sqlx(flatten)]
    pub episode: Episode,
    pub media_path: Option<String>,
    pub media_size: Option<i64>,
    pub media_mime: Option<String>,
    pub media_blake3: Option<String>,
    /// Number of archived copies, including superseded and damaged ones.
    pub media_versions: i64,
    /// Copies that failed their last integrity check.
    pub media_damaged: i64,
}

impl EpisodeView {
    /// File name of the archived media file, e.g. `media.mp3`.
    pub fn media_file_name(&self) -> Option<&str> {
        self.media_path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct MediaFile {
    pub id: i64,
    pub episode_id: i64,
    pub path: String,
    pub size: i64,
    pub blake3: String,
    pub mime: String,
    pub source_url: String,
    pub downloaded_at: i64,
    pub verified_at: i64,
    pub status: String,
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct Stats {
    pub podcasts: i64,
    pub episodes: i64,
    pub archived: i64,
    pub bytes: i64,
    /// Files that failed their last integrity check.
    pub damaged: i64,
    /// The longest any intact file has gone without a check.
    pub oldest_check: Option<i64>,
}
