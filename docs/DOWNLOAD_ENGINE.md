# Download Engine

How Uguisu turns an episode into a file on disk: the persistent queue, the worker that streams and validates one job, the resume protocol, finalization, recovery after a crash, and everything the API and CLI expose. Built (`uguisu-download`, wired by `uguisu-engine`). The lifecycle diagram is in [`STATE_MACHINES.md`](STATE_MACHINES.md) §3, the tables in [`DATA_MODEL.md`](DATA_MODEL.md) §8, the decisions in ADRs [0018](DECISIONS/0018-download-state-machine-and-queue.md), [0019](DECISIONS/0019-resume-and-finalization.md) and [0020](DECISIONS/0020-id-based-media-paths.md).

## 0. Guarantees

1. **A file at a final path is complete.** Bytes are written to `.part` under a temporary directory and moved into place without ever replacing a file (§7), after the whole body was received, counted and hashed. A reader that sees the file sees all of it.
2. **One job per episode, ever.** `download_jobs.episode_id` is unique. Enqueueing is idempotent: the same episode queued ten times is one job and one download.
3. **A crash loses at most one progress interval.** Whatever the process was doing, the next start reconciles the queue and the retry resumes from a byte offset the database had acknowledged and the file still holds.
4. **A resume is validated.** Bytes are appended only when the server confirmed the range with a `206` and the validator still matches; otherwise the transfer restarts from zero.
5. **Nothing is deleted to make room.** An existing target file, an unknown `.part`, a file that vanished: all are reported, never removed.
6. **Failures are typed and bounded.** Every attempt is classified, retryable errors follow one backoff schedule with a budget, and non-retryable ones stop immediately with a reason a user can act on.

## 1. Pipeline

```text
episode + enclosure
   → enqueue            one job row, `queued(user)`, `download.queued`
   → claim              scheduler: global slot + per-host slot + CAS to `downloading`, attempt row
   → prepare            reload episode, adopt a changed URL, resolve paths, check free space
   → inspect .part      offset = min(file length, acknowledged bytes), truncate, re-hash prefix
   → request            GET with Range/If-Range, identity encoding, idle timeout, cancel token
   → stream             write → hash → count → throttled progress (DB + bus)
   → validate           length against Content-Length, non-empty, sniff the container
   → finalize           fsync, CAS to `finalizing` with the hash, rename, fsync the directory
   → persist            CAS to `completed`, close the attempt, archive_state = archived,
                        `download.completed` in one transaction, published after commit
```

Steps are re-entrant: each one either completes or leaves a state the next attempt can continue from. Nothing but the worker writes a job's progress, and every worker write is a compare-and-set on the state it expects (§3).

## 2. States and reasons

`DownloadState` (persisted in `download_jobs.state`):

| State | Meaning | Leaves to |
|---|---|---|
| `queued` | waiting for a worker | `downloading`, `paused`, `cancelled` |
| `downloading` | a worker holds it | `finalizing`, `retrying`, `paused`, `failed`, `cancelled`, `queued` |
| `finalizing` | body complete and hashed, rename in progress | `completed`, `failed` |
| `retrying` | transient failure, waiting for `next_attempt_at` | `queued`, `downloading`, `paused`, `cancelled` |
| `paused` | kept with its `.part`, needs a command | `queued`, `cancelled` |
| `completed` | the file is at its final path | `queued` (redownload, only when the file is gone) |
| `failed` | gave up, `.part` kept for a retry | `queued` (retry) |
| `cancelled` | the user stopped it | `queued` (retry) |

`state_reason` is a closed vocabulary, not free text. A `retrying` job carries its error kind (§9) instead; `finalizing` and `completed` carry none.

| Value | Set when |
|---|---|
| `user` | the job was created (by a user or the archive policy), or a user paused or cancelled it |
| `requeued` | a failed, cancelled or retrying job was queued again with a fresh budget |
| `recovered` | startup found it `downloading`: the previous process died |
| `shutdown` | a graceful stop parked a running job |
| `resumed` | a paused job was resumed |
| `source_changed` | the episode's enclosure URL changed; the transfer restarts |
| `paused_all` | `pause-all` paused the queue while this job was pending or running |
| `disk_full` | the media file system ran out of space |
| `max_attempts` | the attempt budget is spent |
| `not_retryable` | the error kind is never retried |
| `validation` | the body was empty, or the enclosure URL or a stored path is unusable |
| `target_exists` | a different file already occupies the target path, or took it during the transfer, under any case the file system folds; a retry renders the target again, and the taken name sends it to the identifier layout |
| `finalization` | the rename failed after a complete download |
| `finalization_lost` | a crash while finalizing left neither `.part` nor target |
| `source_missing` | the episode or its enclosure is gone |
| `redownload` | a completed job's archived file is gone and a person asked for it again ([ADR 0060](DECISIONS/0060-repairing-a-missing-file.md)) |

