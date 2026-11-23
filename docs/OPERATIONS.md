# Running Uguisu

This guide is for the person who keeps an Uguisu instance alive: where the data is, how to put
it behind HTTPS, how to back it up, move it and upgrade it.

## What lives where

Everything is in one data directory (`/data` in the container, `./data` with the provided
`compose.yaml`):

| Path | What it is | If it is lost |
| --- | --- | --- |
| `archive/` | The archive: feeds, episodes, media, checksums | Gone. This is what to back up. |
| `uguisu.db` | Index of the archive, login, feed passwords | Rebuilt with `uguisu reindex`; login and feed passwords have to be entered again |
| `backups/` | Daily copies of `uguisu.db` | Nothing, they are copies |
| `cache/` | Reduced covers for the web interface | Nothing, made again when needed |

The format of `archive/` is described in [ARCHIVE.md](ARCHIVE.md).

## HTTPS and a reverse proxy

Uguisu speaks plain HTTP on port 8484. If the instance is reachable from outside a network you
trust, put a reverse proxy with HTTPS in front of it and set `UGUISU_BASE_URL` to the public
address, for example `https://podcasts.example.org`. That address is written into the published
feeds, and with `https` the login cookie is only sent over encrypted connections.

Published feeds and media files need no login – podcast apps cannot log in – so the proxy must
not put its own password in front of `/feed/` and `/media/`. Media files are requested in ranges
when someone seeks; the proxy has to pass `Range` requests through, which the examples below do.

Caddy:

```text
podcasts.example.org {
    reverse_proxy localhost:8484
}
```

nginx (inside a `server` block that already handles TLS):

```nginx
location / {
    proxy_pass http://127.0.0.1:8484;
    proxy_set_header Host $http_host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_buffering off;
}
```

Both examples were run in front of an Uguisu instance (Caddy 2.11 and nginx 1.30 in Docker): login,
published feeds, media files and range requests work through them, with and without
`UGUISU_BASE_URL`. The nginx example passes `$http_host` rather than `$host` on purpose: `$host`
drops the port, so on any port other than 443 the feeds would point to the wrong address and
the login would be refused.

## Backup

Back up the data directory. What matters is `archive/`; including the rest makes a restore
quicker.

- The archive consists of ordinary files that are written once and then left alone, so any file
  backup tool works, also while Uguisu is running. A file that is still being downloaded has a
  name ending in `.part` and can be left out.
- Do not copy `uguisu.db` itself while Uguisu is running; a database copied in the middle of a
  write can be unusable. The copies in `backups/` are made for this: each is a complete database
  from one moment.
- After restoring, run `uguisu verify`. It compares every file with its checksum and tells you
  whether the backup was complete.

To check a backup without Uguisu at all, use the checksum files next to the media files:

```bash
find archive -name '*.blake3' -execdir b3sum -c {} \;
```

## Restore

1. Put the data directory back.
2. If `uguisu.db` is missing or does not open, take the newest file from `backups/` and put it in
   its place as `uguisu.db`.
3. If there is no usable copy either, start from the archive alone: `uguisu reindex` rebuilds the
   database. The published feeds keep their addresses. The login is set anew at the first visit,
   and passwords of protected feeds have to be entered again.
4. Run `uguisu verify`.

With Docker, commands other than the server are run like this (stop the service first for
`reindex`):

```bash
docker compose run --rm uguisu reindex
```

```bash
docker compose run --rm uguisu verify
```

## Moving to another machine

Stop Uguisu, copy the data directory, start Uguisu on the new machine with the same
`UGUISU_BASE_URL`. If the public address changes, set the new one: the feed addresses change
with it, and the podcast apps have to be given the new addresses (the OPML export on the library
page lists them all).

The container runs as user 65532. After copying, the data directory has to belong to that user
again:

```bash
sudo chown -R 65532:65532 data
```

## Stopping

