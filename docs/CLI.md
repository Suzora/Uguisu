# Command-line interface

The `uguisu` binary is both the server (`uguisu serve`) and the command-line client. This document covers every command.

## Global options

| Option | Env | Meaning |
|---|---|---|
| `--json` | | Machine-readable JSON on stdout (stable, versioned by `schema`). A command that produced a result prints it there whatever its exit code: a failed refresh or download job, a search without results, a failed `resolve` or `search --resolve`, findings from `db check` or `archive verify|orphans|manifest verify`. An error that stopped a command before it had a result is `{ error, command, message, phase }` on stderr, and `podcast add` reports a failed resolution this way. An argument error stays text. Progress lines are not written |
| `--server <url>` | `UGUISU_SERVER` | Talk to a running server's API instead of running embedded. Discovery and library commands run embedded when this is not set (ADR 0003, amended). |
| `--token <token>` | `UGUISU_TOKEN` | API token for server mode (`uguisu auth token create`). Sent as `Authorization: Bearer`; a value a header cannot carry is refused (exit 1), not sent as a silently anonymous request. |
| `--data-dir <dir>` | `UGUISU_DATA_DIR` | Directory holding `uguisu.db`, `uguisu.lock` and `uguisu.pid`; defaults to the platform data directory (`~/.local/share/uguisu`, `~/Library/Application Support/uguisu`, `%APPDATA%\uguisu\data`). |
| `--log <filter>` | `UGUISU_LOG` | `tracing` filter, default `warn` (`info`, `uguisu_discovery=debug`, …) |
| `--log-format <format>` | `UGUISU_LOG_FORMAT` | Format of the log events on stderr: `pretty` (default) for a person, or `json`, one JSON object per event. A command's results, errors and progress are not log events; `--json` makes results and errors machine-readable and drops progress lines |

Embedded stateful commands (`podcast add|list|show|refresh|import|export|move-feed|pause|resume|schedule|archive|remove`, `feed refresh|status`, `episode list|show|duplicates|resolve|download`, `db …`, `download …`, `archive …`, `auth …`, `scheduler …`, `config list|get|set|unset|validate`, `search library|reindex`, `serve`) take the data directory's lock for the duration of the command. While `uguisu serve` runs, they exit with code 1 and point to `--server`; `feed inspect`, `search` and `resolve` need neither the directory nor the lock.

Configuration comes from environment variables (see `docs/CONFIGURATION.md` for the full list); the most important ones:

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_APPLE_COUNTRY` | `US` | Apple storefront |
| `UGUISU_PODCASTINDEX_KEY` / `UGUISU_PODCASTINDEX_SECRET` | unset | Enables the Podcast Index provider with your own key |
| `UGUISU_DISCOVERY_GPODDERNET_ENABLED` | `false` | Opt into gpodder.net |
| `UGUISU_DISCOVERY_SOFT_DEADLINE_MS` / `UGUISU_DISCOVERY_HARD_DEADLINE_MS` | `2000` / `8000` | First-results and overall search deadlines |
| `UGUISU_HTTP_ALLOW_PRIVATE_HOSTS` | unset | Comma-separated hosts allowed to resolve to private addresses |
| `UGUISU_DATA_DIR` | platform data dir | Database and lock file |
| `UGUISU_FEED_MAX_BYTES` / `UGUISU_FEED_MAX_ITEMS` | `52428800` / `50000` | Feed body and item caps (`docs/FEED_ENGINE.md`) |
| `UGUISU_FEED_REFRESH_TIMEOUT_MS` | `60000` | Budget for one whole refresh (fetch, parse, sync) |
| `UGUISU_FEED_REFRESH_CONCURRENCY` | `8` | Parallel fetches for `podcast refresh --all`, and feeds resolving at once for `podcast import --apply` |
| `UGUISU_FEED_REMOVAL_STREAK` | `2` | Complete fetches without an episode before it counts as removed |
| `UGUISU_FEED_RETAIN_FETCHES` | `50` | Fetch-log rows kept per podcast |
| `UGUISU_MEDIA_DIR` | `<data dir>/media` | Where downloaded files land, on the path `UGUISU_ARCHIVE_TEMPLATE` renders |
| `UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY` / `UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY` | `3` / `1` | Parallel downloads overall and per host |
| `UGUISU_DOWNLOAD_MAX_ATTEMPTS` | `8` | Attempts per job before it fails |
| `UGUISU_DOWNLOAD_BACKOFF_BASE_MS` / `UGUISU_DOWNLOAD_BACKOFF_MAX_MS` | `30000` / `21600000` | Retry backoff (exponential with jitter, `Retry-After` honoured) |
| `UGUISU_DOWNLOAD_IDLE_TIMEOUT_MS` | `60000` | A body that sends nothing for this long counts as a timeout |
| `UGUISU_DOWNLOAD_MAX_BYTES` / `UGUISU_DOWNLOAD_MIN_FREE_BYTES` | `8 GiB` / `256 MiB` | Largest file accepted; free space that must stay on the media disk |
| `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS` | `15000` | How long `serve`/`download run` wait for running jobs to park on shutdown |

## `uguisu search podcast <query>`

Searches every enabled directory, deduplicates and ranks the results.

```text
uguisu search podcast "darknet diariez"
uguisu search podcast "Jack Rhysider" --explain
uguisu search podcast "gemischtes hack" --country DE --limit 5
uguisu search podcast "darknet" --provider apple,podcastindex --no-cache
uguisu search podcast "darknet diaries" --resolve        # also verify the top result's feed
uguisu search podcast "https://example.com/feed.xml"     # a URL is resolved, not searched
```

Options: `--provider <a,b>` restrict providers · `--limit <n>` · `--country <CC>` · `--explain` show ranking signals · `--resolve` verify the top result's feed · `--no-cache` bypass the discovery cache.

Text output:

```text
1. Darknet Diaries
   Jack Rhysider
   Feed: https://feeds.megaphone.fm/darknetdiaries
   Source: podcastindex, apple
   Confidence: 0.87