The brief's `Preparing` and `Validating` are steps inside `downloading`, logged but not persisted: they have no distinct recovery. `Interrupted` is `queued(recovered)` or `queued(shutdown)`. `Skipped` is an enqueue refusal, not a job state. `Verified` belongs to the archive file ([`STATE_MACHINES.md`](STATE_MACHINES.md) §4).

`transition(state, event)` in `uguisu-download::state` is the pure table of allowed edges: user commands and claims consult it before writing, and the worker and startup recovery write compare-and-sets along its edges. Its tests assert that only a redownload leaves `completed`, that every state is reachable from `queued`, and that the worker's own events apply only to a job a worker holds.

## 3. Who may write what

The database is the truth, and the running worker is not privileged:

- Every worker write is `UPDATE … WHERE id = ? AND state IN (expected)`. If the row moved on, the update matches nothing and the worker only closes its file and records the attempt.
- A user command (`cancel`, `pause`, `resume`, `retry`) writes the database first and *then* cancels the job's token. The worker wakes up cancelled, sees its compare-and-set fail and leaves the final state alone. There is no window where a command is lost or overwritten.
- `finalizing` is not cancellable. Once the hash is written, the job finishes the rename even during shutdown; interrupting there is what `finalization_lost` recovery exists for.

## 4. The queue

**Enqueue.** `enqueue_episode(episode, priority)` returns `created`, `existing`, `requeued` (a failed or cancelled job with a fresh budget) or `already_completed`; it refuses an episode with no enclosure (`invalid`), an unresolved candidate duplicate, one marked skipped, one detected as removed from the feed and one whose archived file an import or a rebuild put in place, while it is there (each a `conflict`), and an enclosure URL without a host and port (`invalid`). `retry` and `resume` of that last kind's old job are refused the same way. `enqueue_podcast` walks a podcast's episodes newest first in pages of 500 and returns counters plus a `skipped` list with a reason per episode (`already_archived` for the import or rebuild case). Enqueueing never starts a transfer.

**Order.** Claims are `ORDER BY priority DESC, created_at ASC, id ASC` — priority first, FIFO within a priority, ties broken by the ULID. Priorities are `low`, `normal` (default) and `high`.

**Admission.** One scheduler task owns a set of running jobs bounded by `global_concurrency`. A claim selects the next eligible job, asks `HostThrottles` for a synchronous permit for its `host_key` (`scheme://host:port`), and only then compare-and-sets it to `downloading` in a `BEGIN IMMEDIATE` transaction. Hosts without a free permit are excluded from the query, so a saturated host never blocks other hosts, and a worker never waits for a host permit while holding a global slot.

**Waking up.** The scheduler sleeps on a notification (enqueue, resume, retry, a worker finishing) or a timer of `min(next due retry − now, 30 s)`. The timer exists only to pick up `retrying` jobs whose `next_attempt_at` has passed; feed refreshes have their own loop ([`SERVICE.md`](SERVICE.md)).

**Workers do not run by themselves.** `Engine::open` reconciles the queue but starts nothing. `uguisu serve`, the desktop shell, `uguisu download run` and `uguisu download <id> --wait` call `start_downloads()`. Queueing from the API on a server that is running does start the transfer, because that server's workers are running.

## 5. Resume protocol

Before a request the worker sets `offset = min(file length, bytes_downloaded)` and truncates the `.part` to it. The tail a crash left unacknowledged (at most one progress interval) is discarded rather than trusted. The prefix is re-hashed in a blocking task with a 1 MiB buffer, which doubles as a readability check; SHA-256 has no resumable state to persist, and reading is far cheaper than transferring (`docs/benchmarks/2026-09-18-phase4.md`).

A resume is attempted only when `offset > 0`, the server advertised `Accept-Ranges: bytes` (or answered a `206` before) and a strong `ETag` or a `Last-Modified` is stored. Weak validators (`W/"…"`) are never sent in `If-Range`.

