# The service

Phase 7 turns Uguisu from a toolbox into an archiver: `uguisu serve` runs, feeds get refreshed, and the archive policy that Uguisu built finally has something to fire it. This document describes the daemon, the scheduler, the settings layer and local search as they are built.

Decisions: [ADR 0027](DECISIONS/0027-service-and-refresh-scheduler.md) (service and scheduler), [ADR 0028](DECISIONS/0028-persisted-settings-and-precedence.md) (settings), [ADR 0029](DECISIONS/0029-local-search-index.md) (search), [ADR 0030](DECISIONS/0030-discovery-persistence.md) (discovery persistence).

## 1. What `serve` starts

```
uguisu serve [--bind 127.0.0.1:8484]
```

It starts four things:

| | |
|---|---|
| the HTTP API | `/api/v1/...`, including the web UI's status view |
| the download workers | the queue, resumable and crash-safe |
| the feed-refresh scheduler | this document |
| the search-index build | in the background, if the index is not ready |

The desktop shell's embedded server starts the same four ([`DESKTOP.md`](DESKTOP.md)), and `download run` or `download --wait` runs the download workers in the foreground until it is done. Every other command does what it was asked and exits. Nothing long-lived hides in `podcast add`, in a refresh, in a download or in an HTTP request.

**One process owns a data directory.** A second `uguisu` on the same directory fails, naming the holder's pid from `uguisu.pid`, and a one-shot command run while `serve` holds the lock says so and suggests `--server http://127.0.0.1:8484`.

**Stopping.** SIGTERM, Ctrl-C or, on Windows, Ctrl-Break cancels one token. The HTTP server stops accepting connections and gives open requests 2 s; an open web UI's event stream is one, and it ends with the engine rather than before it. The queue parks running jobs as `queued(shutdown)`, the scheduler stops starting refreshes and the ones it has abort their fetch and are waited for as they unwind (a pool closed under an open transaction turns a clean cancellation into a storage error), the index build stops where it is, manifests are flushed, the pools close. Everything is bounded by `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS` (15 s). What did not finish is picked up by the next start: a parked job is queued, a half-built index is rebuilt, an unfinished refresh simply happens again.

## 2. The refresh scheduler

### What a pass does

1. Stop if the process is shutting down.
2. Skip the refresh half if `UGUISU_FEED_SCHEDULER` is off (housekeeping still runs).
3. Read the persisted pause. A storage error counts as paused.
4. Work out the free capacity: `UGUISU_FEED_REFRESH_CONCURRENCY` minus what this loop is already refreshing.
5. Ask the database for that many podcasts that are due *and not already being refreshed*, most overdue first, never-fetched before overdue.
6. Start each one through the ordinary refresh path.
7. Run housekeeping if it is due.
8. Sleep until the earliest of: the next planned refresh, the next spacing floor to expire, the next housekeeping pass — clamped to between 1 second and 15 minutes. A freed slot or a changed due time wakes it early.

An idle process wakes at most every fifteen minutes and asks three indexed questions.

### When a podcast is next due

A refresh writes `next_refresh_at` when it ends — the scheduler never rewrites due times. The interval is the podcast's own (`refresh_interval_secs`), or `UGUISU_FEED_REFRESH_INTERVAL_SECS` (1 h), and never shorter than the origin's `Cache-Control: max-age`. After a failure it is the plain interval, doubling with each further consecutive failure up to 24 hours.

On top of that, each podcast gets **its own share of the interval**, derived from its identifier:

| Situation | Added |
|---|---|
| ordinary | 0–10 % of the interval |
| overdue by more than one interval, or never fetched | 0–100 % |

The share never changes and never depends on the clock, so the same library produces the same schedule on every machine. It is always added, never subtracted: fetching *earlier* than the interval says would undo the `Cache-Control` the previous paragraph honours.

This is what makes a restart after a week of downtime a queue rather than a stampede: the drain is capped by the concurrency, and every drained podcast is replanned across a whole interval, so the library spreads itself out after one pass instead of re-synchronising.

### Pausing