```

When the query as typed found nothing and a looser one did ([`DISCOVERY.md`](DISCOVERY.md) §7), one line comes first:

```text
No results for `darknet diariez`; showing results for `darknet`.
```

With `--json` the output is a `SearchResponse` (`schema: 1`): `outcome`, `query`, `results[]` (each with `rank`, `score`, `confidence`, `candidate`, `explanation.signals[]`, `ambiguities[]`), `providers[]` (status, latency, cache), `relaxed[]` (each looser query asked, with its own `providers[]`), `attribution[]`, `timing`.

## `uguisu resolve <input>`

Resolves a feed URL, a podcast website or a directory page (Apple Podcasts, Podcast Index) to a verified feed and prints its identity, the steps taken and any warnings. Spotify and YouTube links are reported as having no RSS feed.

```text
uguisu resolve https://darknetdiaries.com/
uguisu resolve https://podcasts.apple.com/us/podcast/darknet-diaries/id1296350485 --json
```

JSON output is a `ResolvedFeed` (feed URL, canonical URL, `moved_to`, title, author, artwork, `podcast_guid`, item counts, newest item, `provenance[]`, `warnings[]`) or, on failure, `{ "error": { "kind": …, … }, "suggestion": …, "provenance": [...] }`.

## `uguisu podcast add <input> [--yes]`

Resolves the input (feed URL, website or directory page), stores the podcast and runs its first refresh. A feed URL is added directly. A website or directory page is resolved first; without `--yes` (or `--json`) the resolved feed is printed and the command stops with exit code 2 so the user can confirm. Adding the same feed twice is a no-op that reports the existing podcast; a second feed carrying the `podcast:guid` of a podcast already in the library is refused (exit 1) because that is a migration, not a new show.

```text
uguisu podcast add https://rss.buzzsprout.com/1.rss
uguisu podcast add https://darknetdiaries.com/ --yes
uguisu podcast add https://example.com/feed.xml --json
```

Text output names the podcast and its id, then the first refresh's report (below). JSON: `{ schema, podcast, source, created, resolved, report }`.

## `uguisu podcast import <file> [--apply] [--mode auto|manual …]`

Reads an OPML file (`-` for standard input) and plans what importing it would do; `--apply` does it (ADR 0049). Every outline with an `xmlUrl` counts, at any depth; folders are ignored. The file is read as UTF-8, at most 1 MiB. With `--server` the CLI reads the file and sends its text, so the file is on the machine running the command, not on the server.

The plan sends no request. Each feed is `add`, `already_present` (a podcast has it, now or as a former source), `duplicate` (earlier in the file) or `invalid` (not an `http`/`https` URL); scheme, `www.`, a trailing slash and tracking parameters do not make a different feed. `--apply` resolves each new feed like `podcast add`, at most `UGUISU_FEED_REFRESH_CONCURRENCY` at a time, and adds it only when the URL served the feed itself: a website or directory page is `needs_review`, with the feed it found, for `podcast add`. A second feed of a show already in the library is a `conflict`; anything else that goes wrong is `failed` with the reason, and stops nothing else.

`--mode` stores that policy with each podcast the import adds, with `--max-backlog`, `--max-age-days` and `--priority` as in `archive policy set`; without it the global defaults apply. Podcasts already in the library keep theirs. An import does not refresh: the scheduler fetches new podcasts first, or `podcast refresh --all` does it now. Each podcast is added on its own, so an import cut short is continued by running it again.

```text
uguisu podcast import subscriptions.opml
uguisu podcast import subscriptions.opml --apply --mode auto --max-backlog 3
curl -s https://old.example/export.opml | uguisu podcast import - --apply
```

```text
Added 41 of 44 feeds
  already present: 1
  invalid: 1
  needs review: 1
  invalid: feed://example.com/rss (feed: is not http or https)
  needs_review: https://example.com/ (this is a page, not a feed; it points to https://example.com/feed.xml)