`docker stop` is safe at any moment. Uguisu stops within a few seconds:

- Requests that are being answered get three seconds to finish; a podcast app in the middle of
  a large file is cut off and asks again later.
- A download that is under way stays on disk as `download.part` and is continued from there at
  the next start, as long as the source still offers the same file.
- A verification that is under way is abandoned and done again when it is next due.

The same holds when the process is killed or the machine loses power: files are written under
another name and moved into place only when they are complete, and the database is a SQLite
database in WAL mode.

## Checking what an image contains and where it comes from

Released images carry two attachments in the registry:

- a **provenance** record: the repository, the commit and the workflow that built the image,
  and the steps of the build;
- an **SBOM** (software bill of materials): the packages in the image and, from version 0.5
  on, the Rust libraries the program is built from. (In 0.4.0 the SBOM lists only the
  packages of the base image.)

Both can be read without pulling the image:

```sh
docker buildx imagetools inspect ghcr.io/suzora/uguisu:<version> --format '{{ json .Provenance }}'
docker buildx imagetools inspect ghcr.io/suzora/uguisu:<version> --format '{{ json .SBOM }}'
```

The list of Rust libraries is also written into the program itself, so it can be read from
any copy of it, for example with [`cargo audit bin`](https://crates.io/crates/cargo-audit).

The records are made by the build itself and stored next to the image. They are not signed:
they tell you what the registry holds, and are as trustworthy as the registry and the
project's GitHub account.

## Upgrading

Pull or build the new image and restart. The database is migrated at the start, and an archive
in an older format is brought up to date in the background. Going back to an older version after
that is not supported: an older Uguisu refuses an archive in a newer format rather than risk
damaging it. Keep a backup from before the upgrade if you want a way back.

## Keeping an eye on it

- The **Status** page of the web interface shows failed downloads, feeds that could not be
  checked, damaged files and free disk space.
- `docker ps` shows whether the instance answers (health check).
- `uguisu verify` exits with a non-zero status if any file is damaged or missing, so it can be
  run from cron and its result mailed or monitored.
- Downloads stop while less than `UGUISU_MIN_FREE_GB` (1 by default) is free on the disk.

### Notifications

Set `UGUISU_WEBHOOK_URL` to an address, and Uguisu sends it a JSON document by HTTP POST when
something needs attention. Any service that accepts such a request can turn it into a message:
ntfy, Gotify, Home Assistant, a chat webhook behind a small adapter, or a script of your own.

| `event` | When | Further fields |
| --- | --- | --- |
| `archive.damaged` | A check found damaged or missing files | `damaged`, `missing` (counts), `damaged_files`, `missing_files` (at most 50 names each) |
| `feed.failing` | Fetching a feed failed three times in a row; sent once | `podcast` (`id`, `title`, `feed`), `error` |
| `feed.recovered` | A feed reported as failing works again | `podcast` |
| `test` | "Send a test notification" on the settings page | none |

Every document also has `time` (UTC), `software` and `text`, a sentence a person can read:

```json
{
  "event": "feed.failing",
  "time": "2026-11-21T12:00:00Z",
  "software": "uguisu 0.4.0",
  "text": "The feed of \"Example\" has failed 3 times in a row: the feed is not valid XML",
  "podcast": { "id": 4, "title": "Example", "feed": "https://example.org/feed.xml" },
  "error": "the feed is not valid XML"
}
```

Good to know:

- The address is yours to choose and may point into your own network; the protection against
  private addresses that applies to feeds does not apply to it. It may contain a secret and
  is never shown or logged; the settings page names only its host.
- A notification is tried once, with ten seconds to answer, and redirects are not followed.
  If the receiver is down, the failure is logged and the notification is lost. The status page
  still shows the problem.
- Any answer with a 2xx status counts as delivered.

## Lost password

```bash
docker compose exec uguisu /usr/local/bin/uguisu reset-password
```

The next visit to the web interface asks for a new password.
