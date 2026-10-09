# Deployment

How to put Uguisu somewhere other than your own laptop, and what Uguisu refuses to guess.

| For | Read |
|---|---|
| The API's own rules | [`API.md`](API.md) |
| Every setting and its default | [`CONFIGURATION.md`](CONFIGURATION.md) |
| Commands, flags and exit codes | [`CLI.md`](CLI.md) |
| The threat model | [`SECURITY.md`](SECURITY.md) |

The Docker image has its own page, [`DOCKER.md`](../DOCKER.md), for what is different in a container; everything here holds there too. The desktop application's installers and packages are in [`INSTALL.md`](../INSTALL.md); this page is about `uguisu serve`.

## Installing the server

The server is the `uguisu` binary and the built web UI. The desktop packages do not include the `uguisu` binary, and no release binary is published yet, so on a machine without Docker it is built from a checkout ([`DOCKER.md`](../DOCKER.md) builds the same two things into an image).

It needs the Rust toolchain `rust-toolchain.toml` names (rustup installs it on the first build), a C compiler for the bundled SQLite, and Node 22 or newer with Corepack, which fetches the pnpm `web/package.json` pins:

```bash
cargo build --release --locked -p uguisu-cli
corepack enable
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
```

Then install the binary and the web UI, and tell the server where the web UI is:

```bash
sudo install -m 755 target/release/uguisu /usr/local/bin/uguisu
sudo mkdir -p /usr/local/share/uguisu
sudo cp -r web/dist /usr/local/share/uguisu/web
```

`UGUISU_WEB_DIR=/usr/local/share/uguisu/web` (or `--web`) points `serve` at it; without it `serve` looks for `web/dist` under its working directory and, finding none, serves the API alone. A unit file for systemd is in [`SERVICE.md`](SERVICE.md) §7. On Windows the binary is `target\release\uguisu.exe`, and the same variables apply.

## Binding, and the refusal

`uguisu serve` binds `127.0.0.1:8484` unless told otherwise. Binding anything reachable from a network **without a password set is a startup failure** (ADR 0037):

```
$ uguisu serve --bind 0.0.0.0:8484
uguisu serve: refusing to bind 0.0.0.0:8484 without authentication
  set a credential first: uguisu auth set-password
  or bind to loopback: --bind 127.0.0.1:8484
  to override deliberately: UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1
```

Exit code 12. The check runs before the listener opens and before the download workers and scheduler start.

The intended order on a new host:

```
uguisu auth set-password              # reads the password from the terminal, or from stdin
uguisu serve --bind 0.0.0.0:8484      # now allowed
```

`UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1` (or `--allow-insecure-exposure`) starts anyway, for a host where something else — a firewall, a private network namespace — is the boundary. It logs one `WARN` naming the address, `auth = "disabled"` and `insecure_override = true`; a loopback start with no password logs a different `WARN`, about DNS rebinding ([SECURITY.md](SECURITY.md) §3.6). Both knobs take the same words as every `UGUISU_*` boolean: `1`, `true`, `yes`, `on`.

Loopback means loopback in either family: `127.0.0.0/8`, `::1` and `::ffff:127.0.0.1` all count.

## Behind a reverse proxy

Uguisu does not terminate TLS and has no plans to. Put a proxy in front, terminate there, and forward to the loopback port.

**nginx:**

```nginx
server {
    listen 443 ssl;
    server_name uguisu.example;

    ssl_certificate     /etc/ssl/uguisu.crt;
    ssl_certificate_key /etc/ssl/uguisu.key;

    # Archived episodes are large and the API streams them with ranges.
    client_max_body_size 4m;
    proxy_read_timeout   300s;

    location / {
        proxy_pass http://127.0.0.1:8484;
        proxy_http_version 1.1;
        proxy_set_header Host $http_host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;

        # The event stream is Server-Sent Events: no buffering, no gzip.
        proxy_buffering off;
        gzip off;
    }
}
```

**Caddy:**

```caddyfile
uguisu.example {
    reverse_proxy 127.0.0.1:8484 {
        flush_interval -1   # do not buffer the event stream
    }
}
```

Applying an OPML import answers only once every new feed is resolved, and an archive import only once every file is copied; either can take longer than the nginx `proxy_read_timeout` above. What was added by then stays added, and sending the same request again continues with the rest ([ADR 0049](DECISIONS/0049-opml-import-and-export.md), [ADR 0050](DECISIONS/0050-podgrab-migration.md)). For a large archive, run the import embedded with the service stopped.