Their episodes arrive with the next scheduled refresh; `uguisu podcast refresh --all` fetches them now.
```

A dry run starts with `Would add … (dry run)` and ends with `Run again with --apply to add them.`; `needs_review`, `conflict` and `failed` only appear once feeds are resolved.

Exit code 0 for the plan and for `--apply`, whatever became of single feeds; 2 when the file is not OPML, larger than 1 MiB, or a flag is wrong; 1 when the file cannot be read. JSON: `{ schema, applied, counts: { add, already_present, duplicate, invalid, needs_review, conflict, failed }, items: [{ title, xml_url, action, podcast_id, detail }] }`. Over `--server` the CLI waits up to an hour for `--apply`; a reverse proxy's own timeout may end the request sooner.

## `uguisu podcast export`

Prints every podcast, whatever its status, as a flat OPML 2.0 file on stdout: title, current feed URL and website, sorted by title. The same library always exports the same bytes. A private feed's URL is exported as stored, token included. With `--json` the document is wrapped: `{ schema, opml }`.

```text
uguisu podcast export > subscriptions.opml
```

## `uguisu podcast list` / `uguisu podcast show <id>`

`list` prints one line per podcast: id, title, episodes not detected as removed, status, time since the last refresh and the feed's fetch state. `show` prints the podcast's metadata, counters, its current source with fetch state (ETag, Last-Modified, failures, timestamps), the feed URL the feed announces when it failed the same-show check (`Announced:`, with why and the `move-feed` command), and the last fetch. JSON: `{ schema, podcasts: [detail] }` / `{ schema, podcast, source, announced, episodes_total, episodes_present, last_fetch }`.

## `uguisu podcast archive <id>` / `uguisu podcast remove <id> --yes`

`archive` is for a show that has ended or that you no longer want fetched, and want to keep ([ADR 0055](DECISIONS/0055-archiving-and-removing-a-podcast.md)): the podcast becomes `archived` and its feed `disabled`, and it is never refreshed — the scheduler and `refresh --all` skip it, and `podcast refresh <id>` is a conflict (exit 1). Episodes, files and history stay; an episode can still be downloaded by hand. `podcast resume` brings it back. Prints `<id> is archived`; JSON `{ schema, podcast_id, status }`.

`remove` takes the podcast out of the library: its sources, episodes, archive records, download jobs, policy, artwork and manifest rows, change and fetch logs and search entries go, and **every file stays** — media, sidecars, manifests, artwork, partial downloads. Without `--yes` it removes nothing and exits **2**, with `--json` too. It is refused (exit 1) while a refresh of the podcast or one of its downloads is running; queued and paused downloads go with it. Prints `Removed podcast <id> (<title>): <n> episodes; its <m> archived files stay on disk`; JSON `{ schema, podcast_id, title, episodes, files }`. Afterwards `archive orphans` reports the files as unknown media; adding the feed again and running `archive reconcile --rebuild --apply` puts the records back from their sidecars. Both commands work with `--server`.

## `uguisu podcast move-feed <id> <url> [--dry-run] [--force]`

Moves a podcast to another feed URL: the one its feed announces (`itunes:new-feed-url`) when Uguisu could not verify it, or one the old feed never mentioned ([ADR 0052](DECISIONS/0052-moving-a-feed-by-hand.md)). The URL is fetched and checked as an announced feed is — the same `podcast:guid`, or the same title and half of the stored episodes present — and then:

| The check | Without a flag | `--force` | `--dry-run` |
|---|---|---|---|
| passes | moves (exit 0) | moves | reports, exit 0 |
| fails | `Not moved: …` and exit **2** | moves | reports, exit 0 |

A move keeps every episode and its id; the old URL stays in the podcast's source history, and a refresh of the new feed follows (its report is printed, and its exit code is the command's). A rewritten GUID makes the episode a candidate duplicate for `episode resolve`. Exit 1 (conflict) for a URL that is another podcast's feed or a feed carrying another podcast's `podcast:guid`; 6, 7 or 8 for a URL that is refused, is not a podcast feed or cannot be fetched; 2 for an unknown podcast, or a podcast id or URL that is not one. The URL the podcast already uses prints `Podcast <id> already uses <url>`. JSON: `{ schema, podcast_id, from, to, verified, check, moved, report }`.

## `uguisu podcast refresh <id> | --all [--force]`

Refreshes one podcast (or every active one, up to `UGUISU_FEED_REFRESH_CONCURRENCY` at a time) and prints a refresh report:

```text
Versioned Show: fetched · 4 seen · 1 added · 1 updated · 2 unchanged · 0 malformed · 0 removed · 41 ms
  podcast metadata changed: description_html, artwork_url
  http 200 · 2513 bytes
  ! item 2: unparseable date `sometime`
