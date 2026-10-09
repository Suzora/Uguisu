-- Phase 7: the service, the scheduler and the things that outlive a process
-- (docs/SERVICE.md, ADR 0027/0028/0029/0030).
--
-- Conventions as in 0001-0004: ULID TEXT ids, RFC 3339 TEXT timestamps,
-- booleans as INTEGER 0/1, JSON in TEXT columns, paths relative to the
-- media directory with POSIX separators.

-- ---------------------------------------------------------------- scheduler

-- One row, like `download_control` (0002). Deliberately *not* that table:
-- a full disk pauses downloads automatically, and feeds must keep being
-- refreshed when transfers stop - an operator pausing transfers is not
-- asking to go stale.
--
-- `last_maintenance_at` is what lets a restart re-derive the next
-- maintenance time instead of running the daily work on every boot.
CREATE TABLE scheduler_control (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    paused              INTEGER NOT NULL DEFAULT 0,
    paused_reason       TEXT,
    paused_at           TEXT,
    last_maintenance_at TEXT,
    updated_at          TEXT NOT NULL
);
INSERT INTO scheduler_control (id, paused, updated_at)
VALUES (1, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

-- The index the due query rides. `idx_podcasts_status_next` (0001) stays:
-- it serves status filters. This one is partial and ordered the other way
-- round, so "what is due next" is a seek rather than a scan.
--
-- The predicate is spelled `OR` rather than `IN` on purpose: SQLite's
-- partial-index prover matches the query's WHERE against the index's, and
-- the due query repeats this clause verbatim. A test asserts the plan.
CREATE INDEX idx_podcasts_due ON podcasts (next_refresh_at, id)
    WHERE status = 'active' OR status = 'error';

-- ----------------------------------------------------------------- settings

-- The persisted configuration layer (ADR 0028). `value` holds exactly the
-- text the matching environment variable would hold - not JSON, as the
-- Phase-1 sketch had it, because a second syntax would mean a second
-- parser and a second way to be wrong. `key` is the `AVIARY_*` name, so
-- there is one vocabulary rather than two.
--
-- A value that no longer parses is quarantined, never deleted: the row is
-- the only copy of what the user meant.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL CHECK (length(value) <= 4096),
    updated_at TEXT NOT NULL,
    updated_by TEXT
);

-- ---------------------------------------------------------------- discovery

-- The provider cache, surviving restarts (ADR 0030). The in-memory tier
-- keeps a monotonic `Instant`; a row has to carry wall-clock times, which
-- is why this is a separate representation rather than a dump of the other.
CREATE TABLE discovery_cache (
    provider   TEXT NOT NULL,
    kind       TEXT NOT NULL,
    query_key  TEXT NOT NULL,
    payload    TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    PRIMARY KEY (provider, kind, query_key)
);
CREATE INDEX idx_discovery_cache_expiry ON discovery_cache (expires_at);

-- What a resolution decided and why. Provenance only: nothing here is ever
-- read as a feed source, and a row never turns into a podcast by itself.
-- `podcast_id` is set only when the user went on to add one, and survives
-- that podcast's deletion as a NULL rather than taking the record with it.
CREATE TABLE discovery_records (
    id           TEXT PRIMARY KEY,
    input        TEXT NOT NULL,
    provider     TEXT,
    provider_ref TEXT,
    feed_url     TEXT,
    website      TEXT,
    status       TEXT NOT NULL,
    detail       TEXT,
    steps        TEXT NOT NULL DEFAULT '[]',
    warnings     TEXT NOT NULL DEFAULT '[]',
    podcast_id   TEXT REFERENCES podcasts (id) ON DELETE SET NULL,
    resolved_at  TEXT NOT NULL,
    created_at   TEXT NOT NULL
);
CREATE INDEX idx_discovery_records_time ON discovery_records (resolved_at DESC, id DESC);
CREATE INDEX idx_discovery_records_podcast ON discovery_records (podcast_id)
    WHERE podcast_id IS NOT NULL;

-- ------------------------------------------------------------------- search

-- Full-text search (ADR 0029). Content-owning tables carrying the ULID as
-- an UNINDEXED column, not `content='episodes'`: our primary keys are TEXT
-- ULIDs, so the base tables have an *implicit* rowid that VACUUM may
-- renumber, and an external-content index keyed on it would silently point
-- at the wrong rows afterwards. Owning the text costs disk and is honest.
--
-- Only the first SEARCH_EXCERPT_CHARS (4000) characters of a description
-- are indexed - a bound on an input the publisher chooses, not a guess.
CREATE VIRTUAL TABLE episodes_fts USING fts5(
    episode_id UNINDEXED,
    podcast_id UNINDEXED,
    title,
    subtitle,
    description_text,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3 4'
);
CREATE VIRTUAL TABLE podcasts_fts USING fts5(
    podcast_id UNINDEXED,
    title,
    author,
    description_text,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);

-- An UNINDEXED column has no index, so `DELETE ... WHERE episode_id = ?`
-- would scan the whole index - once per episode, inside a 500-row refresh
-- batch. This map gives every episode a stable INTEGER to use as the FTS
-- rowid. AUTOINCREMENT because a plain rowid may be reused after a delete,
-- and a reused number would resurrect a deleted episode's text.
--
-- `podcasts_fts` gets no map: podcast text changes are rare and the table
-- is bounded by the size of the library, so the scan is affordable there.
CREATE TABLE episode_search_ids (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    episode_id TEXT NOT NULL UNIQUE REFERENCES episodes (id) ON DELETE CASCADE
);

