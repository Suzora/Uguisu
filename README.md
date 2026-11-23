<p align="center">
  <img src="crates/uguisu/assets/logo.webp" alt="Uguisu logo: a singing bush warbler on a branch" width="280">
</p>

# Uguisu

Uguisu is a self-hosted **podcast archiver**. It downloads every episode of the podcasts you add,
keeps them verifiably intact, and publishes its archive as ordinary RSS feeds – so the podcast app
you already use (AntennaPod, PinePods, Apple Podcasts, …) plays from your archive.

Uguisu is not a player and does not sync listening progress. That is the job of your podcast app.

> Status: early development (0.3). The archive format may still change before 1.0.

## What it does

- **Archives for the long term.** Every distinct version of a source feed is stored byte for byte.
  Feeds that are split into pages (RFC 5005) are read to the last page.
  Episodes that disappear from a feed stay in the archive and are marked, not deleted. Changes to
  titles and show notes are kept as revisions together with the raw `<item>` XML.
- **Verifies its files.** Each media file is hashed with BLAKE3 while it is downloaded; a
  download that breaks off is continued where it stopped. In the
  background every file is re-hashed at least once in 30 days; the library shows whether the
  archive is intact. `uguisu verify` checks everything at once. Damaged or missing files are
  reported and queued for a new download.
- **Keeps replaced audio.** Optionally per podcast: when the source replaces an episode's media
  file, the new one is archived next to the earlier copies (`media.v2.mp3`, …) and the newest is
  published. A file that only moved to a new address is not stored twice.
- **Publishes the archive as feeds.** One feed per podcast behind a secret address. Episodes are
  published as the original feed entries – show notes, chapters, transcripts and other
  Podcasting 2.0 tags included – with the media address pointing at the archive. Range requests
  (seeking) are supported. Adding `?limit=50` to a feed address publishes only the newest 50
  episodes, for apps that struggle with very long feeds.
- **Finds things again.** Titles, show notes and transcripts of the whole archive can be
  searched, and every episode has a page showing its files, checksums and what the publisher
  changed over time.
- **Offers one feed for everything.** Besides the feed per podcast there is one feed with the
  newest episodes of all podcasts.
- **Archives what an episode links to.** Transcripts, chapter files, episode images and the
  images shown in the show notes are stored next to the media file, checked like it, and the
  published feed points at the archived copies.
- **Archives everything, unless told otherwise.** Per podcast a start date can be set: earlier
  episodes are listed, but their files are not downloaded.
- **Is a polite guest.** Requests to the same server are at least a second apart, and a server
  that asks for a break (`Retry-After`) gets it, without the wait counting as a failure.
- **Takes input the easy way.** Search podcast directories (iTunes and fyyd), paste a feed
  address or simply the address of the podcast's web page, or import an OPML file from your
  current app. Source feeds may be RSS 2.0, Atom or
  RSS 1.0; the published feed is always RSS 2.0.
- **Keeps the archive readable without Uguisu.** The database is only an index. Everything needed
  to understand the archive is on disk as plain files, and the database can be rebuilt from them.
- **Speaks English and German.** The language of the web interface is chosen in the settings.
- **Says when something is wrong.** Damaged files and feeds that keep failing can be reported to
  a webhook.

## Archive layout

```text
data/
  uguisu.db                              index (SQLite)
  archive/
    <podcast>-<hash>/
      podcast.json                       podcast metadata
      cover.jpg
      feeds/<timestamp>-<hash>.xml       every version of the source feed, unmodified
      episodes/<date>-<title>-<hash>/
        episode.json                     episode metadata
        item.xml                         raw <item> element from the feed
        media.mp3                        the episode
        media.mp3.blake3                 checksum, usable with `b3sum -c`
        transcript-<hash>.vtt            files the episode links to: transcripts,
        chapters-<hash>.json             chapters, its own image and the images
        notes-image-<hash>.png           in its show notes
```

The format is described in [docs/ARCHIVE.md](docs/ARCHIVE.md).

## Running with Docker

```bash
docker compose up -d --build
```

Then open <http://localhost:8484> and choose a password. The container runs as an unprivileged
user (uid 65532) and needs to write to the mounted directory. For the bind mount in `compose.yaml`
create it before the first start:

```bash
mkdir -p data && sudo chown 65532:65532 data
```

Alternatively set `user:` in `compose.yaml` to your own uid and gid; the file shows how. The image
has a health check, so `docker ps` shows whether the instance answers.

Each release also publishes the image as `ghcr.io/suzora/uguisu:<version>`; to use it, replace
`build: .` in `compose.yaml` with `image: ghcr.io/suzora/uguisu:<version>`.

[docs/OPERATIONS.md](docs/OPERATIONS.md) covers HTTPS, backup and restore, moving and upgrading.
[docs/FAQ.md](docs/FAQ.md) answers common questions, including what to mind legally.

Set `UGUISU_BASE_URL` to the address your podcast app will use to reach Uguisu. Put a reverse
proxy with HTTPS in front of it if the instance is reachable from outside your network.

## Building from source

Docker is the supported way to run Uguisu. For development you need Linux or macOS (on Windows:
WSL) and Rust 1.88 or newer:

```bash
cargo run -p uguisu
```

