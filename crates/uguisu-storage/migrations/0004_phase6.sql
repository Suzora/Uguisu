-- Phase 6: archive portability, provenance and assets
-- (docs/ARCHIVE_ENGINE.md, docs/DATA_MODEL.md "Phase 6 deltas", ADR 0024/0026).
--
-- Conventions as in 0001-0003: ULID TEXT ids, RFC 3339 TEXT timestamps,
-- booleans as INTEGER 0/1, paths relative to the media directory with
-- POSIX separators.

-- Provenance and metadata state for the artifacts Phase 5 already records.
--
-- `hash_value` keeps meaning "the bytes on disk now", so every Phase-5
-- verification path is untouched. The new `source_*` columns record what
-- was received, which is what tells a file Aviary retagged apart from one
-- that was altered behind its back. They are added nullable because SQLite
-- cannot add a NOT NULL column without a constant default, and a constant
-- here would be a lie; the backfill below makes them non-null in practice.
ALTER TABLE archive_files ADD COLUMN source_size_bytes  INTEGER;
ALTER TABLE archive_files ADD COLUMN source_hash_algo   TEXT;
ALTER TABLE archive_files ADD COLUMN source_hash_value  TEXT;
ALTER TABLE archive_files ADD COLUMN origin             TEXT NOT NULL DEFAULT 'download';
-- Written *before* the media file is touched, so a crash cannot forge it:
-- after an interrupted retag this is the only thing that distinguishes
-- "Aviary was mid-write" from "someone changed the file".
ALTER TABLE archive_files ADD COLUMN tag_state          TEXT NOT NULL DEFAULT 'untagged';
ALTER TABLE archive_files ADD COLUMN tag_mode           TEXT;
ALTER TABLE archive_files ADD COLUMN tagged_at          TEXT;
ALTER TABLE archive_files ADD COLUMN sidecar_written_at TEXT;

-- For every pre-Phase-6 row the two are the same: nothing had ever
-- rewritten an archived file.
UPDATE archive_files
   SET source_size_bytes = size_bytes,
       source_hash_algo  = hash_algo,
       source_hash_value = hash_value
 WHERE source_hash_value IS NULL;

-- Both partial and normally empty: recovery scans must stay O(work to do),
-- not O(archive), so that startup does not slow down as the archive grows.
CREATE INDEX idx_archive_tag_state  ON archive_files (tag_state)
    WHERE tag_state <> 'untagged';
CREATE INDEX idx_archive_no_sidecar ON archive_files (created_at, id)
    WHERE sidecar_written_at IS NULL;

-- Podcast artwork, content-addressed: the file is named after its hash, so
-- fetching a replacement can never destroy the previous one. Exactly one
-- row per podcast may be current, which the partial unique index enforces
-- rather than leaving it to a check that could race.
CREATE TABLE podcast_artwork (
    id            TEXT PRIMARY KEY,
    podcast_id    TEXT NOT NULL REFERENCES podcasts (id) ON DELETE CASCADE,
    source_url    TEXT,
    relative_path TEXT NOT NULL UNIQUE,
    format        TEXT NOT NULL,
    content_type  TEXT,
    size_bytes    INTEGER NOT NULL,
    hash_algo     TEXT NOT NULL DEFAULT 'sha256',
    hash_value    TEXT NOT NULL,
    etag          TEXT,
    last_modified TEXT,
    is_current    INTEGER NOT NULL DEFAULT 0,
    retrieved_at  TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (podcast_id, hash_value)
);
CREATE UNIQUE INDEX idx_artwork_current ON podcast_artwork (podcast_id)
    WHERE is_current = 1;
CREATE INDEX idx_artwork_podcast ON podcast_artwork (podcast_id, retrieved_at DESC, id DESC);

-- One manifest per podcast. The manifest file is derived data; this row is
-- what says whether it still matches the index. `stale` is set inside the
-- same transaction that changes an artifact, so a crash can only ever
-- leave "marked stale but actually fresh" - never the reverse.
CREATE TABLE archive_manifests (
    podcast_id    TEXT PRIMARY KEY REFERENCES podcasts (id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    entries       INTEGER NOT NULL DEFAULT 0,
    hash_value    TEXT,
    stale         INTEGER NOT NULL DEFAULT 1,
    generated_at  TEXT,
    stale_since   TEXT,
    updated_at    TEXT NOT NULL
);
CREATE INDEX idx_manifests_stale ON archive_manifests (podcast_id) WHERE stale = 1;
