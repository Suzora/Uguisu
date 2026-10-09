-- Phase 5: archive artifacts and per-podcast archive policies
-- (docs/DATA_MODEL.md "Phase 5 deltas", docs/ARCHIVE_ENGINE.md, ADR 0021/0023).
--
-- Conventions as in 0001/0002: ULID TEXT ids, RFC 3339 TEXT timestamps,
-- booleans as INTEGER 0/1, paths relative to the media directory with
-- POSIX separators.

-- One active artifact per episode. `relative_path` is unique across the
-- whole archive, so two episodes can never claim the same file: the
-- collision rule proposes a path, the database is the final boundary.
CREATE TABLE archive_files (
    id                  TEXT PRIMARY KEY,
    episode_id          TEXT NOT NULL UNIQUE REFERENCES episodes (id) ON DELETE CASCADE,
    podcast_id          TEXT NOT NULL REFERENCES podcasts (id) ON DELETE CASCADE,
    relative_path       TEXT NOT NULL UNIQUE,
    size_bytes          INTEGER NOT NULL,
    content_type        TEXT,
    sniffed_type        TEXT,
    hash_algo           TEXT NOT NULL DEFAULT 'sha256',
    hash_value          TEXT NOT NULL,
    mtime_unix          INTEGER,
    verification_state  TEXT NOT NULL,
    verification_reason TEXT,
    verified_at         TEXT,
    registered_at       TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE INDEX idx_archive_state   ON archive_files (verification_state, created_at DESC, id DESC);
CREATE INDEX idx_archive_podcast ON archive_files (podcast_id, created_at DESC, id DESC);
CREATE INDEX idx_archive_list    ON archive_files (created_at DESC, id DESC);

-- Per-podcast overrides of the global archive policy. A row exists only
-- where a user changed something; everything NULL falls back to the
-- configured default (ADR 0023).
CREATE TABLE archive_policies (
    podcast_id   TEXT PRIMARY KEY REFERENCES podcasts (id) ON DELETE CASCADE,
    mode         TEXT NOT NULL,
    max_backlog  INTEGER,
    max_age_days INTEGER,
    priority     INTEGER,
    updated_at   TEXT NOT NULL
);
