# ADR 0033 — One event stream per tab, and what a reconnect means

**Status:** accepted · **Date:** 2026-09-21 · relates to [ADR 0010](0010-event-system.md), [ADR 0016](0016-refresh-coalescing-lock-and-post-commit-events.md)

## Context

`GET /api/v1/events` streams the 49 event kinds as Server-Sent Events. The web UI has nine views, most of which care about some of them: the queue needs `download.progress`, the library needs `podcast.*`, the archive needs `archive.*`. The obvious implementation — each view opens the stream it needs — gives a browser five connections to the same endpoint, five reconnect loops and five copies of every message.

Two properties of the stream make this more than a plumbing question. The server sends an `id:` line but **no `retry:` directive and no `Last-Event-ID` handling**, so reconnection is entirely the client's problem. And `download.progress` is published on the bus but never stored, so it is not in `?after=` replay: an event missed while the stream was down is missed for good, and nothing in the payload reveals that a gap happened.

## Decision

**One `EventStream` for the application, owned by the shell.** Views call `subscribe(listener)` and get an unsubscribe function; the connection is opened once when the shell mounts and closed when it unmounts. `start()` is idempotent, so a remount cannot double it. No view constructs an `EventSource`.

**Reconnect with exponential backoff**, `500 ms · 2^n` capped at 30 s, reset on a successful open. The UI shows three states — connecting, live, reconnecting — because a stream that is quietly dead looks exactly like a library where nothing is happening.

**Every successful open runs the resync callbacks, including the first one.** A view registers `onResync` to re-read the API it depends on. This is the decision that matters: the stream is an optimisation, never the source of truth, and because a progress gap is invisible the only correct response to *any* open is to ask the server again. The queue view additionally throws away its live progress overlay on resync, so a paused or finished job cannot keep a stale speed on screen.

**Events are an overlay on API data, never a substitute.** A `download.progress` message updates the bytes on a row the API supplied; it never creates a row. A state-changing event (`download.completed`, `archive.verified`, `podcast.added`) triggers a re-read rather than a local mutation. Nothing in the browser reconstructs a record from a sequence of events.

**Every kind is subscribed, and filtering is the view's job.** SSE dispatches by event name, so the client registers a listener per name from an explicit list of the 49 — a name missing from that list is an event the UI never sees, which is worth having in one visible place. `?exclude=` stays available for a client that wants less, and the UI uses it for nothing: dropping `download.progress` at the server would blind the queue.

## Consequences

- A tab holds exactly one stream, whatever is on screen, and the count is asserted by a test.
- A reconnect costs a burst of API reads. That is the price of correctness, and it happens only when the connection actually broke.
- A view that forgets `onResync` degrades to stale-until-navigated rather than wrong-forever; the reconnect test is what catches it.
- Replay through `?after=<id>` is not used. The stream's job is liveness, and the authoritative re-read is both simpler and complete, where a replay would still be missing every transient event.

## Alternatives considered

- **A stream per view.** Five connections, five reconnect loops, duplicated delivery, and each view inventing its own gap handling.
- **Client-side event sourcing.** Replay `?after=` from the last id and apply events to a local store. Rejected: `download.progress` is never stored, so the local copy is wrong in exactly the case the UI is watching, and it puts a second implementation of the state machines in the browser.
- **Polling instead of SSE.** Five views at one second each is worse on every axis, and the infrastructure already exists.
- **Asking the server for `retry:` and `Last-Event-ID`.** Would help a client that wants replay; it changes nothing for a client that re-reads authoritative state on every open, which it must do anyway.
