-- Phase 4: download jobs, attempt history and the global queue control
-- (docs/DATA_MODEL.md "Phase 4 deltas", docs/DOWNLOAD_ENGINE.md, ADR 0018).
--
-- Conventions as in 0001: ULID TEXT ids, RFC 3339 TEXT timestamps,
-- booleans as INTEGER 0/1, paths relative to the media directory with
-- POSIX separators.

CREATE TABLE download_jobs (
    id                TEXT PRIMARY KEY,
    episode_id        TEXT NOT NULL UNIQUE REFERENCES episodes (id) ON DELETE CASCADE,
    podcast_id        TEXT NOT NULL REFERENCES podcasts (id) ON DELETE CASCADE,
    enclosure_id      TEXT,
    source_url        TEXT NOT NULL,
    host_key          TEXT NOT NULL,
    state             TEXT NOT NULL,
    state_reason      TEXT,
    priority          INTEGER NOT NULL DEFAULT 1,
    attempt_count     INTEGER NOT NULL DEFAULT 0,
    max_attempts      INTEGER NOT NULL,
    next_attempt_at   TEXT,
    bytes_downloaded  INTEGER NOT NULL DEFAULT 0,
    total_bytes       INTEGER,
    part_path         TEXT NOT NULL,
    target_path       TEXT NOT NULL,
    content_type      TEXT,
    sniffed_type      TEXT,
    etag              TEXT,
    last_modified     TEXT,
    accept_ranges     INTEGER,
    hash_algo         TEXT NOT NULL DEFAULT 'sha256',
    hash_value        TEXT,
    last_http_status  INTEGER,
    last_error_kind   TEXT,
    last_error_detail TEXT,
    claimed_at        TEXT,
    progress_at       TEXT,
    started_at        TEXT,
    finished_at       TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);
CREATE INDEX idx_jobs_claim   ON download_jobs (state, priority DESC, created_at, id);
CREATE INDEX idx_jobs_due     ON download_jobs (next_attempt_at) WHERE state = 'retrying';
CREATE INDEX idx_jobs_podcast ON download_jobs (podcast_id, created_at DESC, id DESC);
CREATE INDEX idx_jobs_list    ON download_jobs (created_at DESC, id DESC);

CREATE TABLE download_attempts (
    id              TEXT PRIMARY KEY,
    job_id          TEXT NOT NULL REFERENCES download_jobs (id) ON DELETE CASCADE,
    attempt_no      INTEGER NOT NULL,
    started_at      TEXT NOT NULL,
    finished_at     TEXT,
    source_url      TEXT NOT NULL,
    range_start     INTEGER NOT NULL DEFAULT 0,
    http_status     INTEGER,
    bytes_received  INTEGER NOT NULL DEFAULT 0,
    duration_ms     INTEGER NOT NULL DEFAULT 0,
    avg_rate_bps    INTEGER,
    outcome         TEXT,
    error_kind      TEXT,
    error_detail    TEXT,
    next_attempt_at TEXT
);
CREATE UNIQUE INDEX idx_attempts_job ON download_attempts (job_id, attempt_no);

CREATE TABLE download_control (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    paused        INTEGER NOT NULL DEFAULT 0,
    paused_reason TEXT,
    paused_at     TEXT,
    updated_at    TEXT NOT NULL
);
INSERT INTO download_control (id, paused, updated_at)
VALUES (1, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
