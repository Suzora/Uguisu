# ADR 0029 — Local search over the library

**Status:** accepted · **Date:** 2026-09-21 · relates to [ADR 0002](0002-sqlite-storage.md), [ADR 0020](0020-id-based-media-paths.md), [ADR 0025](0025-rebuild-and-import.md)

## Context

A library of a hundred podcasts and forty thousand episodes has no way to answer "which episode was that". Discovery searches the world; nothing searched what the user already has.

## Decision

### FTS5, content-owning, with a stable rowid map

SQLite ships FTS5 and `libsqlite3-sys` compiles it in, so there is nothing to add. The index tables own their text.

**External content (`content='episodes'`) is rejected twice over.** Uguisu's primary keys are TEXT ULIDs, so the base tables have an *implicit* rowid that `VACUUM` may renumber — and `ARCHITECTURE.md` plans a vacuum. An external-content index keyed on that rowid would afterwards point at the wrong rows, silently. It also needs the pre-image of an updated row, which `INSERT … ON CONFLICT DO UPDATE` does not have without a second read per row inside a five-hundred-row refresh batch. Contentless is rejected because it still needs the rowid map *and* loses `snippet()`.

`episode_search_ids(seq INTEGER PRIMARY KEY AUTOINCREMENT, episode_id TEXT UNIQUE)` gives every episode a stable integer to use as the FTS rowid. Without it, `DELETE … WHERE episode_id = ?` would scan the whole index once per episode inside every refresh batch, because an `UNINDEXED` column has no index. `AUTOINCREMENT` because a reused rowid would resurrect a deleted episode's text. Podcast text changes are rare and the table is bounded by the size of the library, so `podcasts_fts` gets no map.

Only the first 4 000 characters of a description are indexed — a bound on an input the publisher chooses, not a guess — and the documentation says so plainly.

### The database keeps the index current, not the repositories

Six triggers. Five writers already touch these tables and more arrive later; a trigger is a rule that also holds for an import, for a migration and for a person with the `sqlite3` CLI. `UPDATE OF title, subtitle, description_text … WHEN old IS NOT new` keeps it cheap: a refresh rewrites every podcast column on every run but the `WHEN` finds the text unchanged, and the narrow episode updates never name a text column.

Two things the implementation taught, both recorded because they are invisible until they bite:

- **`INSERT OR IGNORE` inside a trigger body does not do what it says.** SQLite discards the body's conflict policy and applies the *outer* statement's, which for the episode upsert is `ON CONFLICT DO UPDATE` — so an `OR IGNORE` raised on every second refresh. The id-map insert is guarded with `WHERE NOT EXISTS` instead.
- **A foreign-key cascade runs before the child's own `AFTER DELETE` body.** Deleting a podcast therefore left its episodes in the index: the map row was already gone when the episode's trigger looked for it. Cleanup hangs off `AFTER DELETE ON episode_search_ids`, keyed on `old.seq`, so the cascade and the explicit delete converge on the same rowid. `PRAGMA recursive_triggers` joins `foreign_keys` in `Storage::open_path`, or the cascade fires no triggers at all.

### `archive reconcile --rebuild` writes no index rows, and needs no exception

Verified rather than assumed: rebuild *reads* podcasts and episodes and *writes* `archive_files` rows and events. It creates no podcast or episode primary data — that is ADR 0025's rule, not an accident — and the archive path's only episode write touches `archive_state`, `skip_reason` and `updated_at`, none of them a column the `UPDATE OF` clauses name.

So a rebuild leaves the search index untouched **because it writes no searchable text**, not because a trigger looks the other way. Two consequences worth stating: rebuilding the archive index changes nothing about search, and after a total database loss the episodes come back through an ordinary feed refresh, which fires the triggers normally — search recovers with the library, not with the archive. A test asserts the index is byte-identical across a `--rebuild --apply`; should a later phase make rebuild write episode rows, the triggers would index them correctly and that test is what would flag the change.

### The migration creates the index but does not fill it

Migrations run inside `Engine::open`, in one transaction. Building a hundred thousand rows there would be tens of seconds that an interrupted start rolls back and repeats for ever. `search_index_state` starts `stale`; `serve` builds it in the background in batches of a thousand, one transaction each, so the single writer stays available. An interrupted build leaves a row that says `building` with the counts it reached, and the next start builds again — nothing is lost, because the index is derived.

A search during a build answers with what the index has *and* says it is incomplete. Silently returning fewer results is the one failure mode a search must not have.

### No user input reaches the FTS5 parser

`parse_query` splits on everything that is not alphanumeric — which removes `*`, `"`, `(`, `)`, `:`, `^` and `-` by construction, and is also how `unicode61` split the indexed text — bounds the result to sixteen terms of sixty-four characters, and re-emits them as quoted phrases, where `OR` is the word "or" and `-` is a hyphen. Prefix mode stars the last term only, and only at two characters or more. Empty or punctuation-only input yields no query at all: no SQL runs, and the outcome says `empty_query`.

**The claim is about syntax.** Whatever a user types, the generated `MATCH` expression parses. A corrupt index, a closed pool or a disk error remains an ordinary query failure and surfaces as one: search is not exempt from the failure modes every other read has.

Case and diacritics are deliberately left alone. FTS5 applies the table's tokenizer to the query as well, and folding them here a second, slightly different way is how a search stops finding what it indexed.

### Ranking is `bm25` plus three named signals

Title weighted ten times the shownotes, then exact-title, recency (halving every thirty days) and archived-state as bounded adjustments, in the shape `uguisu-discovery` already uses: a name, a weight, a value, a contribution, so `--explain` shows the arithmetic. The text signal is squashed into 0…1 on purpose — a title match or a recent episode should break a near-tie, not overrule the text. A missing publication date scores zero recency rather than a penalty, because plenty of feeds have none.

Snippets let FTS5 choose the column: naming `description_text` hands back the first words of the shownotes for a title match, which is text that has nothing to do with the search.

## Consequences

Every write to a podcast's or an episode's text costs an index write, bounded by the `WHEN` clauses. Disk grows by an excerpt per episode.

`uguisu search library` answers with an outcome, never a silence, and exits 3 when it found nothing — whatever the reason, and saying which.

## Alternatives considered

**`LIKE '%term%'`.** No index, no ranking, no snippets, and a full scan per query.

**An external search engine.** A second process for a single-binary program that fits in a file.

**Indexing the whole description.** Unbounded by anything Uguisu controls.
