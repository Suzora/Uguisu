# Uguisu

Uguisu is a self-hosted **podcast archiver**. It downloads every episode of the podcasts you add,
keeps them verifiably intact, and publishes its archive as ordinary RSS feeds – so the podcast app
you already use (AntennaPod, PinePods, Apple Podcasts, …) plays from your archive.

Uguisu is not a player and does not sync listening progress. That is the job of your podcast app.

> Status: early development (0.1). The archive format may still change.

## What it does

- **Archives for the long term.** Every distinct version of a source feed is stored byte for byte.
  Episodes that disappear from a feed stay in the archive and are marked, not deleted. Changes to
  titles and show notes are kept as revisions together with the raw `<item>` XML.
- **Verifies its files.** Each media file is hashed with BLAKE3 while it is downloaded.
  `uguisu verify` re-hashes the archive, reports damaged or missing files and queues them for a
  new download.
- **Publishes the archive as feeds.** One feed per podcast behind a secret address, with the
  original episode GUIDs and support for range requests (seeking). An OPML export lists all
  archive feeds for import into a podcast app.
- **Takes input the easy way.** Search podcast directories (iTunes and fyyd), paste a feed
  address, or import an OPML file from your current app.
- **Keeps the archive readable without Uguisu.** The database is only an index. Everything needed
  to understand the archive is on disk as plain files.

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
```

## Running with Docker

```bash
docker compose up -d --build
```

Then open <http://localhost:8484> and choose a password. The container runs as an unprivileged
user (uid 65532); for the bind mount in `compose.yaml` make the directory writable for it:

```bash
mkdir -p data && sudo chown 65532:65532 data
```

Set `UGUISU_BASE_URL` to the address your podcast app will use to reach Uguisu. Put a reverse
proxy with HTTPS in front of it if the instance is reachable from outside your network.

## Running from source

Requires a current stable Rust toolchain.

```bash
cargo run --release -p uguisu
```

The result is a single binary with the web interface built in; it runs on Linux, Windows and
macOS.

## Configuration

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--data-dir` | `UGUISU_DATA_DIR` | `./data` | Database and archive |
| `--listen` | `UGUISU_LISTEN` | `0.0.0.0:8484` | Address of the web interface |
| `--base-url` | `UGUISU_BASE_URL` | derived from the request | Public address written into feeds |
| `--refresh-minutes` | `UGUISU_REFRESH_MINUTES` | `60` | Interval between checks of each source feed |
| `--download-concurrency` | `UGUISU_DOWNLOAD_CONCURRENCY` | `2` | Simultaneous downloads |
| `--allow-private-networks` | `UGUISU_ALLOW_PRIVATE_NETWORKS` | off | Allow sources on local and private addresses |

Commands: `uguisu serve` (default) and `uguisu verify`. `verify` exits with a non-zero status if
any file is damaged or missing, so it can be used from cron or a monitoring job.

## Security notes

- The web interface has a single account, protected by a password (Argon2) and a session cookie.
- Published feeds and their media need no login. They are protected by a random 192-bit token in
  the address, because most podcast apps cannot log in. Treat a feed address like a password; a new
  one can be generated per podcast.
- Uguisu refuses to fetch from loopback, private and link-local addresses unless
  `--allow-private-networks` is set. When an HTTP proxy is configured through the usual
  environment variables, host names are resolved by the proxy and this check only covers IP
  literals.
- Archive only what you are allowed to keep, and do not share feed addresses of content you have
  no right to redistribute.

## Project layout

- `crates/uguisu-core` – the archive engine: feed parsing, on-disk archive, downloads, fixity
  checks, feed generation, search, OPML.
- `crates/uguisu` – the server binary: web interface, authentication, published feeds.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Not there yet

Podcast Index search (needs an API key), Atom feeds, paged feeds (RFC 5005), keeping several
versions of an episode whose audio was replaced, transcripts and chapters, show-note images,
scheduled verification, and passing through Podcasting 2.0
tags in the published feeds.

## License

Uguisu is free software under the [GNU Affero General Public License](LICENSE), version 3 or
(at your option) any later version. It bundles [htmx](https://htmx.org) (Zero-Clause BSD).