| | |
|---|---|
| `uguisu scheduler pause [--reason "…"]` | stops automatic refreshing, across restarts |
| `uguisu scheduler resume` | starts it again |
| `uguisu podcast pause <id>` | takes one show out of the schedule |
| `uguisu podcast resume <id>` | puts it back, due inside the next interval |
| `uguisu podcast archive <id>` | stops fetching it for good, until `resume` ([ADR 0055](DECISIONS/0055-archiving-and-removing-a-podcast.md)) |
| `uguisu podcast schedule <id> [--at RFC3339]` | sets when one show is next due |
| `uguisu scheduler run` | one pass now, without waiting for the loop |

`scheduler run` starts the refreshes a pass found and then waits for them, bounded by `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS`, before it closes the database. It has to: the pass returns as soon as the work is *started*, and a process that exited there would cancel every fetch it had just asked for.

Pausing the scheduler is **not** pausing downloads: a full disk pauses transfers by itself, and an operator who stops transfers is not asking the library to go stale. A manual `podcast refresh` works on a paused podcast and leaves it paused.

### Automatic downloads

The scheduler refreshes feeds. Whether a discovered episode is downloaded is the archive policy's decision, unchanged: `UGUISU_ARCHIVE_AUTO_DOWNLOAD` is still `false` by default, enabling a policy still backfills nothing, and a failed refresh enqueues nothing. What Phase 7 changes is that refreshes now happen without being asked, so a policy that *is* enabled finally fires.

## 3. Housekeeping

Once a day (`UGUISU_MAINTENANCE_INTERVAL_SECS`), in the scheduler's own task:

- prune the event log to `UGUISU_EVENTS_RETAIN_DAYS` (30) and `UGUISU_EVENTS_RETAIN_MAX_ROWS` (100 000);
- delete discovery-cache rows whose time has passed;
- delete sessions that can no longer authenticate — revoked, or past either deadline — and report `sessions_pruned`. A revoked **token** is kept, so a listing can still say it was revoked ([ADR 0035](DECISIONS/0035-credentials-sessions-and-tokens.md)).