```

Conditional requests use the stored ETag and Last-Modified; an unchanged body is detected by its fingerprint even when the host ignores validators. `--force` skips both and parses the feed again. `--json` prints the `RefreshReport` (`schema: 1`): `outcome` (`fetched` | `not_modified` + `reason` | `failed` + `kind`, `detail`), `http`, `podcast_changed_fields`, `episodes` counters, `feed_url` status, `warnings`, `truncated`, `removal_suppressed`, `duration_ms`. With `--all` the JSON is `{ schema, entries: [{ podcast_id, title, report | error }] }` and the exit code is the worst individual code.

## `uguisu feed inspect <url>`

Fetches and parses a feed the way a refresh would — kind, encoding, channel identity, item counts, identity sources, the first ten items with their identity keys and up to fifty warnings — **without touching the database**. Exit code 0 for a clean podcast feed, 9 when it parsed with problems (truncated, malformed items), 7 when it is not a podcast feed, 8 when it could not be fetched.

## `uguisu feed refresh <source-id> [--force]` / `uguisu feed status <source-id>`

`feed status` shows one source's fetch state (state, consecutive failures, validators, fingerprint, timestamps, last error) and its last fetch with warnings. `feed refresh` refreshes the podcast behind a current source. Source ids are printed by `podcast show`.

## `uguisu episode list <podcast-id>` / `uguisu episode show <id>`

`list` prints every episode of a podcast, newest first, one line each: id, publication date, archive state, duration and title. `show` prints one episode: what the feed says (publication, duration, GUID, identity key and where it came from), its archive state with any skip reason, the candidate it may duplicate, when it left the feed, its primary enclosure, its download job and its archived file with size, hash and last verification. Both follow the cursor to the end with `--server`. JSON: `{ schema, episodes: [episode] }` / `{ schema, episode, podcast_title, archive, job }`. An unknown or malformed id exits 2.

## `uguisu episode duplicates [--podcast <id>]` / `uguisu episode resolve <id> --same|--separate`

When a feed brings back an episode under a contradicting identity (a rewritten GUID, typically), the refresh stores the item as a **candidate duplicate**: skipped, never downloaded, linked to the episode it probably is (ADR 0014). `duplicates` lists every candidate with that episode and the reasons (`same enclosure url`, `same title`, …). JSON: `{ schema, duplicates: [{ candidate, original }] }`.

`resolve` decides one candidate (ADR 0051):

| Flag | Effect | Prints |
|---|---|---|
| `--same` | The candidate merges into the original, which keeps its file, history and id and takes over the new GUID; the candidate's row is gone. Cannot be undone. | `Merged episode <candidate> into <original>` |
| `--separate` | The candidate becomes an episode of its own; the archive policy runs for it as for a new one. | `Separated episode <candidate> from <original>`, plus `; queued for download` when the policy queued it |

`--same` is refused (exit 1) when the candidate has an archived file of its own or when the last fetch saw both items in the feed; `--separate` works in both cases. An id that is no candidate exits 1, an unknown one 2. JSON: `{ schema, resolution, candidate, original, episode, queued }`, where `episode` is the one that remains.

## `uguisu download <episode-id> [--priority low|normal|high] [--wait]`

Queues one episode (its primary enclosure) for download and prints the job: `outcome` is `created` (exit 0, a new job), `existing` (a pending or paused job already exists), `requeued` (a failed or cancelled job got a fresh attempt budget) or `already_completed`. Queueing is idempotent: one job per episode, ever. `episode download <id>` is an alias. An episode without an enclosure is refused with exit 2; a candidate duplicate, an episode marked skipped, one removed from the feed and one whose file an import or a rebuild archived, while that file is in place, are refused with exit 1 (`conflict`); with the file gone, downloading it is the repair ([ADR 0060](DECISIONS/0060-repairing-a-missing-file.md)). The last also holds for `download retry` and `download resume` of such an episode's old job, and `download retry-failed` leaves those jobs alone.

Nothing is downloaded until workers run. `uguisu serve` runs them; embedded, `--wait` starts them in this process, follows the job with a progress line on stderr and exits with the job's outcome (0 completed, 8 network failure, 6 blocked by policy, 11 disk full, 10 any other failure; Ctrl-C parks the job as `queued(shutdown)` and exits 1). With `--server`, `--wait` polls the server's job every second instead. Files land on the path `UGUISU_ARCHIVE_TEMPLATE` renders under the media directory, or at `<media dir>/<podcast-id>/<episode-id>.<ext>` when it cannot render; the partial file lives under `<media dir>/<podcast-id>/.uguisu-tmp/<job-id>.part` until the download is complete and hashed, then it is renamed into place atomically.

## `uguisu download podcast <id> [--priority p]`

Queues every downloadable episode of a podcast (newest first) and prints a summary: `created`, `existing`, `requeued`, `completed`, and `skipped` with a reason per episode (`no_enclosure`, `duplicate_candidate`, `skipped`, `removed_from_feed`, `already_archived`).

## `uguisu download list|show|cancel|pause|resume|retry`

`list` prints jobs newest first (`--state`, `--podcast`, `--after <job-id>` for the next page, `--limit`, default 50, max 500). `show <job-id>` prints one job with its validators, last error, live progress (when a worker in this process holds it) and attempt history. `cancel` stops a job for good (the row stays; `retry` queues it again), `pause` keeps its partial file for a later `resume`, `retry` re-queues a failed or cancelled job with a fresh attempt budget. A command that the job's state does not allow (pausing a completed job, resuming a running one) exits 1 with `conflict`.

## `uguisu download retry-failed` / `pause-all` / `resume-all` / `stats` / `reconcile [--deep]`

`retry-failed` re-queues every failed job, except one whose episode an import or a rebuild archived while that file is in place. `pause-all` stops claiming jobs and pauses the running ones; the pause is stored in the database, so it survives restarts, and a full disk sets it too (`paused (disk_full)`; `resume-all` clears it). `stats` prints counts per state, whether workers run in the answering process, the global pause and the next scheduled retry. `reconcile` re-runs the startup reconciliation (interrupted jobs back to `queued(recovered)`, half-finalized jobs completed or failed, orphan `.part` files reported, never deleted); `--deep` also checks that every completed file is still on disk (`missing_targets`, episodes marked `missing`).

## `uguisu download run [--until-idle]`

Runs the download workers in this process (embedded only): every queued job is downloaded with the configured concurrency, terminal events are printed as they happen, and the command stops on Ctrl-C/SIGTERM (running jobs are parked as `queued(shutdown)` within the grace period) or, with `--until-idle`, once nothing is claimable. The exit code is the worst outcome of the run (0 when every job completed).

## `uguisu archive list|show|missing|invalid`

`list` prints archived files newest first, with `--podcast`, `--state`, `--source-changed` (files whose feed now points at different audio; ARCHIVE_ENGINE.md §8) and `--limit`. `show <episode-id>` prints one record: path, size, hash, declared and sniffed type, the last verification and when it was archived. `missing` and `invalid` are the two listings that need attention — files whose record says they are gone, and files that are there but do not match. All four are read-only and work against a read-only media directory.

## `uguisu archive verify [<episode-id>] [--all] [--podcast <id>] [--full]`

Checks that archived files are still there and still correct. Without `--full` it compares type, size and modification time; with `--full` it hashes the whole file (1 MiB at a time, so the memory cost does not grow with the file). Name an episode, or pass `--all` or `--podcast`; naming nothing is a usage error rather than a whole-archive scan by accident. Only `--full` clears an `invalid` finding: a light check of a file found invalid still reports it `invalid`, with the check's own detail (`mtime_changed`, an I/O error), or `only a full pass clears this finding` when size and modification time both still match. A file whose tag write has not settled keeps its verdict, with `a tag write has not settled; check again after it`.

Verification **never changes a file and never deletes a record**. A tampered file is reported and left exactly as it is; a deleted file leaves its record, hash included, so it can be recovered. The exit code is **1** when anything is missing or invalid: a finding is a result to act on, not a crash.

## `uguisu archive path-preview <episode-id>`

Renders the current template for one episode and shows where the file would go, including any collision suffix and whether it differs from where the file is now. It creates no directory and reads no media, so it is safe to run before anything is downloaded and on an archive the process cannot write.

## `uguisu archive relocate [<episode-id>] [--all] [--podcast <id>] [--dry-run]`

Moves archived files to the paths the current template produces. Changing the template never moves files on its own; this command is how it happens. `--dry-run` prints the moves without making them. A move is a rename, so a file is always at one path or the other; a target on another filesystem is refused rather than copied, and an occupied target is refused rather than overwritten. One file that cannot move does not abandon the rest — the failure is reported and the run continues.

## `uguisu archive reconcile [--deep] [--rebuild] [--apply] [--podcast <id>]`

Registers finished downloads that have no archive record — **where their files lie**, so reconciliation never moves anything — and then confirms that recorded files exist. `--deep` adds a light verification of every artifact. Neither hashes the archive, only a file whose tag write an interrupted run left unsettled; hashing stays `verify --full`.

`--rebuild` is a different operation: it reads the sidecars on disk and puts back records the database has lost. A rebuilt record is always `unchecked` with reason `rebuilt` — a sidecar says what Uguisu knew, not what the bytes are, so only `verify` may write `verified`, and a record that already carries a checked finding is reported as a conflict rather than overwritten. Nothing is written without `--apply`; the dry run prints the same report. Documents whose media file is gone, that do not parse, or that name an episode this library does not have are counted and the first hundred named.

## `uguisu archive orphans`

Reports what nothing owns under the media directory, and removes none of it (ADR 0051): `leftovers` (scratch files of an interrupted import, restore, tag write, manifest, sidecar or artwork write, which no running writer holds), `orphan_parts` (`.part` files no download job owns), `unknown_media` (media files no archive record names), `stray_sidecars` (`<name>.<media extension>.json` whose media file is gone, as a relocation or a hand-moved file leaves) and `unreadable` (links and directories the walk did not enter). Each group is `{ count, sample }` with up to a hundred paths relative to the media directory; `clean` says whether all are empty. What to do with a finding is yours to decide: delete a leftover by hand, put a file back, or import it from outside the media directory. It walks the whole tree, so it is not run at start-up; with `--server` the CLI waits up to an hour. Exit **1** when anything is found, 0 when nothing is. [`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §21 has what each group means.

