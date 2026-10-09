# Configuration

Uguisu is configured through environment variables and, since Phase 7, through settings stored in the database. `uguisu config list` prints every key with its value and where that value came from; `uguisu config validate` does the same and exits non-zero when a stored value is being ignored. `core::settings::SETTINGS` is the one list of keys, and this document follows it.

**The environment wins over a stored setting** — see "Precedence" below, and [ADR 0028](DECISIONS/0028-persisted-settings-and-precedence.md) for why that is the opposite of what the sketch said.

Values are parsed strictly: an unparseable number, duration or boolean is a startup error naming the variable. A boolean takes `1`, `true`, `yes` or `on` and their negatives, in any case — one vocabulary, shared by the settings and by the `serve` flags that mirror them.

## Data directory and feed engine

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_DATA_DIR` | platform data directory (`~/.local/share/uguisu`, `~/Library/Application Support/uguisu`, `%APPDATA%\uguisu\data`); `/data` in the Docker image | Directory holding `uguisu.db` (+ WAL files), `uguisu.lock` and `uguisu.pid`. Created on first use. The CLI's `--data-dir` overrides it. |
| `UGUISU_FEED_MAX_BYTES` | `52428800` (50 MiB) | Cap on a feed body. Enforced by the HTTP client while reading and by the parser; larger feeds fail with `too_large`. |
| `UGUISU_FEED_MAX_ITEMS` | `50000` | Items parsed per feed, at least 1; further items are dropped and the fetch is `truncated` (removal detection off for that fetch). |
| `UGUISU_FEED_REFRESH_TIMEOUT_MS` | `60000` | Budget for one whole refresh: fetch, verification fetch of an announced feed URL, parse. Exceeding it is a `timeout` failure. |
| `UGUISU_FEED_REFRESH_CONCURRENCY` | `8` | Parallel podcasts in the scheduler, in `podcast refresh --all` / `POST /api/v1/podcasts/refresh`, and feeds resolved at once when an OPML import is applied. |
| `UGUISU_FEED_RETAIN_FETCHES` | `50` | Fetch-log rows (`feed_fetches`) kept per podcast; at least 1. |
| `UGUISU_FEED_REMOVAL_STREAK` | `2` | Consecutive complete fetches without an episode before it is marked removed from the feed (never deleted). |
| `UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT` | `50` | Percent of the present episodes that may be missing from one complete fetch before removal detection is suppressed for that fetch. 1–100; 100 never suppresses. |

Not exposed yet (constants in `FeedConfig`/`FeedLimits`): parser depth 64, text node cap 64 KiB, field cap 4 KiB, 16 enclosures and 64 raw extensions per item, 100 isolated malformed items, podcast `error` status after 5 consecutive failures, a refresh back-off that doubles per further consecutive failure up to 24 h.

## Downloads and media

Full semantics in [`DOWNLOAD_ENGINE.md`](DOWNLOAD_ENGINE.md).

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_MEDIA_DIR` | `<data dir>/media`; `/media/podcasts` in the Docker image | Root of the media tree. Downloads land on the path the archive template renders (see below); partial files stay in `<podcast-id>/.uguisu-tmp/` so finalization is an intra-filesystem rename (ADR 0020). Created lazily, so read-only commands never touch it. |
| `UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY` | `3` | Jobs downloading at once in one process. |
| `UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY` | `1` | Jobs per `scheme://host:port`; must not exceed the global limit. |
| `UGUISU_DOWNLOAD_MAX_ATTEMPTS` | `8` | Attempts before a job fails with `max_attempts`; at least 1. |
| `UGUISU_DOWNLOAD_BACKOFF_BASE_MS` | `30000` | First retry delay; doubles per attempt with jitter. |
| `UGUISU_DOWNLOAD_BACKOFF_MAX_MS` | `21600000` (6 h) | Cap for the backoff and for an honoured `Retry-After`. |
| `UGUISU_DOWNLOAD_IDLE_TIMEOUT_MS` | `60000` | A body that sends nothing for this long is a `timeout` (retryable). |
| `UGUISU_DOWNLOAD_PROGRESS_INTERVAL_MS` | `1000` | Floor between progress writes and `download.progress` events per job (and at least 1 MiB or 1 % of movement). |
| `UGUISU_DOWNLOAD_MAX_BYTES` | `8589934592` (8 GiB) | Largest accepted media file; a larger body fails with `validation` while streaming. At least 1 MiB. |
| `UGUISU_DOWNLOAD_MIN_FREE_BYTES` | `268435456` (256 MiB) | Space that must remain free on the media file system; falling short pauses the queue with `disk_full`. |
| `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS` | `15000` | How long a graceful stop waits for running jobs to park as `queued(shutdown)`. |