## Configuration

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--data-dir` | `UGUISU_DATA_DIR` | `./data` | Database and archive |
| `--listen` | `UGUISU_LISTEN` | `0.0.0.0:8484` | Address of the web interface |
| `--base-url` | `UGUISU_BASE_URL` | derived from the request | Public address written into feeds |
| `--refresh-minutes` | `UGUISU_REFRESH_MINUTES` | `60` | Interval between checks of each source feed |
| `--download-concurrency` | `UGUISU_DOWNLOAD_CONCURRENCY` | `2` | Simultaneous downloads |
| `--verify-days` | `UGUISU_VERIFY_DAYS` | `30` | Re-hash every file at least this often; `0` turns it off |
| `--min-free-gb` | `UGUISU_MIN_FREE_GB` | `1` | Downloads wait while less than this is free on the disk; `0` turns it off |
| `--backups` | `UGUISU_BACKUPS` | `7` | Daily copies of the database kept in `backups/`; `0` turns them off |
| `--webhook-url` | `UGUISU_WEBHOOK_URL` | none | Address that is told about damaged files and failing feeds (see the [operations guide](docs/OPERATIONS.md)) |
| `--allow-private-networks` | `UGUISU_ALLOW_PRIVATE_NETWORKS` | off | Allow sources on local and private addresses |

Commands: `uguisu serve` (default), `uguisu verify`, `uguisu reindex`, `uguisu import` and
`uguisu reset-password`. `verify` exits with a
non-zero status if any file is damaged or missing, so it can be used from cron or a monitoring
job. `reindex` rebuilds a lost or damaged database from the archive; the published feeds keep
their addresses. Feed passwords and the login are not part of the archive and have to be entered
again.

### Importing files you already have

Episodes downloaded earlier by a podcast app can be taken into the archive:

```bash
uguisu import /path/to/files --feed https://example.org/feed.xml --keep-unmatched
```

A file is matched to an episode by the file name in the episode's media address, or else by the
episode's title in the file name. With `--keep-unmatched`, files that fit no episode of the feed –
typically episodes the feed no longer lists – become episodes of their own. Files are copied and
checked; the directory is left as it is. With Docker the directory has to be mounted into the
container, for example
`docker compose run --rm -v /path/to/files:/import:ro uguisu import /import --feed …`.

### The data directory

```text
data/
  uguisu.db        index of the archive
  archive/         the archive itself – this is what matters
  backups/         daily copies of the database
  cache/           reduced covers for the web interface; can be deleted at any time
```

### The database

`uguisu.db` is an index of the archive. Once a day Uguisu writes a complete copy of it to
`backups/` in the data directory and keeps the newest seven. To go back to a copy, stop Uguisu
and put the copy in the place of `uguisu.db`. Without any copy, `uguisu reindex` rebuilds the
database from the archive; only feed passwords and the login are then missing. The copies contain
feed passwords in plain text, like the database itself.

## Security notes

- The web interface has a single account, protected by a password (Argon2) and a session cookie.
  The password is changed under Settings, which also signs out other devices. If it is lost,
  `uguisu reset-password` (with Docker: `docker compose exec uguisu /usr/local/bin/uguisu reset-password`)
  removes it, and the next visit asks for a new one.
- Published feeds and their media need no login. They are protected by a random 192-bit token in
  the address, because most podcast apps cannot log in. Treat a feed address like a password; a new
  one can be generated per podcast.
- Uguisu refuses to fetch from loopback, private and link-local addresses unless
  `--allow-private-networks` is set. When an HTTP proxy is configured through the usual
  environment variables, host names are resolved by the proxy and this check only covers IP
  literals.
- A feed behind a password is added as `https://user:password@host/feed`. The credentials are
  also used for media files on the same server, never for another one. They are stored in the
  database in plain text and left out of the archive files, published feeds and log messages.
- Archive only what you are allowed to keep, and do not share feed addresses of content you have
  no right to redistribute. If someone wants the same archive, share Uguisu instead and let
  them archive the podcast themselves.

## Project layout

- `crates/uguisu-core` – the archive engine: feed parsing, on-disk archive, downloads, fixity
  checks, feed generation, search, OPML.
- `crates/uguisu` – the server binary: web interface, authentication, published feeds.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI additionally runs [`cargo deny check`](https://github.com/EmbarkStudios/cargo-deny)
(vulnerabilities, unmaintained crates and licenses of all dependencies, see `deny.toml`) and
[`typos`](https://github.com/crate-ci/typos) (spelling, see `_typos.toml`).

### Releasing

Set the version in `Cargo.toml`, then push a tag with the same version, for example `v0.2.0`. The
release workflow publishes the container image to `ghcr.io` and creates the GitHub release.
The image carries a provenance record and an SBOM (see the
[operations guide](docs/OPERATIONS.md)).
Starting the workflow by hand from the Actions page is a dry run that builds the image and
publishes nothing.

## Roadmap

[PLAN.md](PLAN.md) lists what is done, what comes next, and what Uguisu will not do.
[CHANGELOG.md](CHANGELOG.md) lists what changed from release to release, and
[CONTRIBUTING.md](CONTRIBUTING.md) says how to take part.

## License

Uguisu is free software under the [GNU Affero General Public License](LICENSE), version 3 or
(at your option) any later version. It bundles [htmx](https://htmx.org) (Zero-Clause BSD).