| Response | Interpretation |
|---|---|
| `200` without a Range request | fresh transfer |
| `200` after a Range request | the validator changed or the host ignores ranges: truncate to zero and restart within the same attempt (`range_ignored` in the attempt detail) |
| `206` | `Content-Range: bytes N-…/total` must start exactly at the requested offset, else `range_invalid`, not retryable |
| `416` | the `.part` already holds the full length → validate and finalize; otherwise restart from zero |
| `401`, `403`, `404`, `410` | fail without a retry; the detail names the status, never a header |
| `408`, `425`, `429`, `500`, `502`–`504` | schedule a retry, `Retry-After` preferred over the backoff |
| other `4xx`/`5xx` | fail |

`Range` and `If-Range` ride every redirect hop, so a CDN that redirects per request still resumes. The recorded validators, `Accept-Ranges`, `Content-Type` and the total length are stored on the job as they are learned.

## 6. Integrity and validation

- The body is hashed with SHA-256 **while streaming** (ADR 0007); there is no second read.
- The media client is built with decompression disabled and sends `Accept-Encoding: identity`, so `Content-Length` means bytes on disk. A host that answers with `Content-Encoding` anyway is stored as served and the fact is noted.
- When a total length is known and the received total differs, the attempt fails with `content_length_mismatch`, which is retryable: the next attempt resumes at the new offset.
- An empty body fails with `validation`. A body larger than `max_bytes` fails with `validation` as soon as the cap is crossed.
- The first bytes are sniffed (ID3, MPEG sync, `fLaC`, `OggS`, `RIFF`, `ftyp`, EBML) and recorded as `sniffed_type`. Uguisu records; it never refuses a file for its container.

## 7. Destinations and finalization

The target is decided once, at enqueue: the archive template's rendered path (ADR 0009, 0022), or the identifier layout of ADR 0020 when no rendered path is usable. The `.part` is always named by identifiers.

```text
<media dir>/<rendered template path>                   the target
<media dir>/<podcast-id>/<episode-id>.<ext>            the target when no rendered path is usable
<media dir>/<podcast-id>/.uguisu-tmp/<job-id>.part     while downloading
```

