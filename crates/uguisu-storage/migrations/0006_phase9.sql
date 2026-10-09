-- Phase 9: the credential, the sessions it opens and the tokens it issues
-- (docs/SECURITY.md §3.5, ADR 0035/0036), plus two indexes the API's new
-- pages ride.
--
-- Conventions as in 0001-0005: ULID TEXT ids, RFC 3339 TEXT timestamps,
-- booleans as INTEGER 0/1.
--
-- No personal data anywhere below: no address, no user agent. Nothing would
-- read them, and they would be the only such data in the database.

-- ---------------------------------------------------------------- credential

-- One row, like `scheduler_control` (0005). Deliberately *not* a `users`
-- table with a role column, which ADR 0013 charted and this migration
-- corrects: Aviary has one operator and no role distinguishes anything, so
-- the table that exists is the one the code reads. A second identity, if it
-- ever arrives, is a forward migration like every other change here.
--
-- `password_hash` is an argon2id PHC string (`$argon2id$v=19$m=...$...`), so
-- the parameters travel with the hash and can be raised later without a
-- migration: an old hash still verifies with the values it was made under.
CREATE TABLE auth_credential (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    username      TEXT NOT NULL CHECK (length(username) BETWEEN 1 AND 64),
    password_hash TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- ------------------------------------------------------------------ sessions

-- `id` is the row's ULID and may appear in a log. The cookie the browser
-- holds is a *separate* 256-bit secret that exists here only as a SHA-256
-- digest, so a stolen database cannot be replayed as a session.
--
-- `csrf_token` is stored in the clear on purpose: it is useless to anyone
-- without the cookie, and hashing it would cost a digest on every mutation
-- while protecting nothing the cookie does not already protect.
--
-- Both deadlines are stored rather than derived. The absolute one is fixed
-- when the session opens; the idle one moves, but at most once an hour,
-- because the writer pool has a single connection and a write per request
-- would put the whole API behind this table.
CREATE TABLE auth_sessions (
    id                  TEXT PRIMARY KEY,
    token_digest        TEXT NOT NULL UNIQUE,
    csrf_token          TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    last_seen_at        TEXT NOT NULL,
    idle_expires_at     TEXT NOT NULL,
    absolute_expires_at TEXT NOT NULL,
    revoked_at          TEXT
);

-- Housekeeping asks "what has expired", never "whose session is this". The
-- lookup by digest rides the UNIQUE index above.
CREATE INDEX idx_auth_sessions_idle ON auth_sessions (idle_expires_at);

-- -------------------------------------------------------------------- tokens

-- A token is shown once, when it is created, and never again: of the secret
-- only the digest survives, and it is looked up by that digest rather than by
-- any name the caller controls.
--
-- A revoked token's row stays, so `aviary auth token list` can say that the
-- token somebody wrote down no longer works - which is the answer they need.
-- Expired *sessions* are pruned; expired tokens are not, because there are
-- few of them and each one is a decision the operator made.
CREATE TABLE auth_tokens (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    token_digest TEXT NOT NULL UNIQUE,
    scope        TEXT NOT NULL CHECK (scope IN ('read', 'write')),
    created_at   TEXT NOT NULL,
    last_used_at TEXT,
    expires_at   TEXT,
    revoked_at   TEXT
);

-- ------------------------------------------------------------- page supports

-- `GET /api/v1/podcasts` keysets on (sort_title, id). `idx_podcasts_sort_title`
-- (0001) orders by the first column but cannot resolve the tie-break without
-- visiting the row, which is what makes a cursor page cost a scan.
CREATE INDEX idx_podcasts_page ON podcasts (sort_title, id);

-- `feed_fetches` had no index on `source_id` at all - 0001 indexes
-- (podcast_id, fetched_at) - so "the newest fetch of this source" scanned the
-- whole fetch log, once per podcast, on every podcast-list request. Keyed on
-- the source rather than the podcast because after a feed-URL migration the
-- newest fetch for a podcast can belong to the source it moved away from.
CREATE INDEX idx_fetches_source ON feed_fetches (source_id, fetched_at DESC);