The proxy must pass the browser's `Host` header on, as the nginx block does with `proxy_set_header Host $http_host` (not `$host`, which drops the port) and Caddy does by default: a change whose `Origin` does not match the `Host` the server sees is refused as coming from another site ([ADR 0058](DECISIONS/0058-refusing-cross-site-changes.md)).

With TLS in front, set `UGUISU_AUTH_COOKIE_SECURE=1`. Uguisu adds `Secure` to the session cookie only when this says so, and it will not infer TLS from a header — see below. Name the proxy with `--trusted-proxy` so each client has its own login bucket.

## What Uguisu trusts from a proxy

**`X-Forwarded-For`, from a proxy you name, and nothing else** ([ADR 0062](DECISIONS/0062-trusted-proxies.md)). `--trusted-proxy 127.0.0.1` (or `UGUISU_TRUSTED_PROXIES=127.0.0.1,10.0.0.0/8`) names the proxies, by address or network. From any other peer no header is read. From a named one, the `X-Forwarded-For` hops are read from the right and the first address that is not a named proxy is the client: the login limiter gives that client its own bucket (ten failures per five minutes), and the login log lines name it. With no proxy named, every request behind a proxy shares the proxy's bucket and only the global cap (a hundred failures per five minutes) bounds guessing.

Name only what can actually reach the port. A network that other hosts share lets each of them claim any address; `0.0.0.0/0` and `::/0` are refused for that reason. The desktop shell's session exchange always checks the socket, whatever a header says.

**No other forwarded header is read** — not the scheme, not the host. The cookie's `Secure` flag is yours to set: `UGUISU_AUTH_COOKIE_SECURE=1` is how a TLS-terminating proxy tells Uguisu the connection is encrypted, because Uguisu will not take a header's word for it.

## The web UI

`--web <dir>` serves a built SPA from a directory (ADR 0031); `pnpm --dir web build` writes `web/dist`. Hashed assets are served `immutable`, and any path that is not a file and has no extension falls through to `index.html` so a deep link survives a refresh. The shell itself is public — the login form has to load — and every `/api/` route behind it is not.

Responses carry `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`, and on HTML a Content-Security-Policy whose every executable directive is `'self'`, with `frame-ancestors 'none'`, `base-uri 'none'` and `object-src 'none'`. `img-src` is the one open directive, because a feed's own artwork URL is the fallback for a podcast whose artwork Uguisu has not fetched.

## Tokens for automation

A password is for a person at a keyboard. Everything else should hold a token:

```
uguisu auth token create ci --scope read     # prints the secret once
UGUISU_TOKEN=… uguisu --server https://uguisu.example podcast list
```

A `read` token cannot start a download, change a setting or pause the queue; it is refused with 403. Revoke with `uguisu auth token revoke <id>` — the row stays so the list can say it was revoked.

The first token needs the session cookie or filesystem access: `uguisu --server … auth token create` authenticates with a token it does not have yet, and `uguisu auth token create` run embedded needs the data directory, which a running server holds an exclusive lock on. Mint the first one either with the server stopped, or on the web UI's Settings page once signed in, which calls `POST /api/v1/auth/tokens` with the browser session.

## The desktop application

The desktop application ([`INSTALL.md`](../INSTALL.md)) embeds this same server, bound to `127.0.0.1` on a port the OS picks, always with authentication required. It never listens on a network interface, so nothing in this file applies to it. It reads the same data format: close it, then `UGUISU_DATA_DIR=<its data directory> uguisu serve` opens the same library. Only one process may hold a data directory at a time.

## Upgrading

Migrations run on open, in order, and a shipped one never changes. Stop the server, replace the binary, start it: the data directory carries everything, and an older binary against a newer database refuses to open rather than guessing.

## Backups

Three things, in this order of importance: the media root (the archive itself, and the only irreplaceable part), `uguisu.db` in the data directory (the index: if it is lost, add the podcasts again and refresh them, and `archive reconcile --rebuild` puts the archive records back from the sidecars), and the settings the environment supplies, which live wherever your unit file or shell profile keeps them. Back up the database with `uguisu db backup`, or `uguisu --server … db backup` while the server runs, which writes a consistent copy into the data directory's `backups/` ([ADR 0056](DECISIONS/0056-database-maintenance.md)) — copying a file that is being written to is how a WAL gets torn. A backup holds the password hash, the token digests and every feed URL as stored: keep it like the database. Nothing removes old backups.