`<ext>` is a whitelisted extension for the enclosure's MIME type, otherwise a lowercase alphanumeric extension of at most five characters from the URL, otherwise `bin`. `uguisu-download` asks the engine's `DestinationResolver` for the rendered path, checks that it resolves inside the media directory and that no other job claims it, and falls back to the identifier layout otherwise ([`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §7). Feed text reaches the target only through the template's sanitization ([`SECURITY.md`](SECURITY.md) §3.2) and never reaches the `.part`. Paths are stored relative to the media directory with POSIX separators, so the archive can move.

Finalization, in order:

1. Flush the writer, `sync_all` the file, drop the handle (Windows cannot rename an open file).
2. Compare-and-set `downloading → finalizing`, storing the hash, the byte count and the sniffed type. This write is the durable "the body is complete" marker.
3. Create the target directory, then move the `.part` into place without ever replacing a file: a hard link to the target, which fails when the name is taken in any case the file system folds, then removing the `.part` name. Where a link cannot be made (FAT, some network shares) it falls back to checking the target is absent and renaming.
4. On Unix, fsync the target's directory so the rename survives a power cut. Windows has no directory fsync; sharing violations (os error 32 or 5, typically an antivirus scanner holding the new file) are retried five times with a doubling 100 ms delay before `failed(finalization)`.
5. One transaction: compare-and-set `finalizing → completed`, close the attempt, set the episode's `archive_state` to `archived`, insert `download.completed`. Publish after commit.

Durability, by platform: the media file is `sync_all`ed and (on Unix) its directory fsynced before it is visible under its final name; the database runs WAL with `synchronous=NORMAL` (ADR 0002), so a power cut can lose the last commits but never corrupt the file. Progress writes are deliberately not fsynced — they are advisory, and the `min(file, database)` rule at the start of the next attempt repairs any disagreement.

## 8. Disk space

Before an attempt the worker asks the media file system for free space and compares it with what is still needed (the remaining bytes, else the enclosure's declared length, else a 64 MiB floor) plus `min_free_bytes`. If it does not fit, or a write fails with `ENOSPC`, the job becomes `paused(disk_full)`, **the attempt is refunded** (a full disk must not burn the budget), the whole queue is paused with `paused_all(disk_full)` and `download.paused_all` is emitted. The pause lives in a one-row table, so it survives restarts; only `resume-all` lifts it, after which running jobs resume from their `.part` with a Range request.

## 9. Retry taxonomy

| Kind | Retryable | Typical cause |
|---|---|---|
| `network` | yes | connection reset, transport failure, early close |
| `timeout` | yes | no headers, or no chunk within `idle_timeout` |
| `dns` | yes | name resolution failed |
| `http` | 408, 425, 500, 502–504 only | any other status, too many redirects, a bad redirect |
| `rate_limited` | yes | 429 (`Retry-After` preferred) |
| `content_length_mismatch` | yes | short or long body |
| `tls` | no | certificate or handshake failure |
| `not_found`, `unauthorized`, `forbidden` | no | 404/410, 401, 403 |
| `range_invalid` | no | a 206 that does not match the request, a 416 to a request from zero |
| `range_unsupported` | — | never raised: a 200 to a Range request restarts from zero (§5) |
| `disk_full` | pauses instead | `ENOSPC` |
| `permission_denied`, `io` | no | local filesystem |
| `validation` | no | empty body, over the cap, an enclosure URL without a host |
| `policy_blocked` | no | the SSRF policy refused the URL or a redirect |
| `cancelled` | no | a user command or shutdown |
| `storage` | no | the database write failed |

Schedule: `min(backoff_base × 2^(attempt−1), backoff_max)` with jitter, `Retry-After` used when it is not longer than the cap. Defaults: base 30 s, cap 6 h, 8 attempts. `next_attempt_at` is persisted, so a restart never produces a retry storm — the scheduler waits out the stored time.

## 10. Cancellation, pause and shutdown

Each running job holds a cancellation token that is a child of the service's token, plus a stop reason. `cancel` and `pause` write the final state first, so the worker's own write loses. `pause-all` and a disk-full pause stop the running jobs and let each one persist `paused(paused_all)` or `paused(disk_full)`. A shutdown sets `shutdown` on every handle, stops the scheduler *before* cancelling (so no job is claimed on the way out), and each worker parks its job as `queued(shutdown)`; jobs that outlive the grace period are abandoned and recovered at the next start. `uguisu serve` and `uguisu download run` wire Ctrl-C, SIGTERM on Unix and Ctrl-Break on Windows to that path.

## 11. Startup reconciliation

`Engine::open` runs a shallow reconcile before anything else can claim:

| Found | Action |
|---|---|
| `downloading` | → `queued(recovered)`, open attempts closed as `interrupted` |
| `finalizing`, a file of the recorded size at the target | → `completed`; a `.part` still there (a crash between the move's link and its unlink) is a second name for the same bytes, reported as a leftover and kept |
| `finalizing`, anything else at the target | → `failed(target_exists)`, counted as a finalization conflict; that file and the `.part` both stay |
| `finalizing`, `.part` exists | finalization is redone; if the rename fails again, → `failed(target_exists)` when the name is taken, else `failed(finalization)` with the error, counted as a failed finalization, and the `.part` stays |
| `finalizing`, neither | → `failed(finalization_lost)`; a user retry starts it over |
| `retrying`, `paused`, a global pause | untouched (a persisted pause is a decision, not damage) |
| a `.part` whose name is no job id | reported as an orphan, never deleted |

`download reconcile --deep` (and `POST /api/v1/downloads/reconcile?deep=true`) additionally stats every completed target: a missing file is counted and the episode's `archive_state` becomes `missing` — checking the bytes is the archive's verification ([`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §3) — and a pending job whose target already exists is reported.

## 12. Events

Every payload but those of `download.paused_all` and `download.resumed_all` carries `job_id`; the envelope carries `podcast_id` and `episode_id` as usual (ADR 0010).

| Kind | Payload |
|---|---|
| `download.queued` | `enclosure_url`, `priority`, `requeued` |
| `download.started` | `attempt`, `resumed_from`, `total_bytes`, `url` |
| `download.progress` | `bytes_downloaded`, `total_bytes`, `percentage`, `speed_bps`, `eta_secs` |
| `download.paused` | `reason`, `bytes_downloaded` |
| `download.resumed` | — |
| `download.retry_scheduled` | `attempt`, `error_kind`, `detail`, `http_status`, `next_attempt_at` |
| `download.completed` | `path`, `size_bytes`, `hash_algo`, `hash_value`, `content_type`, `sniffed_type`, `attempts`, `duration_ms` |
| `download.failed` | `reason`, `error_kind`, `detail`, `http_status`, `attempts` |
| `download.cancelled` | `bytes_downloaded` |
| `download.paused_all` / `download.resumed_all` | `reason` / — |

`download.progress` is **transient**: it is published on the bus and never written to the `events` table (`EventKind::is_transient`). It is throttled to at most one per job per `progress_interval`, and only when at least 1 MiB or 1 % has moved; `speed_bps` is an exponential moving average over one-second samples and `eta_secs` follows it. Consumers that do not want it ask for `GET /api/v1/events?exclude=download.progress`.

Failure details never contain headers or the request's query string; the URL appears only in `download.queued` and `download.started`, where it is the user's own input.

## 13. API

| Route | Purpose |
|---|---|
| `POST /api/v1/downloads` | enqueue one episode (`201` created, `200` otherwise) |
| `POST /api/v1/podcasts/{id}/downloads` | enqueue a whole podcast |
| `GET /api/v1/downloads?state=&podcast=&after=&limit=` | list jobs, newest first, keyset paging |
| `GET /api/v1/downloads/stats` | counts per state, running workers, global pause, next retry |
| `GET /api/v1/downloads/{id}` | one job with attempts and live progress |
| `POST /api/v1/downloads/{id}/{cancel\|pause\|resume\|retry}` | job commands (`409` when the state forbids it) |
| `POST /api/v1/downloads/retry-failed` | re-queue every failed job, except one whose episode an import or a rebuild archived, while that file is in place |
| `POST /api/v1/downloads/{pause\|resume}` | global pause and resume |
| `POST /api/v1/downloads/reconcile?deep=` | re-run reconciliation |

## 14. CLI

`uguisu download <episode-id> [--priority low|normal|high] [--wait]`, `download podcast <id>`, `download list|show|cancel|pause|resume|retry|retry-failed|pause-all|resume-all|stats|reconcile [--deep]|run [--until-idle]`. Full description and exit codes in [`CLI.md`](CLI.md); 10 means a download failed for a reason other than the network (8) or the policy (6), 11 means the disk is full.

## 15. Configuration

| Variable | Default | Effect |
|---|---|---|
| `UGUISU_MEDIA_DIR` | `<data dir>/media` | root of the media tree |
| `UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY` | `3` | running jobs in this process |
| `UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY` | `1` | running jobs per `scheme://host:port` |
| `UGUISU_DOWNLOAD_MAX_ATTEMPTS` | `8` | attempts before `failed(max_attempts)` |
| `UGUISU_DOWNLOAD_BACKOFF_BASE_MS` / `_MAX_MS` | `30000` / `21600000` | retry backoff base and cap |
| `UGUISU_DOWNLOAD_IDLE_TIMEOUT_MS` | `60000` | no chunk for this long is a timeout |
| `UGUISU_DOWNLOAD_PROGRESS_INTERVAL_MS` | `1000` | floor between progress writes per job |
| `UGUISU_DOWNLOAD_MAX_BYTES` | `8589934592` (8 GiB) | largest accepted file |
| `UGUISU_DOWNLOAD_MIN_FREE_BYTES` | `268435456` (256 MiB) | space that must remain free |
| `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS` | `15000` | how long a stop waits for running jobs |

`per_host_concurrency` must not exceed `global_concurrency`, `max_attempts` must be at least 1, the backoff cap must not be below its base and `max_bytes` must be at least 1 MiB; a value that breaks one of these is a configuration error.

## 16. Known limitations (Phase 4)

- **Automatic enqueue is off by default.** `episode.discovered` means "this exists"; a refresh queues an episode only when the archive policy is turned on ([`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §9, ADR 0023).
- **Deep reconcile only reports.** A missing file is counted and its episode marked `missing`; checking the bytes is the archive's verification ([`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §3).
- **Workers are process-local.** One process owns the data directory (ADR 0016); an embedded command without `--wait`/`run` queues but does not download.
- **One stream per job.** No chunked parallel range downloads: the benchmark shows a single stream already saturates loopback, and multi-part fetching multiplies load on the publisher's host.
- **No bandwidth limit.** Politeness is expressed as concurrency, not bytes per second; a rate limiter would go next to the per-host throttle.
- **`Retry-After` dates** are honoured only as a delay, and a value beyond the backoff cap falls back to the backoff.

---

*Status: Phase 10, as built. Numbers in `docs/benchmarks/2026-09-18-phase4.md`.*
