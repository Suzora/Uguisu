# ADR 0050 — Podgrab migration: its real names, its database and the tags of foreign files

**Status:** accepted, amends [ADR 0025](0025-rebuild-and-import.md), amended by [ADR 0059](0059-archive-import-in-the-web-ui.md) · **Date:** 2026-10-03

## Context

ADR 0025 built the import of a foreign archive as a structural reading of a directory tree, with a Podgrab adapter that "depends on no version, database or configuration file". Its fixtures used names like `2024-01-05 - 013 - Der Einbruch.mp3`. Podgrab does not write those. Read from its source (akhilrex/podgrab, `service/fileService.go`, `service/podcastService.go`, `db/`), it stores an episode as

```
$DATA/<sanitize(podcast title)>/[<n>-][<YYYY-MM-DD>-]<kebab(sanitize(episode title))>.<ext>
```

where `sanitize` lowercases, transliterates accents, turns `[ &_=+:]` into `-`, drops everything but ASCII letters, digits, `-` and `.`, and keeps only what follows the last `/`; `<n>` is Podgrab's own counter, not `itunes:episode`, and comes before the date. Both prefixes are off by default. So `13-2024-01-05-der-einbruch.mp3` yielded neither the date nor a usable title, and a real Podgrab directory imported almost nothing.

Podgrab also keeps `$CONFIG/podgrab.db`, a SQLite database with, for every downloaded episode, its GUID, enclosure URL, publication date, title and the path it wrote, and for every podcast its feed URL. The file names lose most of that; the database has it exactly.

Files from any tool carry their publisher's tags, which nothing read (the open item "reading the embedded tags of foreign files"). And reading the import code for this change turned up defects that a migration would hit: an import could rewrite the record of an episode Uguisu had already archived, two files could claim one episode, and two episodes one path.

## Decision

### Podgrab's own names

The adapter strips Podgrab's `<n>-<YYYY-MM-DD>-` prefix and drops the counter. A lone leading `<n>-` stays in the title, because `123: Intro` also becomes `123-intro` and numbered titles are common; the reading without it is offered as a **fallback title**. The podcast hint is the file's parent directory, not the first component of its path.

### Fallback titles, tried in order

A candidate has a primary title and fallback titles. The matcher classifies with the primary title and, only when that is not a clear match, with each fallback in turn; the first clear match wins. Anything that matched before matches the same way. Taking the best of all titles instead was rejected: a stale tag could then turn today's match into an ambiguity.

### The tags of foreign files

Every media file is read through `lofty`, without cover art and with its audio properties. Three things are used:

- the **title**, as a fallback title;
- the **duration of the audio**, where nothing else gave one;
- an embedded **episode GUID** that exactly one episode of the podcast carries, as an exact match (`embedded_guid`) — unless scoring without it clearly names a *different* episode, which makes the file ambiguous.

Dates and track numbers from tags are not used: recording dates, bare years and stale numbers would contradict the feed and block matches the title makes today. A file whose tags cannot be read is matched as before.

### Podgrab's database, when given

`--podgrab-db <file>` opens the database read-only and immutable, with `query_only` on and `trusted_schema` off, and refuses when a `-journal` or `-wal` lies beside it: a running Podgrab must be stopped first. Nothing is written next to it.

A downloaded row is tied to a scanned file by the last two components of its stored path and of the file's path, so the import root may be the directory Podgrab wrote to, its parent, or one podcast folder. A path that several rows share — Podgrab does not download a file again when one is already there — ties to no row, and the file is scored.

For a file with a row, the podcast is the one whose feed URL (current or former, compared by `feed_key`) is the row's, else the one titled exactly as Podgrab titled it, else the directory gate as before. The episode is the one with the row's GUID, else the one with its enclosure URL, each only when unique: an exact match (`source_database`). Otherwise the file is scored with the row's title and date instead of its name. `--podgrab-db` implies the Podgrab layout.

### An archived episode is never registered again

When the matched episode already has an archive record, an import never touches it. A file whose size and hash equal the record's current or as-received bytes is `already_present`; any other file is a `conflict` naming the archived path. Restoring an archived file that has gone missing is not an import.

### One file per episode, one episode per path

Several files matching one episode: identical bytes import once and the others are `already_present`; different bytes are all `conflict`, because choosing would be a guess. A target planned for one file counts as taken for every later one. Applying adopts a file found at the target only when its hash is the source's, and refuses when a record appeared since the plan.

### What an import reports

Every scanned file has a line in the plan; the hundred-path cap of ADR 0025 applies to rebuild and manifest findings, not to an import, whose plan is its output. Episodes Podgrab kept after the feed dropped them have no episode in Uguisu and are reported, never created.

## Consequences

- A Podgrab directory imports with its names as Podgrab wrote them; with the database, every downloaded episode still in the feed is identified exactly, including those whose title Podgrab's names destroyed.
- An episode Uguisu has archived keeps its record whatever an import finds.
- An import reads more of each file than its first bytes: the tags and the audio properties, bounded by `lofty`'s own limits. SECURITY.md says so.
- The Podgrab adapter now knows Podgrab's naming and schema. Podgrab is unmaintained, so neither is expected to move; the schema assumptions live in one function.
- Duration from the audio can disagree with a feed that inserts ads dynamically or declares a wrong `itunes:duration`; it is one signal among five.

## Alternatives considered

- **The best of all titles.** Rejected: see above.
- **Dates and track numbers from tags.** Rejected: too often a recording date, a year or a number from another numbering.
- **First file wins** when several match one episode. Rejected: it is a guess, which ADR 0025 rules out.
- **Creating episodes from Podgrab rows** the feed no longer lists. Deferred: it invents library content from another tool's record and needs its own decision.
- **Reading the database while Podgrab runs.** Rejected: an immutable open ignores a live journal, and a non-immutable one would write lock files into Podgrab's directory.
