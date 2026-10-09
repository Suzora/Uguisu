-- Phase 3: podcasts, sources with fetch state, episodes, enclosures,
-- extras, change log, fetch log and events. See docs/DATA_MODEL.md
-- ("Phase 3 deltas") and ADR 0002.
--
-- Conventions: ids are ULIDs (TEXT), timestamps are UTC RFC 3339 with
-- second precision and a trailing Z (TEXT sorts chronologically), JSON
-- columns hold rarely-queried structures, booleans are INTEGER 0/1.

CREATE TABLE podcasts (
    id                    TEXT PRIMARY KEY,
    title                 TEXT NOT NULL,
    sort_title            TEXT NOT NULL,
    subtitle              TEXT,
    author                TEXT,
    publisher             TEXT,
    owner_name            TEXT,
    owner_email           TEXT,
    description_html      TEXT,
    description_text      TEXT,
    website               TEXT,
    artwork_url           TEXT,
    language              TEXT,
    categories            TEXT NOT NULL DEFAULT '[]',
    explicit              INTEGER,
    copyright             TEXT,
    podcast_guid          TEXT,
    feed_kind             TEXT NOT NULL,
    status                TEXT NOT NULL DEFAULT 'active',
    refresh_interval_secs INTEGER,
    next_refresh_at       TEXT,
    last_refresh_at       TEXT,
    last_error            TEXT,
    directory_name        TEXT,
    metadata_hash         TEXT NOT NULL DEFAULT '',
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);
CREATE INDEX idx_podcasts_sort_title ON podcasts (sort_title);
CREATE INDEX idx_podcasts_status_next ON podcasts (status, next_refresh_at);
CREATE INDEX idx_podcasts_guid ON podcasts (podcast_guid);

