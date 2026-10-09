# ADR 0023 — The automatic archive policy

**Status:** accepted · **Date:** 2026-09-18 · relates to [ADR 0010](0010-event-system.md), [ADR 0018](0018-download-state-machine-and-queue.md)

## Context

Through Phase 4, downloading is entirely manual: discovering an episode emits `episode.discovered` and someone has to ask for it. That is the right default for a tool that fills a disk, but it is not what a self-hosted archiver is for. Phase 5 adds a rule that decides on its own, and the risk it has to manage is obvious: an archiver that starts downloading without being asked can fill a disk overnight.

## Decision

### Off by default, opt-in in both directions

`UGUISU_ARCHIVE_AUTO_DOWNLOAD` defaults to `false`. With the shipped configuration, discovering an episode queues nothing, which a test asserts directly. A per-podcast policy can opt one podcast in on an otherwise manual installation, or hold one back on an otherwise automatic one: the podcast's `mode` always wins, while every other field falls back to the global default when unset.

### A pure function, called by the engine

`decide(policy, episode, position, now) -> Queue { priority } | Skip { reason }`: no database, no filesystem, no queue. The engine asks, then calls the same `DownloadService::enqueue_episode` a user command uses.

`uguisu-download` therefore never learns what a policy is, and the unique `download_jobs.episode_id` stays the real idempotency boundary — three refreshes of one feed produce one job per episode, regardless of what the policy decides each time.

### Manual always wins

A user command does not go through the policy at all. An episode the policy passed over can still be downloaded by name, and a policy at its backlog limit does not block `download podcast`.

### It runs after the refresh transaction, and cannot fail it

The policy is evaluated once the refresh has committed. Its failures are logged, not propagated: the feed is stored either way, and a queue that refuses must not cost the user the fetch.

### `max_backlog` counts outstanding work

It is the number of the podcast's episodes already queued, retrying, downloading or finalizing — not the number this evaluation queued. Counting per evaluation would let every refresh top the queue back up to the limit, so the limit would mean nothing over time.

### Every skip names a reason

From a closed vocabulary: `disabled`, `already_archived`, `already_queued`, `no_enclosure`, `duplicate`, `skipped`, `too_old`, `backlog_exceeded`, `removed_from_feed`. `archive.policy_skipped` is emitted for all of them **except** `disabled`, which on a default installation would fire for every episode of every refresh — noise, not information.

### An undated episode is undated, not ancient

An age limit ignores an episode with no `published_at` rather than excluding it, so a feed that omits dates does not silently archive nothing. `sort_at` is deliberately not used as a substitute: for such a feed it is the time Uguisu first saw the item, which would make the answer depend on when the podcast was added.

## Consequences

- The dangerous default is the safe one, and turning it on is a deliberate act with a visible backlog limit.
- The decision is table-testable, and its reasons are machine-readable rather than prose a client has to parse.
- Because the queue is the idempotency boundary, the policy can be re-run freely — on every refresh, after a restart, or by hand — without creating duplicates.
- A policy that wants to be cleverer later (bandwidth windows, per-episode rules, retention) extends one pure function and its table, not the queue.
- Phase 5 has no scheduler, so the policy only runs when something refreshes a feed. Automatic refreshing is Phase 7; until then, automatic archiving follows manual refreshes.

## Alternatives considered

- **On by default with a small backlog.** Friendlier first run, but it downloads without consent on a machine whose disk Uguisu does not own. Rejected.
- **Policy inside `DownloadService`.** Would put feed-shaped rules in the queue and make the queue's API depend on episode semantics. Rejected; the queue stays a queue.
- **Deciding on the `episode.discovered` event asynchronously.** Tempting, but a dropped or lagged event would silently skip episodes, and the refresh already knows exactly which episodes are new.
- **Counting the backlog per evaluation.** Simpler, and wrong: repeated refreshes defeat the limit.
- **A quota in bytes rather than a count of episodes.** More precise about disk, but the size is unknown before the download and feed-declared lengths are unreliable. A count is honest about what it controls.
