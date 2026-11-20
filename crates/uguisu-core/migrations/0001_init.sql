-- Uguisu schema. The database is an index over the archive directory;
-- everything needed to understand the archive also lives on disk as sidecar files.

CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE TABLE podcasts (
    id                INTEGER PRIMARY KEY,
    feed_url          TEXT NOT NULL UNIQUE,      -- where we fetch from today
    original_feed_url TEXT NOT NULL,             -- what the user originally added
    podcast_guid      TEXT,                      -- <podcast:guid>, survives feed moves
    title             TEXT NOT NULL,
    author            TEXT,
    description       TEXT,
    link              TEXT,
    image_url         TEXT,
    language          TEXT,
    dir               TEXT NOT NULL UNIQUE,      -- directory below archive/
    cover_file        TEXT,                      -- file name inside dir
    feed_token        TEXT NOT NULL UNIQUE,      -- secret part of the published feed URL
    include_removed   INTEGER NOT NULL DEFAULT 1,
    etag              TEXT,
    last_modified     TEXT,
    created_at        INTEGER NOT NULL,
    last_fetched_at   INTEGER,
    last_fetch_error  TEXT
) STRICT;

CREATE TABLE feed_snapshots (
    id         INTEGER PRIMARY KEY,
    podcast_id INTEGER NOT NULL REFERENCES podcasts(id) ON DELETE CASCADE,
    fetched_at INTEGER NOT NULL,
    blake3     TEXT NOT NULL,
    size       INTEGER NOT NULL,
    path       TEXT NOT NULL,                    -- relative to archive/
    UNIQUE (podcast_id, blake3)
) STRICT;

CREATE TABLE episodes (
    id                INTEGER PRIMARY KEY,
    podcast_id        INTEGER NOT NULL REFERENCES podcasts(id) ON DELETE CASCADE,
    guid              TEXT NOT NULL,
    title             TEXT NOT NULL,
    description       TEXT,
    link              TEXT,
    published_at      INTEGER,
    duration          INTEGER,
    enclosure_url     TEXT,
    enclosure_type    TEXT,
    enclosure_length  INTEGER,
    image_url         TEXT,
    season            INTEGER,
    episode_no        INTEGER,
    dir               TEXT NOT NULL,             -- directory below <podcast dir>/episodes/
    meta_hash         TEXT NOT NULL,
    first_seen_at     INTEGER NOT NULL,
    last_seen_at      INTEGER NOT NULL,
    removed_at        INTEGER,                   -- set once the episode left the source feed
    download_attempts INTEGER NOT NULL DEFAULT 0,
    download_error    TEXT,
    next_attempt_at   INTEGER NOT NULL DEFAULT 0,
    UNIQUE (podcast_id, guid)
) STRICT;

CREATE INDEX episodes_by_podcast ON episodes (podcast_id, published_at DESC);

-- Every distinct state of an episode's metadata, including the raw <item> XML.
CREATE TABLE episode_revisions (
    id         INTEGER PRIMARY KEY,
    episode_id INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    seen_at    INTEGER NOT NULL,
    meta_hash  TEXT NOT NULL,
    raw_xml    TEXT NOT NULL,
    UNIQUE (episode_id, meta_hash)
) STRICT;

-- One row per archived file. An episode may accumulate several versions.
CREATE TABLE media_files (
    id            INTEGER PRIMARY KEY,
    episode_id    INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    path          TEXT NOT NULL UNIQUE,          -- relative to archive/
    size          INTEGER NOT NULL,
    blake3        TEXT NOT NULL,
    mime          TEXT NOT NULL,
    source_url    TEXT NOT NULL,
    downloaded_at INTEGER NOT NULL,
    verified_at   INTEGER NOT NULL,
    status        TEXT NOT NULL DEFAULT 'ok'     -- ok | corrupt | missing
) STRICT;

CREATE INDEX media_by_episode ON media_files (episode_id);
