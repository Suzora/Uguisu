# ADR 0018 — Download state machine and persistent queue

**Status:** accepted, amended by [ADR 0060](0060-repairing-a-missing-file.md) · **Date:** 2026-09-18 · amends [ADR 0002](0002-sqlite-storage.md)

## Context

Downloads outlive the process that starts them. A user cancels while bytes are moving, a container is stopped mid-transfer, a host answers 503 for an hour, a disk fills up. The queue therefore has to be persistent, the transitions have to be unambiguous, and two things must never happen: a job downloaded twice, and a job silently stuck.

Phase 3 established the constraints this has to live with: one SQLite writer with `BEGIN IMMEDIATE`, one process per data directory (ADR 0016), events written inside the producing transaction and published after commit.

## Decision

- **The database is the queue.** `download_jobs` holds one row per episode (`episode_id UNIQUE`), `download_attempts` the history, `download_control` a single row for the global pause. There is no in-memory queue to lose.
- **Eight persisted states** — `queued`, `downloading`, `finalizing`, `retrying`, `paused`, `completed`, `failed`, `cancelled` — plus a closed `state_reason` vocabulary. `finalizing` is persisted because its recovery differs from every other state: the rename may already have happened. The brief's `Preparing`/`Validating` are steps inside `downloading` (logged, no distinct recovery), `Interrupted` is `queued(recovered|shutdown)`, `Skipped` is an enqueue refusal.
- **A pure transition table.** `transition(state, event)` in `uguisu-download::state` decides every move before it is written; a property test checks the table against the persisted transitions and that `completed` has no outgoing edge. *(Amended by ADR 0060: one edge, `completed → queued` on a redownload of a file that is gone; nothing else leaves `completed`.)*
- **Compare-and-set everywhere.** Every worker write is `UPDATE … WHERE id = ? AND state IN (expected)`. User commands write the database first and cancel the job's token second, so a command can never be overwritten by the worker that was running when it arrived.
- **Admission.** One scheduler task, a `JoinSet` bounded by `global_concurrency`, a synchronous per-host permit taken at claim time, and a claim query that excludes saturated hosts. A worker never awaits a host permit while holding a global slot.
- **The attempt log numbers itself.** `attempt_count` is the retry budget and a user retry resets it; `download_attempts.attempt_no` is `MAX + 1` for the job, so the history stays unique and monotonic across retries.
- **Workers are explicit.** Opening the engine reconciles but starts nothing; `serve`, `download run` and `--wait` start the workers.

## Consequences

- A queue survives kill -9, a full disk and a restart with no bespoke recovery code per failure: reconciliation is one pass over three state groups.
- Priority and FIFO order are a single indexed query (`state, priority DESC, created_at, id`); the first claim out of 1 000 queued jobs takes 1.6 ms.
- Enqueue costs one small transaction per episode (≈ 0.7 ms), so queueing a back catalogue is a visible but bounded operation.
- The single-writer discipline holds: a running job writes a handful of rows plus one throttled progress update per second.
- Two processes cannot share the queue. That is the ADR 0016 lock, not a downloader decision, and a durable multi-process queue would be a new ADR.

## Alternatives considered

- **An in-memory queue rebuilt from the database at startup:** the same rows, but state lives in two places and a crash between them is a bug per state rather than one reconcile.
- **A job state machine without `finalizing`:** simpler table, but a crash after the rename and before the completion write is then indistinguishable from an interrupted body, and the file would be downloaded again.
- **Per-job rows for the pause flag:** a global pause that has to touch every row is neither atomic nor cheap; one control row is both.
- **Optimistic concurrency with a version column:** equivalent to the state compare-and-set here, with an extra column and no extra safety.