Not exposed yet (constants in the download engine): 256 KiB write buffer, 1 MiB re-hash buffer, 5 redirect hops, 64 MiB assumed size when a length is unknown, 30 s maximum scheduler tick, five rename retries on Windows sharing violations.

## Archive

Full semantics in [`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md).

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_ARCHIVE_TEMPLATE` | `{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}` | Path template (ADR 0009/0022). An unparseable template is reported once at startup and every download keeps the identifier layout, rather than failing each enqueue in turn. |
| `UGUISU_ARCHIVE_PATH_PROFILE` | `portable` | `windows`, `posix` or `portable` (the intersection). Decides what a segment may contain, so an archive written under `portable` moves between systems unchanged. Do not choose `posix` for an archive on Windows or on an NTFS or FAT volume: it keeps `:`, `?`, device names and trailing dots, which those file systems refuse or read differently. |
| `UGUISU_ARCHIVE_VERIFY_ON_COMPLETION` | `true` | Verify right after a download. With `false`, the file is still confirmed to exist — one `stat` — so a finalization that did not land is still caught. |
| `UGUISU_ARCHIVE_VERIFY_DEPTH` | `light` | `existence`, `light` or `full`. The depth of that check; `full` hashes every finished download a second time, which doubles the I/O of a large transfer. |
| `UGUISU_ARCHIVE_AUTO_DOWNLOAD` | `false` | Queue discovered episodes without being asked. Off by design (ADR 0023); a per-podcast policy can opt one podcast in or out either way. |
| `UGUISU_ARCHIVE_MAX_BACKLOG` | `3` | Episodes of one podcast that may be waiting to be archived at once — queued, retrying, downloading or finalizing. `0` means no limit. |
| `UGUISU_ARCHIVE_MAX_AGE_DAYS` | `0` | The policy ignores episodes published longer ago than this. `0` means no limit. An episode with no publication date is never excluded by it. |
| `UGUISU_ARCHIVE_PRIORITY` | `normal` | `low`, `normal` or `high`: queue priority for jobs the policy creates. Manual commands keep their own. |
| `UGUISU_ARCHIVE_SIDECARS` | `true` | Write a `<media>.json` sidecar beside each archived file. With `false` nothing is written and nothing existing is removed — but `archive reconcile --rebuild` then has less to rebuild from, which is the whole reason the default is on. |
| `UGUISU_ARCHIVE_MANIFESTS` | `true` | Maintain a `sha256sum -c` manifest per podcast. With `false` the stale marker is still kept, so turning it back on writes what was missed rather than starting over. |
| `UGUISU_ARCHIVE_ARTWORK_FETCH` | `false` | Fetch podcast artwork after a refresh that stored a changed feed, when the cover URL changed or no image is stored yet (ADR 0047). Off by design: a refresh must not start downloading images on its own. `archive artwork fetch` works regardless. |
| `UGUISU_ARCHIVE_ARTWORK_MAX_BYTES` | `16777216` | Byte cap for one image, enforced by `Profile::Artwork` on the declared length and again while reading. |
| `UGUISU_ARCHIVE_TAG_MODE` | `fill_missing` | The default for `archive tags write`: `fill_missing` (also accepted as `fill-missing`) writes a managed field only where the file has none, `sync` makes the managed fields agree with Uguisu. Neither ever removes a tag Uguisu does not manage. Setting this does **not** make Uguisu tag anything on its own — tag writing is always an explicit command. |
| `UGUISU_ARCHIVE_IMPORT_MATCH_THRESHOLD` | `85` | The score (0–100) a candidate must reach to be imported. A candidate must also beat its runner-up by 10 points; anything closer is ambiguous and is reported instead of imported. Values 1–100; `0` is refused, because it would mean "import everything". |

Not exposed yet (constants in the archive engine): 120 characters and 200 UTF-8 bytes per path segment and 200 characters per path, a 1 MiB hashing buffer, 200 records per verification batch, six characters of episode id in a collision suffix, 256 KiB per sidecar document, a scan depth of 12 directories, the 10-point match margin and the 55-point floor below which a candidate is unmatched rather than ambiguous, and the weights of the five matching signals (title 40, date 25, number 15, duration 12, size 8).

## Networking (shared by discovery and feeds)

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_HTTP_ALLOW_PRIVATE_HOSTS` | unset | Comma-separated host names that may resolve to private, loopback or link-local addresses (a LAN feed mirror). Everything else non-public is refused (`docs/SECURITY.md` §3.1). |
| `UGUISU_HTTP_USER_AGENT` | `Uguisu/<version> (+https://github.com/suzora/uguisu)` | `User-Agent` header. |
| `UGUISU_HTTP_CONNECT_TIMEOUT_MS` | `10000` | TCP/TLS connect timeout. |
| `UGUISU_HTTP_REQUEST_TIMEOUT_MS` | `30000` | Per-request timeout (feed fetches; provider calls are capped by the provider timeout below). |

Feed fetches make up to three attempts on a transient failure, with exponential back-off (500 ms base, 10 s cap, `Retry-After` honoured up to 30 s). Media transfers do **not** retry inside the request: the download queue owns that schedule (above), so a failed body resumes from disk instead of restarting.

## Discovery

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_DISCOVERY_APPLE_ENABLED` | `true` | Apple iTunes Search (keyless). |
| `UGUISU_APPLE_COUNTRY` | `US` | Apple storefront (ISO 3166-1 alpha-2). |
| `UGUISU_APPLE_LANG` | unset | Apple result language: `en_us` or `ja_jp`, the only two Apple accepts. |
| `UGUISU_DISCOVERY_APPLE_BASE_URL` | `https://itunes.apple.com` | Override for tests. |
| `UGUISU_DISCOVERY_PODCASTINDEX_ENABLED` | `true` when both credentials below are set, else `false` | Podcast Index. Without both credentials it stays off whatever this says. |
| `UGUISU_PODCASTINDEX_KEY` / `UGUISU_PODCASTINDEX_SECRET` | unset | Your own Podcast Index credentials (never logged). |
| `UGUISU_DISCOVERY_PODCASTINDEX_BASE_URL` | `https://api.podcastindex.org/api/1.0` | Override for tests. Environment only, since the key and secret go wherever it points. |
| `UGUISU_DISCOVERY_GPODDERNET_ENABLED` | `false` | gpodder.net (opt in). |
| `UGUISU_DISCOVERY_GPODDERNET_BASE_URL` | `https://gpodder.net` | Override for tests. |
| `UGUISU_DISCOVERY_SOFT_DEADLINE_MS` | `2000` | First results are returned when every provider answered or this elapsed. |
| `UGUISU_DISCOVERY_HARD_DEADLINE_MS` | `8000` | Overall search deadline. |
| `UGUISU_DISCOVERY_PROVIDER_TIMEOUT_MS` | `6000` | Per-provider call timeout. |
| `UGUISU_DISCOVERY_LIMIT` | `25` | Default result limit; 1–100. |
| `UGUISU_DISCOVERY_CACHE_SEARCH_TTL_SECS` | `900` | Search cache TTL (provider `max-age` shortens it; `no-cache` disables caching of that answer). |
| `UGUISU_DISCOVERY_CACHE_LOOKUP_TTL_SECS` | `86400` | Lookup cache TTL. |
| `UGUISU_DISCOVERY_CACHE_MAX_ENTRIES` | `10000` | In-memory cache size. |

## Process and CLI

These are not settings. They decide how a process starts, so they are not in
the `settings` table and cannot be changed through the API: a database row that
permits insecure exposure could be written through the very API it protects
(ADR 0031's precedent, ADR 0037's reason).

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_SERVER` | unset | Send CLI commands to a running server instead of running embedded (`docs/CLI.md`). |
| `UGUISU_TOKEN` | unset | API token for server mode (`uguisu auth token create`). Sent as `Authorization: Bearer`; a value a header cannot carry is a usage error, not a silently anonymous request. |
| `UGUISU_BIND` | `127.0.0.1:8484` | `uguisu serve` bind address. |
| `UGUISU_WEB_DIR` | `web/dist` | The built web UI `uguisu serve` serves, relative to the working directory unless absolute; a directory that is not there serves the API alone. Also `--web`. The desktop shell tries it first ([ADR 0041](DECISIONS/0041-desktop-shell-and-embedded-server.md)). |
| `UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE` | `0` | Bind a network address with no password set. Without it, `serve` exits 12 (ADR 0037, [`DEPLOYMENT.md`](DEPLOYMENT.md)). Also `--allow-insecure-exposure`. |
| `UGUISU_AUTH_COOKIE_SECURE` | `0` | Mark the session cookie `Secure`, for a deployment with TLS in front — Uguisu will not infer TLS from a header it does not trust. Also `--cookie-secure`. |
| `UGUISU_TRUSTED_PROXIES` | unset | Comma-separated addresses or networks of reverse proxies whose `X-Forwarded-For` names the client, for the login limiter and the logs; `0.0.0.0/0` and `::/0` are refused ([ADR 0062](DECISIONS/0062-trusted-proxies.md), [`DEPLOYMENT.md`](DEPLOYMENT.md)). Also `--trusted-proxy`. |
| `UGUISU_LOG` | `warn` | `tracing` filter (`info`, `uguisu_engine=debug`, …); logs go to stderr. |
| `UGUISU_LOG_FORMAT` | `pretty` | Format of the `uguisu` binary's log events: `pretty`, or `json` for one JSON object per event. Also `--log-format`; command output follows `--json` ([`CLI.md`](CLI.md)). The desktop shell ignores it and writes text. A URL in a log keeps no userinfo, query or fragment ([`SECURITY.md`](SECURITY.md) §3.5). |

## Precedence

```
built-in defaults  <  stored settings  <  environment  <  CLI flags
```

Per-podcast overrides (refresh interval, archive policy) sit above all of it for the podcast they belong to.

**A key set in the environment is pinned.** `uguisu config set` and `PUT /api/v1/settings/{key}` refuse it with a conflict rather than storing a value that would be ignored; `config list` marks it. The remedy is to unset the variable — there is no `--force`.

**Six keys may never be stored:** `UGUISU_PODCASTINDEX_KEY`, `UGUISU_PODCASTINDEX_SECRET`, `UGUISU_DISCOVERY_PODCASTINDEX_BASE_URL`, `UGUISU_DATA_DIR`, `UGUISU_MEDIA_DIR` and `UGUISU_HTTP_ALLOW_PRIVATE_HOSTS`. Secrets stay in the environment or a file, and so does where they are sent; a row inside the database cannot say where the database lives; and the SSRF allowlist is part of the network policy (`SECURITY.md` §3.1).

**Live versus restart.** The feed, archive and housekeeping keys are read per operation, so storing one takes effect at once. The HTTP clients, the discovery stack, the download queue and the archive path template and profile capture their configuration at start-up; storing one of those keys is accepted and reported as `restart_required`. `config list` says which is which.

**A stored value that stops parsing is quarantined, not deleted:** the row stays as written, the engine starts without it, and `config validate` shows it with the parser's message ([ADR 0028](DECISIONS/0028-persisted-settings-and-precedence.md)).

**`config.toml` does not exist**, and v1 has no file layer (ADR 0028). An environment file — systemd's `EnvironmentFile=`, `docker --env-file`, a compose `env_file:` — is the file-based configuration.

## Service and housekeeping

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_FEED_SCHEDULER` | `true` | Whether `uguisu serve` refreshes feeds on its own. Off means no automatic refreshes at all; housekeeping still runs. |
| `UGUISU_FEED_REFRESH_INTERVAL_SECS` | `3600` | Default interval between refreshes of one podcast, when the podcast has none of its own. Never shorter than the origin's `Cache-Control: max-age`. At least 60 s, at most a year. |
| `UGUISU_MAINTENANCE_INTERVAL_SECS` | `86400` | How often housekeeping runs (event pruning, discovery-cache expiry, session pruning). At least 60 s, at most a year. |
| `UGUISU_EVENTS_RETAIN_DAYS` | `30` | Days of event history kept. `0` means no age limit. |
| `UGUISU_EVENTS_RETAIN_MAX_ROWS` | `100000` | Event rows kept regardless of age, newest first. `0` means no row limit — never "delete everything". |