Housekeeping never verifies or reconciles the archive. Every start registers what a crash left unrecorded, and a server confirms in the background that recorded files exist ([`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §8); `archive verify` (light, or `--full`) and `archive reconcile --deep` run when asked. Both are expensive, and neither has a failure mode that waiting makes worse.

Both event limits treat `0` as **no limit**, never as "delete everything". Only derived data is touched: no media, no episode, no podcast, and not the episode change log, which is kept (ADR 0015). `last_maintenance_at` is recorded in the same transaction, so a restart works out when the next pass is due instead of running one on every boot. `uguisu scheduler maintenance` runs it now.

## 4. Settings

### What wins

```
defaults  <  stored settings  <  environment  <  command-line flag
```

An environment variable beats a stored setting on purpose. Somebody who writes `UGUISU_FEED_REFRESH_CONCURRENCY=4` into a unit file has to be able to rely on it, so a key that is set in the environment is **pinned**: `config set` refuses it (409 over HTTP) rather than storing a value that would be ignored. The remedy is to unset the variable; there is no `--force`.

There is no `config.toml` in v1, and no file is read, written or half-wired: an environment file (`EnvironmentFile=`, `docker --env-file`) is the file-based configuration (ADR 0028).

### Using them

```
uguisu config list                       # every key, its value and where it came from
uguisu config get UGUISU_FEED_SCHEDULER
uguisu config set UGUISU_FEED_REFRESH_CONCURRENCY 4
uguisu config unset UGUISU_FEED_REFRESH_CONCURRENCY
uguisu config validate                   # exits non-zero if a stored value is being ignored
```

A stored value is written in exactly the syntax the environment variable uses, and validated the same way — including the rules that involve more than one key, like `per_host ≤ global`.

### Keys that change a running daemon

The feed, archive and housekeeping keys are read per operation, so storing one takes effect at once. The HTTP clients, the discovery stack, the download queue and the archive path template and profile capture their configuration when the process starts; those keys are stored and reported as `restart_required` rather than pretending. `config list` says which is which.

### Keys that may never be stored

Six keys, listed with the reason in [`CONFIGURATION.md`](CONFIGURATION.md): the secrets and where they are sent, the two directories and the SSRF allowlist, which one compromised session must not be able to widen (`SECURITY.md` §3.1).

### When a stored value stops parsing

It is **quarantined, not deleted**. The row stays exactly as written, the engine starts without it, a warning names it and a `settings.rejected` event records it. `config validate` and `GET /api/v1/settings` show it with the parser's message. Only `config set` (with something valid) or `config unset` clears it.

The reasoning is in ADR 0028: refusing to start would brick the daemon whose API is the only way to fix the row, and deleting it would throw away what the user meant at the moment they need to correct it.

## 5. Local search

```
uguisu search library "async runtimes" [--limit 25] [--prefix] [--explain]
uguisu search reindex
```

Searches the library — podcasts and episodes already added — not the directories. `uguisu search podcast` is still the one that searches the world.

**Operators are not special.** Every word you type is a word: `OR`, `NEAR`, `*`, `"`, `:`, `-` and brackets match themselves, and no input can produce a syntax error. Matching covers titles, subtitles and the first ~4 000 characters of the shownotes, folded for case and diacritics by SQLite's tokenizer.

Results are ranked by text relevance with the title weighted heavily, adjusted for an exact title match, how recent the episode is and whether it is already archived. `--explain` prints the arithmetic.

**The answer always says what happened**: `ok`, `no_results`, `empty_query`, `index_building` (with how far it has got) or `index_stale`. A search never returns a silence that has to be guessed at. Nothing found exits 3, whichever of those it was.

The index is kept current by the database itself, so anything a refresh writes is searchable at once. It has to be *built* once for a library that predates it: `serve` does that in the background, and `uguisu search reindex` does it now. Interrupting a build loses nothing — the index is derived, and the next start rebuilds it.

## 6. Status

`GET /api/v1/status` answers in one request: the library's size, the scheduler, the download queue, the search index, when housekeeping last ran and whether any stored setting is being kept but not used. `uguisu scheduler status` is the same picture for the scheduler on the command line. The web UI's **Service** view renders it, re-read on every `scheduler.*` and `search.*` event and after each of its controls, which are the API's: pause and resume the scheduler, run a pass, run housekeeping, refresh every podcast, pause and resume all downloads, rebuild the index. Stored settings are changed in its **Settings** view ([`WEB_UI.md`](WEB_UI.md)).

## 7. Running it

Uguisu is one binary and one data directory, plus the built web UI ([`DEPLOYMENT.md`](DEPLOYMENT.md), "Installing the server"). A unit file is enough:

```ini
[Unit]
Description=Uguisu
After=network-online.target

[Service]
ExecStart=/usr/local/bin/uguisu serve --bind 127.0.0.1:8484
Environment=UGUISU_DATA_DIR=/var/lib/uguisu
Environment=UGUISU_MEDIA_DIR=/srv/podcasts
Environment=UGUISU_WEB_DIR=/usr/local/share/uguisu/web
Restart=on-failure
# SIGTERM is handled: jobs are parked and the database is closed cleanly.
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
```

`TimeoutStopSec` should exceed `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS`, or the supervisor will kill a shutdown that was about to finish. A SIGKILL is survivable — that is what the recovery and the archive reconciliation at start are for — but it costs work in flight.

In a container the same rules hold: one process per data directory, the data and media directories on volumes, and SIGTERM forwarded to the binary. The image and how to run it are in [`DOCKER.md`](../DOCKER.md) (ADR 0061); no image is published to a registry.

**Binding.** The default is loopback, and a bind that is not loopback **with no password set refuses to start** (exit 12, [ADR 0037](DECISIONS/0037-exposure-gate.md)). Set one with `uguisu auth set-password`, and put a TLS-terminating reverse proxy in front for anything beyond a trusted network — Uguisu does not terminate TLS, and reads `X-Forwarded-For` only from a proxy named with `--trusted-proxy` (ADR 0062). [`DEPLOYMENT.md`](DEPLOYMENT.md) has the configurations.
