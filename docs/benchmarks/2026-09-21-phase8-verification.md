# Verification — Phase 8 web UI in a real browser (2026-09-21)

`scripts/e2e.py` proves the HTTP contract and runs in CI; the frontend suite proves each view's behaviour against a stubbed API. Neither drives a browser, because Phase 8 deliberately adds no browser-automation dependency to the repository (§34 of the phase brief). This file records the browser pass that was run by hand instead, so the claim "it works in a browser" has something behind it.

## How it was run

A one-off Playwright script against the container's pre-installed Chromium 1194, pointed at the debug binary serving `web/dist` over loopback, with a local publisher standing in for the feeds. The script is not in the repository: reproducing it needs Playwright, which is not a project dependency. What it asserted is below, and everything it covers is also covered by the two automated suites at a lower level.

```
node browser.mjs http://127.0.0.1:<port>
```

## What passed

| Step | Asserted |
|---|---|
| dashboard loads | `h1` and the library count painted |
| live stream connects | the header's indicator reaches `open` |
| library lists the podcast | the seeded show is a link |
| podcast detail opens | the podcast's `h1` and its first episode |
| artwork comes from Uguisu | the `img` source is `/api/v1/podcasts/{id}/artwork/image`, and it answers |
| episode plays | `<audio>` mounted; a range request from the page returns `206` with exactly 100 bytes |
| deep link survives a reload | a full navigation to `/podcasts/{id}` renders the podcast |
| downloads view | the finished jobs are listed as `completed` |
| archive view | the archived files are listed |
| local search | typing finds the seeded episode |
| search result navigates | the hit links to its podcast |
| settings shows the pinned value | `pinned by the environment`, and **no** editable control |
| service view pauses and resumes | the scheduler's buttons swap and swap back |
| back returns to settings | `history.back()` restores the previous view |
| mobile width | no horizontal overflow at 390×844 on six pages |
| keyboard | the first Tab stop is "Skip to content" |
| API loss and recovery | the offline banner appears with the API blocked and is gone after it returns |

18 of 18. The only console error in the run is the deliberate `net::ERR_FAILED` from the blocked-API step.

## What the browser pass found that the other suites had not

**A setting held only by the environment offered an editable control.** The API sets `pinned: true` only when a stored value *also* exists and something above it wins; a key simply exported in the environment arrives with `pinned: false`, `stored: null` and `origin: "env"`. The Settings view branched on `pinned`, so it rendered an input for exactly the case where a write would be refused with `409` — which §18 of the brief forbids in as many words. The view now branches on the origin, and three unit tests pin the three cases (environment, command line, environment overriding a stored value).

Two earlier failures in the same run were the script's own, not the UI's, and are recorded because they say something about testing this UI:

- `waitUntil: 'networkidle'` never settles, because the SSE connection is a request that stays open for the life of the page. Every navigation in a Phase-8 browser test has to wait on a condition, not on the network going quiet.
- The first fixture served an ID3 header followed by random bytes. The media endpoint served them correctly and Chromium refused to decode them — which exercised the player's failure path rather than its success path. `scripts/e2e.py` now serves a real WAV, so both the committed E2E and any browser run play genuine audio.

## What this does not establish

A person with a screen reader has not used this UI. The automated accessibility checks are structural — one `h1`, no skipped level, labels, names, table semantics, the page-change announcement — and the browser pass adds only the first Tab stop and the absence of horizontal overflow. Neither is a claim of conformance.
