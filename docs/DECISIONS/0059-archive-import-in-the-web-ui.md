# ADR 0059 — Importing an archive from the web UI: a server path, typed

**Status:** accepted, amends [ADR 0050](0050-podgrab-migration.md) · **Date:** 2026-10-04

## Context

An archive import reads a directory on the machine Uguisu runs on (ADR 0025, ADR 0050). Until now only the CLI and the API could start one, so a person who runs Uguisu as a server, or uses the desktop app, had to drop to a terminal for the step `MIGRATION.md` calls the most important. The web UI has never asked the user for a path on the server. Everything it showed came from the API, and `docs/WEB_UI.md` says no filesystem path is constructed in the browser.

## Decision

**A page at `/archive/import` asks for the folder as text and sends it, unchanged, in the JSON body of `POST /api/v1/archive/import`.** The page offers the same choices the CLI has: layout (detect, Podgrab, generic), Podgrab's database (also typed), and one podcast. The threshold stays the configured default. It runs a dry run first, shows the plan with a filter per outcome, and applies the same request with `apply: true` only when asked. Nothing new is added to the API.

**The request stays one synchronous call.** The page waits without a client deadline and counts `archive.imported` events while a copy runs. If the answer is lost, say because a proxy cut the connection, the page says that running it again continues. It does not wait for `archive.import.completed`, because an interrupted handler never sends it. A second run while one is going is blocked in the page; the engine already keeps two runs from overwriting anything (ADR 0050).

**What the page does not do:**
- **No browsing.** There is no endpoint that lists the server's directories. It would map the server's filesystem for anyone holding a credential, and it is not needed to type a path the operator knows.
- **No upload.** `<input type="file" webkitdirectory>` reads the client's disk, not the server's, and the body limit is 2 MB.
- **No path in a URL.** The path travels only in the POST body, so it reaches no proxy log or browser history.

The route is a mutation like any other, so it needs a write credential once a password is set, and ADR 0058 refuses it from another site. A failure shows the server's message, which echoes the path the operator typed.

## Consequences

- In the Flatpak a typed host path does not work: the sandbox has no filesystem permission (ADR 0044), so the import fails with `import_source_invalid`. The page and `MIGRATION.md` say to use the CLI outside the sandbox there. A portal picker for this is not built.
- `docs/WEB_UI.md`'s rule becomes: the browser builds no path from its parts. It may carry one the operator typed.
- A large import over a proxy still meets the proxy's read timeout, as with the CLI (`docs/DEPLOYMENT.md`).

## Alternatives considered

- **A background job with progress events.** The engine's import is one call that finishes what it can and converges on a re-run. A job table, its states and its recovery would duplicate that for one page. Rejected for the same reason ADR 0049 kept the OPML import synchronous.
- **A native folder picker in the desktop app.** It would help the desktop only, adds an IPC command to the shell's narrow surface (ADR 0041), and is still no help in the Flatpak without the portal. Left for later.
