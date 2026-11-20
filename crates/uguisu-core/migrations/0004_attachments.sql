-- Files an episode links to besides its media: transcripts, chapters, its own image.
CREATE TABLE attachments (
    id              INTEGER PRIMARY KEY,
    episode_id      INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,               -- transcript | chapters | image
    source_key      TEXT NOT NULL,               -- source address without query string
    source_url      TEXT NOT NULL,               -- source address as last seen
    path            TEXT,                        -- relative to archive/, once downloaded
    size            INTEGER,
    blake3          TEXT,
    mime            TEXT,
    downloaded_at   INTEGER,
    verified_at     INTEGER,
    status          TEXT NOT NULL DEFAULT 'pending', -- pending | ok | corrupt | missing
    attempts        INTEGER NOT NULL DEFAULT 0,
    error           TEXT,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    UNIQUE (episode_id, source_key)
) STRICT;

CREATE INDEX attachments_by_status ON attachments (status, next_attempt_at);