CREATE TABLE podcast_sources (
    id                     TEXT PRIMARY KEY,
    podcast_id             TEXT NOT NULL REFERENCES podcasts (id) ON DELETE CASCADE,
    feed_url               TEXT NOT NULL,
    canonical_url          TEXT,
    website_url            TEXT,
    provider               TEXT NOT NULL,
    provider_ref           TEXT,
    discovered_at          TEXT NOT NULL,
    verified_at            TEXT,
    is_current             INTEGER NOT NULL DEFAULT 1,
    replaced_by_source_id  TEXT,
    replacement_reason     TEXT,
    fetch_state            TEXT NOT NULL DEFAULT 'never_fetched',
    last_attempt_at        TEXT,
    last_success_at        TEXT,
    last_not_modified_at   TEXT,
    last_error_at          TEXT,
    consecutive_failures   INTEGER NOT NULL DEFAULT 0,
    last_http_status       INTEGER,
    last_error_kind        TEXT,
    last_error_detail      TEXT,
    http_etag              TEXT,
    http_last_modified     TEXT,
    content_fingerprint    TEXT,
    last_content_length    INTEGER,
    created_at             TEXT NOT NULL,
    updated_at             TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_sources_current ON podcast_sources (podcast_id) WHERE is_current = 1;
CREATE INDEX idx_sources_podcast ON podcast_sources (podcast_id);
CREATE INDEX idx_sources_feed_url ON podcast_sources (feed_url);

CREATE TABLE episodes (
    id                      TEXT PRIMARY KEY,
    podcast_id              TEXT NOT NULL REFERENCES podcasts (id) ON DELETE CASCADE,
    guid                    TEXT,
    guid_is_permalink       INTEGER,
    identity_key            TEXT NOT NULL,
    identity_source         TEXT NOT NULL,
    guid_key                TEXT,
    enclosure_key           TEXT,
    fingerprint_key         TEXT,
    identity_reason         TEXT NOT NULL DEFAULT '',
    title                   TEXT NOT NULL,
    subtitle                TEXT,
    sort_title              TEXT NOT NULL,
    description_html        TEXT,
    description_text        TEXT,
    link                    TEXT,
    published_at            TEXT,
    published_at_raw        TEXT,
    published_at_quality    TEXT NOT NULL DEFAULT 'invalid',
    updated_at_source       TEXT,
    duration_secs           INTEGER,
    duration_raw            TEXT,
    season                  INTEGER,
    episode_number          INTEGER,
    episode_type            TEXT,
    explicit                INTEGER,
    artwork_url             TEXT,
    author                  TEXT,
    content_hash            TEXT NOT NULL,
    archive_state           TEXT NOT NULL DEFAULT 'expected',
    skip_reason             TEXT,
    malformed               INTEGER NOT NULL DEFAULT 0,
    malformed_reason        TEXT,
    duplicate_of_episode_id TEXT,
    duplicate_reasons       TEXT NOT NULL DEFAULT '[]',
    missing_streak          INTEGER NOT NULL DEFAULT 0,
    first_seen_at           TEXT NOT NULL,
    last_seen_in_feed_at    TEXT NOT NULL,
    removed_from_feed_at    TEXT,
    sort_at                 TEXT NOT NULL,
    source_metadata         TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_episodes_identity ON episodes (podcast_id, identity_key);
CREATE INDEX idx_episodes_sort ON episodes (podcast_id, sort_at DESC, id);
CREATE INDEX idx_episodes_enclosure_key ON episodes (podcast_id, enclosure_key);
CREATE INDEX idx_episodes_fingerprint_key ON episodes (podcast_id, fingerprint_key);
CREATE INDEX idx_episodes_archive_state ON episodes (archive_state);

CREATE TABLE enclosures (
    id              TEXT PRIMARY KEY,
    episode_id      TEXT NOT NULL REFERENCES episodes (id) ON DELETE CASCADE,
    url             TEXT NOT NULL,
    mime_type       TEXT,
    length_bytes    INTEGER,
    is_primary      INTEGER NOT NULL DEFAULT 0,
    kind            TEXT NOT NULL DEFAULT 'other',
    position        INTEGER NOT NULL DEFAULT 0,
    bitrate         INTEGER,
    height          INTEGER,
    codecs          TEXT,
    lang            TEXT,
    title           TEXT,
    integrity_type  TEXT,
    integrity_value TEXT,
    sources         TEXT NOT NULL DEFAULT '[]',
    UNIQUE (episode_id, url)
);
CREATE INDEX idx_enclosures_episode ON enclosures (episode_id);

CREATE TABLE episode_extras (
    episode_id TEXT PRIMARY KEY REFERENCES episodes (id) ON DELETE CASCADE,
    data       TEXT NOT NULL
);

CREATE TABLE episode_changes (
    id         TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES episodes (id) ON DELETE CASCADE,
    podcast_id TEXT NOT NULL,
    fetch_id   TEXT,
    changed_at TEXT NOT NULL,
    field      TEXT NOT NULL,
    old_value  TEXT,
    new_value  TEXT
);
CREATE INDEX idx_changes_episode ON episode_changes (episode_id, changed_at);
CREATE INDEX idx_changes_podcast ON episode_changes (podcast_id, changed_at);

CREATE TABLE feed_fetches (
    id                    TEXT PRIMARY KEY,
    podcast_id            TEXT NOT NULL REFERENCES podcasts (id) ON DELETE CASCADE,
    source_id             TEXT NOT NULL,
    fetched_at            TEXT NOT NULL,
    outcome               TEXT NOT NULL,
    http_status           INTEGER,
    etag                  TEXT,
    last_modified         TEXT,
    bytes                 INTEGER,
    duration_ms           INTEGER NOT NULL DEFAULT 0,
    error_kind            TEXT,
    error_detail          TEXT,
    partial               INTEGER NOT NULL DEFAULT 0,
    truncated             INTEGER NOT NULL DEFAULT 0,
    fingerprint_changed   INTEGER NOT NULL DEFAULT 0,
    items_seen            INTEGER NOT NULL DEFAULT 0,
    items_added           INTEGER NOT NULL DEFAULT 0,
    items_updated         INTEGER NOT NULL DEFAULT 0,
    items_unchanged       INTEGER NOT NULL DEFAULT 0,
    items_malformed       INTEGER NOT NULL DEFAULT 0,
    items_removed         INTEGER NOT NULL DEFAULT 0,
    items_ambiguous       INTEGER NOT NULL DEFAULT 0,
    podcast_changed       INTEGER NOT NULL DEFAULT 0,
    url_change_detected   INTEGER NOT NULL DEFAULT 0,
    warnings              TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX idx_fetches_podcast ON feed_fetches (podcast_id, fetched_at DESC);

CREATE TABLE events (
    id          TEXT PRIMARY KEY,
    occurred_at TEXT NOT NULL,
    kind        TEXT NOT NULL,
    podcast_id  TEXT,
    episode_id  TEXT,
    payload     TEXT NOT NULL
);
CREATE INDEX idx_events_time ON events (occurred_at, id);
CREATE INDEX idx_events_podcast ON events (podcast_id, occurred_at);