-- Whether the index can be trusted. The migration creates the tables but
-- does not fill them: migrations run inside `Engine::open`, and building
-- 100k rows there would be tens of seconds that an interrupted start rolls
-- back and repeats forever. The engine builds it in batches instead, and a
-- search during the build says so rather than returning nothing.
CREATE TABLE search_index_state (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    state      TEXT NOT NULL,
    built_at   TEXT,
    podcasts   INTEGER NOT NULL DEFAULT 0,
    episodes   INTEGER NOT NULL DEFAULT 0,
    detail     TEXT,
    updated_at TEXT NOT NULL
);
INSERT INTO search_index_state (id, state, updated_at)
VALUES (1, 'stale', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

-- Synchronisation is the database's job, not the repository's. Five
-- writers already touch `episodes` and more arrive later; a trigger is a
-- rule that also holds for an import, for a migration and for a person
-- with the `sqlite3` CLI.
--
-- `UPDATE OF ... WHEN old IS NOT new` is what keeps it cheap: a refresh
-- rewrites every podcast column on every run, and the narrow episode
-- updates (archive_state, missing_streak, ...) never name a text column,
-- so neither costs an index write.
--
-- The id map is guarded with `WHERE NOT EXISTS` rather than `INSERT OR
-- IGNORE`: inside a trigger body SQLite discards the body's own conflict
-- policy and applies the *outer* statement's instead, and the outer
-- statement here is the episode upsert's `ON CONFLICT ... DO UPDATE`. An
-- `OR IGNORE` would therefore raise on the second refresh of an episode.

CREATE TRIGGER episodes_fts_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episode_search_ids (episode_id)
    SELECT new.id
     WHERE NOT EXISTS (SELECT 1 FROM episode_search_ids WHERE episode_id = new.id);
    DELETE FROM episodes_fts
     WHERE rowid = (SELECT seq FROM episode_search_ids WHERE episode_id = new.id);
    INSERT INTO episodes_fts (rowid, episode_id, podcast_id, title, subtitle, description_text)
    VALUES ((SELECT seq FROM episode_search_ids WHERE episode_id = new.id),
            new.id, new.podcast_id, new.title, new.subtitle,
            substr(new.description_text, 1, 4000));
END;

CREATE TRIGGER episodes_fts_au AFTER UPDATE OF title, subtitle, description_text ON episodes
WHEN old.title IS NOT new.title
  OR old.subtitle IS NOT new.subtitle
  OR old.description_text IS NOT new.description_text
BEGIN
    INSERT INTO episode_search_ids (episode_id)
    SELECT new.id
     WHERE NOT EXISTS (SELECT 1 FROM episode_search_ids WHERE episode_id = new.id);
    DELETE FROM episodes_fts
     WHERE rowid = (SELECT seq FROM episode_search_ids WHERE episode_id = new.id);
    INSERT INTO episodes_fts (rowid, episode_id, podcast_id, title, subtitle, description_text)
    VALUES ((SELECT seq FROM episode_search_ids WHERE episode_id = new.id),
            new.id, new.podcast_id, new.title, new.subtitle,
            substr(new.description_text, 1, 4000));
END;

-- Removal hangs off the *map*, not off the episode. Deleting a podcast
-- cascades to its episodes and, from there, to this map - and the cascade
-- runs before the episode's own AFTER DELETE trigger body, so a trigger
-- that looked the rowid up in the map would find it already gone and
-- delete nothing. Keying the cleanup to `old.seq` makes both paths (the
-- cascade, and the explicit delete below) converge on the same rowid.
CREATE TRIGGER episode_search_ids_ad AFTER DELETE ON episode_search_ids BEGIN
    DELETE FROM episodes_fts WHERE rowid = old.seq;
END;

CREATE TRIGGER episodes_fts_ad AFTER DELETE ON episodes BEGIN
    DELETE FROM episode_search_ids WHERE episode_id = old.id;
END;

CREATE TRIGGER podcasts_fts_ai AFTER INSERT ON podcasts BEGIN
    DELETE FROM podcasts_fts WHERE podcast_id = new.id;
    INSERT INTO podcasts_fts (podcast_id, title, author, description_text)
    VALUES (new.id, new.title, new.author, substr(new.description_text, 1, 4000));
END;

CREATE TRIGGER podcasts_fts_au AFTER UPDATE OF title, author, description_text ON podcasts
WHEN old.title IS NOT new.title
  OR old.author IS NOT new.author
  OR old.description_text IS NOT new.description_text
BEGIN
    DELETE FROM podcasts_fts WHERE podcast_id = new.id;
    INSERT INTO podcasts_fts (podcast_id, title, author, description_text)
    VALUES (new.id, new.title, new.author, substr(new.description_text, 1, 4000));
END;

CREATE TRIGGER podcasts_fts_ad AFTER DELETE ON podcasts BEGIN
    DELETE FROM podcasts_fts WHERE podcast_id = old.id;
END;