## `uguisu archive restore <path> [--apply] [--podcast <id>]`

Puts files that `archive verify` found `missing` back from a folder, such as a backup or a disk the files were moved to ([ADR 0060](DECISIONS/0060-repairing-a-missing-file.md)). A file is used only when its SHA-256 is the record's, whatever its name, as long as it keeps the record's extension; the folder is only read, and nothing at a record's path is ever replaced. Without `--apply` it is a dry run that prints the same plan. `--podcast` limits it to one podcast's missing files.

Prints `would restore from <folder>` or `restored from <folder>`, the number of files scanned and one line per missing file: `<action>: <path> [<- <source>] [(<detail>)]`, where the action is `restore`, `returned`, `source_only`, `taken`, `changed`, `not_found` or `failed` ([`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §22). JSON `{ schema, applied, source_root, scanned, items }`. Exit 0 with the report, also when files stay missing or one could not be copied (`failed`); 1 when the folder cannot be used (it does not exist, or it is or contains the media directory). With `--server`, the folder is on the server's machine and the CLI waits up to an hour.

## `uguisu archive redownload <episode-id>`

Downloads an episode again whose archived file is gone. It is refused (exit 1) while anything is at the record's path, including a file that cannot be read, while the old download's `.part` name holds a file (it may be a second name for the archived bytes), and when the episode has no archived file or is a candidate, skipped or gone from the feed; an unknown episode, or one without a usable enclosure, exits 2. A completed download is queued again; a file an import or a rebuild archived gets a download of its own. When it completes, the new file takes over the record. Prints what `download` prints for the job; `serve` or `download run` downloads it.

## `uguisu archive policy show|set|clear|list`

`show <podcast-id>` prints the values in force and whether they come from a stored override or the global defaults. `set <podcast-id> --mode auto|manual [--max-backlog n] [--max-age-days n] [--priority p]` stores an override; a flag left out means "use the global default", so one field can be overridden without pinning the others. `clear` removes the override. `list` shows every stored policy, with `-` where the global default applies.

Automatic archiving is off unless it is turned on, and a manual `download` is never blocked by a policy.

## `uguisu archive import <path> [--format podgrab|generic] [--apply] [--podcast <id>] [--podgrab-db <file>]`

Reads an existing archive someone else's tool wrote and copies what it can place. **Files are copied, never moved, and the source tree is never modified** — a test hashes it before and after. Nothing is copied without `--apply`; the dry run does the whole scan and match and prints the plan that would run. [`MIGRATION.md`](MIGRATION.md) walks through a whole move.

Each line of the plan says what would happen (`import`, `already_present`, `conflict`, `ambiguous`, `unmatched`, `invalid`), which episode it matched, how (`matched_by`: `source_database`, `embedded_guid` or `scored`), how confident that was and where it would land. A file that matches more than one episode closely is **never** imported: a file placed under the wrong episode is a quiet, permanent error, while an unmatched file is a line in a report. Every file's own tags are read: its title is tried when its name does not match, and an embedded episode GUID names its episode unless the name clearly names another. An episode that is already archived keeps its record: a file with the same bytes is `already_present`, any other a `conflict`. Several files for one episode import once when they are identical and are all `conflict` when they differ.

`--podgrab-db` reads Podgrab's `podgrab.db`, without writing it and only while Podgrab is stopped, and names each downloaded file's episode exactly; it implies `--format podgrab` and is a usage error with `--format generic`. `--podcast` skips the per-directory podcast gate when the layout does not name the show. The media directory cannot be imported into itself. With `--server`, the paths are on the server's machine and the CLI waits up to an hour for the answer.

## `uguisu archive sidecar show|write <episode-id>`

The portable `<media file>.json` written beside every archived file. `show` prints it; `write` renders it again from the archive record. Writing is idempotent — two writes of an unchanged record say the same thing.

## `uguisu archive manifest status|write|verify`

`status` lists which podcasts' `manifest.sha256` files are current and which have fallen behind. `write` writes the ones that have, or one with `--podcast`; the engine also writes them when the download queue goes quiet and on a clean shutdown, so this is for when you want it now. `verify --podcast <id>` compares a manifest with the archive and exits **1** on a finding; `--full` re-reads and hashes the files rather than comparing against the database. Neither writes a record: reading the bytes and recording what was found is `archive verify`'s job.

The manifests are written in `sha256sum` format, so `cd <media directory> && sha256sum -c .uguisu/manifests/<podcast-id>/manifest.sha256` checks a podcast without Uguisu.

## `uguisu archive artwork show|fetch <podcast-id>`

`show` prints the artwork Uguisu holds. `fetch` retrieves it from the URL the feed gives, through the same SSRF policy and redirect checks as every other untrusted request, and refuses anything whose bytes are not a JPEG, PNG or WebP however the server describes them. Artwork is stored under its own hash, so a replacement never destroys the image before it, and a conditional request means an unchanged image costs no bytes. Automatic fetching is off by default (`UGUISU_ARCHIVE_ARTWORK_FETCH`).

## `uguisu archive tags show|write <episode-id> [--mode fill_missing|sync]`

`show` prints the fields Uguisu manages as the file currently carries them. `write` writes them: `fill_missing` (the default) only fills gaps, `sync` makes the managed fields match Uguisu. **Neither removes anything, and neither touches a tag Uguisu does not manage** — a write starts from the tag already in the file and changes only the managed keys.

The file is copied, tagged, re-read, re-hashed and only then renamed over the original, so the artifact is intact until the replacement is proven readable. A file that does not already match its record is refused. A container Uguisu does not tag (WAV, AIFF, anything unrecognised) is reported as a result rather than an error, and whatever cannot be embedded stays in the sidecar.

## `uguisu db migrate|backup [<path>]|check|vacuum`

Database maintenance ([ADR 0056](DECISIONS/0056-database-maintenance.md)). Each runs only when asked for.

| Command | What it does | Prints |
|---|---|---|
| `db migrate` | opens the database, which applies any pending migration | `Schema <n>; nothing to apply` or `Schema <found> -> <now> (applied …)`; JSON `{ schema, found, now, applied }` |
| `db backup [<path>]` | writes a consistent, compacted copy while the database stays in use, then fsyncs and renames it into place; an existing file is refused (exit 1); without a path it goes to `<data dir>/backups/uguisu-<UTC time>.db` | `Backed up the database to <path> (<n> bytes)`; JSON `{ schema, path, bytes }` |
| `db check` | `integrity_check` and `foreign_key_check`, reading every page and changing nothing; exit **1** on any finding | `The database is sound`, or one line per finding; JSON `{ schema, ok, integrity, foreign_keys }` |
| `db vacuum` | rewrites the file without its free pages; writes wait, and it needs about the database's size in free space | `Vacuumed the database: <before> -> <after> bytes`; JSON `{ schema, bytes_before, bytes_after }` |

With `--server`, `backup`, `check` and `vacuum` run on the server; a backup then always goes to the server's own `backups/` directory, so `db backup <path> --server` is a usage error (exit 2), as is `db migrate --server`, since a server migrates when it starts. A backup holds what the database holds, the password hash and token digests included: keep it like the database.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (results found / feed resolved / refresh fetched or not modified) |
| 1 | Unexpected error (configuration, I/O, server unreachable, data directory locked, conflict, interrupted `--wait`) |
| 2 | Usage error, unknown id or confirmation required |
| 3 | Search completed with no results |
| 4 | Every provider failed or none is enabled |
| 5 | Results were found but the feed could not be resolved (`--resolve`, `resolve`, `podcast add`) |
| 6 | Refused by the network policy (private address, blocked host) |
| 7 | Feed error: the body is not XML, malformed, unsupported, too large or not a podcast feed |
| 8 | Network error: DNS, connection, TLS, timeout or an HTTP error status |
| 9 | Partial success: the refresh stored what it could but the feed was truncated or had malformed items |
| 10 | Download failed for a reason a retry cannot fix (validation, length mismatch, local I/O, target exists, attempts exhausted on a non-network error) |
| 11 | Disk full: the job and the queue are paused until space is freed and `download resume-all` runs |
| 12 | `serve` refused to bind a network address with no password set (ADR 0037); `auth set-password`, bind to loopback, or set `UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1` |

`archive verify`, `archive manifest verify` and `archive orphans` report a finding with exit code **1**: nothing went wrong with the command, but something is wrong with the archive. No new exit code was added — `1` already means "the result is not what you wanted", and a script distinguishes the cases from the JSON body (`missing`, `invalid`, `clean`), not from a number.

Errors and progress go to stderr; only data goes to stdout, so `uguisu … --json | jq` works.

## Authentication commands (Phase 9)

Authentication is off until a password is set, and required once one is (ADR 0035). See [`SECURITY.md`](SECURITY.md) §3.5 for the controls, [`API.md`](API.md) for how a client authenticates and [`DEPLOYMENT.md`](DEPLOYMENT.md) for putting it behind a proxy.

| Command | What it does |
|---|---|
| `auth set-password [--username <name>]` | read a password from the terminal without echoing it, or from stdin when there is no terminal, and store its argon2id hash. Every other open session is closed. |
| `auth token create <name> [--scope read\|write] [--expires-at RFC3339]` | issue a token and print its secret **once**. `write` by default. |
| `auth token list` | every token with its scope, state and last use. Never a secret: there is none to print. |
| `auth token revoke <id>` | stop a token working. The row stays, so the list can say it was revoked. |

A password never comes from an argument or an environment variable: an argument is in every process listing and shell history. `UGUISU_TOKEN` carries a token, which is what automation should hold instead of the password.

Embedded, `auth set-password` does not ask for the current password — anyone who can run it already has the database and every archived file — so it is also the way back in after a forgotten one. Over `--server` the current password **is** required, because there the network is the boundary. A server holds an exclusive lock on its data directory, so setting a password on a running instance means `--server`, not a second process — and the *first* password therefore means stopping the server, or `POST /api/v1/auth/password`, which is public exactly until one exists.

The same lock decides where the **first token** comes from: `--server auth token create` authenticates with a token you do not have yet, so mint the first one with the server stopped, or on the web UI's Settings page (`POST /api/v1/auth/tokens` with a browser session). After that a `write` token can mint more.

## Attribution

Directory providers require attribution when their data is shown: the JSON `attribution` array and the text output's footer list them (Podcast Index Terms of Service §7).

## Service commands (Phase 7)

`uguisu serve` is the only command that starts anything long-lived: the API, the download workers, the feed-refresh scheduler, the search-index build and the archive check. It **refuses to bind a network address when no password is set** (ADR 0037) — loopback is the default and needs none. Everything below acts on the same data directory and exits. See [`SERVICE.md`](SERVICE.md), and [`DEPLOYMENT.md`](DEPLOYMENT.md) for the refusal, the override and a reverse proxy.

`uguisu health [--bind <addr>]` asks a running server whether it is up, for a container's health check or a script: exit 0 when `GET /api/v1/health` answers `ok`, 1 otherwise. It asks `--server` when given, else the address `serve` binds (`--bind`, `UGUISU_BIND`), on loopback when that address is unspecified (`0.0.0.0`, `::`). It needs no credential and no data directory. Prints `Healthy: Uguisu <version> at <url>`; JSON `{ schema, status, version }`.

| `serve` flag | Environment | Meaning |
|---|---|---|
| `--bind <addr>` | `UGUISU_BIND` | Socket address; `127.0.0.1:8484` by default |
| `--web <dir>` | `UGUISU_WEB_DIR` | Directory holding the built web UI; `web/dist` by default, relative to the working directory. A directory that is not there serves the API alone. |
| `--trusted-proxy <ip\|cidr>` | `UGUISU_TRUSTED_PROXIES` | A reverse proxy whose `X-Forwarded-For` names the client, for the login limiter and the logs; repeatable, or comma-separated. Every address (`0.0.0.0/0`, `::/0`) is refused ([ADR 0062](DECISIONS/0062-trusted-proxies.md)). |
| `--allow-insecure-exposure` | `UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE` | Bind a network address even with no password set. Off by default; without it `serve` exits 12. A deployment knob like `--bind`, not a stored setting — a database row that permits insecure exposure could be written through the API it protects. |
| `--cookie-secure` | `UGUISU_AUTH_COOKIE_SECURE` | Mark the session cookie `Secure`. Set it when a reverse proxy terminates TLS: Uguisu does not terminate TLS and will not guess from a header it does not trust. |

| Command | What it does |
|---|---|
| `scheduler status` | whether the scheduler is enabled, running here, paused; how many podcasts are due, when the next one is, when housekeeping last ran |
| `scheduler pause [--reason "…"]` / `scheduler resume` | stop and start automatic refreshing; persisted across restarts |
| `scheduler run` | one pass now, without waiting for the loop; it waits for the refreshes it starts before exiting |
| `scheduler maintenance` | prune the event log, expire discovery-cache rows, and close sessions that can no longer authenticate |
| `podcast pause <id>` / `podcast resume <id>` | take one show out of the schedule, or put it back (due inside the next interval) |
| `podcast schedule <id> [--at RFC3339]` | set when a show is next due; without `--at`, as soon as the scheduler looks |
| `config list` | every key with its value and where the value came from |
| `config get <KEY>` | one key |
| `config set <KEY> <VALUE>` | store a setting; refused for a key the environment pins (and for the six that may never be stored, [`CONFIGURATION.md`](CONFIGURATION.md)) |
| `config unset <KEY>` | clear a stored setting, whatever state it is in |
| `config validate` | as `list`, and exits non-zero when a stored value is being ignored |
| `search library <text> [--limit] [--prefix] [--explain]` | search the local library; operators are ordinary words |
| `search reindex` | rebuild the search index now |

Exit codes follow the existing conventions: `search library` exits 3 when it found nothing — no matches, an empty query or an index that has not been built — and prints which of those it was; `config validate` exits 2 when a stored value is being ignored.
