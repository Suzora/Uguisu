# Verification — Phase 9 in a real browser (2026-09-22)

`scripts/e2e.py` proves the HTTP contract, including the whole authenticated walk, and runs in CI; the frontend suite proves each view against a stubbed API. Neither drives a browser, because the repository still adds no browser-automation dependency (§8 of the phase brief). This file records the browser pass that was run by hand instead, so "you can log in and play an episode" has something behind it.

## How it was run

A one-off Playwright script against the container's pre-installed Chromium 1194, pointed at the **debug** binary serving `web/dist` over loopback, with a local publisher standing in for the feeds. The fixture is seeded through the public API (one podcast, two episodes, both downloaded, archived and artworked), then a password is set with the server stopped and the server restarted — the same order an operator follows.

The script is not in the repository: reproducing it needs Playwright, which is not a project dependency. What it asserted is below.

## What passed

| Step | Asserted |
|---|---|
| the gate | the login form is shown when a credential is needed |
| the gate holds | the navigation is not in the DOM behind it — not hidden, absent |
| a wrong password | refused, with the server's own message, staying on the form |
| the right password | the app opens |
| the cookie | `HttpOnly: true`, `SameSite: Lax`, as read back through CDP |
| library | the seeded podcast is listed |
| podcast page | its heading and its episodes render |
| artwork | the `img` source is `/api/v1/podcasts/{id}/artwork/image`, and it answers 200 |
| playback | `<audio>` mounts with `/api/v1/archive/{id}/media` |
| ranges | a range request from the page returns **206**, exactly 100 bytes, `Content-Range: bytes 0-99/4044` |
| the queue | the finished jobs are listed |
| the event stream | the header's indicator reaches `open` — with nothing but a cookie |
| a mutation | pausing and resuming the scheduler from the UI both succeed, so the CSRF header is being sent |
| settings | the password panel is offered |
| signing out | returns to the login form |
| images | no image on the page is blocked or broken |
| **no bearer** | **not one request in the whole session carried an `Authorization` header** |
| **no credential in a URL** | no request URL contained the password or a `token`/`password`/`secret` parameter |

18 of 18. The console shows the six 401s of the anonymous boot before the login (which is the shell asking what it may see) and one 404 for the favicon, which the build does not ship.

## What the browser pass found that the other suites had not

**The Content-Security-Policy blocked a podcast's own artwork.** `img-src 'self' data:` is correct for everything the UI serves itself, and wrong for the one thing it deliberately does not: a podcast whose artwork Uguisu has not fetched falls back to the URL its feed published (`docs/WEB_UI.md`), and **artwork fetching is off by default** — so a default installation would have shown no artwork at all, with a CSP violation per podcast in the console. Neither the HTTP e2e nor jsdom enforces a CSP, so only a real browser could have found it.

`img-src` is now `* data:`, and it is the only open directive: `script-src`, `style-src`, `connect-src`, `media-src`, `font-src` and `form-action` stay `'self'`, with `object-src 'none'`, `base-uri 'none'` and `frame-ancestors 'none'`. An image cannot execute, and the privacy cost is the one the fallback already had. `crates/uguisu-server/tests/web.rs` now asserts both halves of that.
