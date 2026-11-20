# Uguisu – project plan

This is the plan from the current state to the finished product. It says what is done, what
comes next and in which order, what will deliberately not be built, and how "finished" (1.0) is
measured. It is updated with every pull request that completes or changes an item.

Legend: `[x]` done · `[~]` in review · `[ ]` open · `[!]` blocked from outside

## 1. Goal and principles

Uguisu archives podcasts for the long term, keeps the archive verifiably intact, and publishes it
as ordinary RSS feeds. Playing episodes and tracking listening progress is the job of the podcast
app that subscribes to those feeds.

Principles every decision is measured against:

- **Faithful to the source.** Feeds are kept byte for byte; episodes are published as their
  original `<item>` elements.
- **Nothing disappears silently.** What a publisher removes or replaces stays in the archive and
  is marked.
- **Readable without Uguisu.** The database is only an index. The files on disk explain
  themselves.
- **Verifiable.** Every archived file has a BLAKE3 checksum and is re-checked regularly.
- **Small surface.** One user, one web interface, one way to run it: the container image.

### Non-goals

- A player, playback queues, or syncing of listening progress.
- Native apps or clients, and standalone programs for Windows, Linux or Android.
- Multiple user accounts.
- Recommendations, ratings or other social features.

## 2. Where the project stands (0.1)

- [x] Rust workspace: `uguisu-core` (archive engine) and `uguisu` (server).
- [x] Self-describing archive on disk, SQLite as index.
- [x] Byte-exact snapshots of every distinct version of a source feed.
- [x] Episode revisions, keyed by a hash over the whole `<item>`.
- [x] Episodes removed from a feed are kept and marked.
- [x] BLAKE3 checksums, `uguisu verify`, and automatic background verification.
- [x] Optional versions of replaced media files, per podcast.
- [x] Attachments: transcripts, chapter files, episode images.
- [x] Published feeds behind a secret token, with range requests.
- [x] Search in iTunes and fyyd; adding by feed address; OPML import and export.
- [x] Protection against requests to private networks.
- [x] Login with one account (Argon2, server-side sessions, CSRF check, CSP).
- [x] Container image with health check; CI (format, lints, tests, `cargo deny`, `typos`, image).
- [x] Release workflow for the image; its dry run passes. Publishing itself runs for the first
      time with the first version tag.
- [x] Feeds split into pages (RFC 5005) are read to the last page.

## 3. Milestones

Each milestone ends with a release. Items are worked through one topic per pull request.

### 0.2 – Complete ingestion

Everything a podcast offers ends up in the archive.

- [x] Paged and archived feeds (RFC 5005).
- [x] Atom and RSS 1.0 as source formats. The published feed stays RSS 2.0.
- [x] Find the feed when the user enters the address of a web page
      (`<link rel="alternate">`), instead of reporting an error.
- [ ] Archive images embedded in show notes and point the published show notes at the copies.
- [ ] More Podcasting 2.0 files: `podcast:alternateEnclosure`, several transcript formats.
- [~] Resume interrupted downloads with range requests.
- [ ] Be polite to sources: a pause between requests per host, respect `Retry-After`.
- [ ] Feeds that need credentials (HTTP Basic, for example paid feeds). Credentials never appear
      in logs or published feeds.
- [!] Podcast Index search. Needs an API key; registration is closed at the moment.
- [ ] Verify the fyyd search from a real instance (so far only the response format is verified).
- [ ] Find out why single hosts answer 403 to media requests (seen with the BBC).

### 0.3 – A dependable archive

The promise "readable without Uguisu" is made good and tested.

- [ ] Write down the archive format (`docs/ARCHIVE.md`) and give it a version number
      (`archive.json` in the archive root).
- [ ] Close the gaps in the files on disk: revisions, attachments, media versions and removal
      marks must all be recoverable from them.
- [ ] `uguisu reindex`: rebuild the database completely from the archive on disk. Test: delete
      the database, reindex, the published feeds are identical.
- [ ] `uguisu import <directory>`: take over an existing collection of files with its feed.
- [ ] Repair: re-download a damaged file and keep the damaged copy until the new one is
      verified; show clearly when the source no longer offers the file.
- [ ] Disk space: check the reserve before downloading, fail with a clear message instead of
      leaving half files.
- [ ] Regular consistent backup of the database into the data directory.
- [ ] Optional rules per podcast: archive only from a date on, or only new episodes. The
      default stays "everything".
- [ ] Load test with a large archive (100 podcasts, 50,000 episodes): start-up, queries, memory.
      Known today: about 8 KB of database per episode.

### 0.4 – Feeds that work everywhere

- [ ] Conditional requests (ETag, Last-Modified) and compression for published feeds; cache the
      generated feed.
- [ ] Very large feeds: optionally limit to the newest episodes, or publish in pages (RFC 5005).
- [ ] Cover thumbnails for the web interface. Originals stay untouched in the archive.
- [ ] One combined feed over all podcasts.
- [ ] Access to removed episodes and earlier media versions through the published feed.
- [ ] Compatibility check with real apps – AntennaPod, PinePods, Apple Podcasts, Pocket Casts,
      gPodder – documented as a table. Not checked with any app so far.
- [ ] Validate generated feeds in the test suite.

### 0.5 – Web interface

- [ ] Episode page: show notes, attachments, media versions, revisions with their differences.
- [ ] History per podcast: feed snapshots, changes, errors.
- [ ] Full-text search in the archive over titles, show notes and transcripts (SQLite FTS5).
- [ ] Overview: running, waiting and failed downloads, state of verification, disk usage.
- [ ] Settings: change the password, end sessions, defaults for new podcasts.
- [ ] German and English.
- [ ] Accessibility (keyboard, contrast, screen readers) and a layout for phones.
- [ ] Final logo and icons. The current logo is provisional.

### 0.6 – Operation and security

- [ ] Images for x86-64 and ARM64.
- [ ] Provenance, SBOM and signature for released images.
- [ ] Automatic dependency updates and a scheduled `cargo deny` run.
- [ ] Notifications about damaged files and feeds that keep failing (webhook, ntfy); optional
      `/metrics`.
- [ ] Login: limit failed attempts, `uguisu reset-password`, `Secure` cookies behind HTTPS.
- [ ] Security review (requests to private networks, paths, XML, headers) and `SECURITY.md`.
- [ ] Operations guide: reverse proxy examples, backup and restore, moving, upgrading.
- [ ] Faster shutdown (about 2 seconds today) and safe behaviour when stopped mid-download.

### 1.0 – The finished product

- [ ] Archive format and feed addresses are frozen. Later versions read 1.0 archives.
- [ ] Migrations are tested from every released version.
- [ ] Complete documentation: installation, operation, archive format, FAQ, legal notes.
- [ ] `CHANGELOG.md`, `CONTRIBUTING.md`, versioning rules (SemVer).

Uguisu is "finished" when:

1. every item of 0.2 to 0.6 is done or deliberately struck from this plan,
2. the reindex test and the app compatibility checks pass,
3. a real instance has run for four weeks without data loss and without an open critical bug.

## 4. After 1.0

Ideas without commitment:

- A second storage location or mirror (second directory, S3-compatible storage).
- Taking over libraries from PinePods or Audiobookshelf.
- Faster updates through Podping or WebSub.
- Archiving the podcast's web pages along with the episodes.
- Multiple users – only if a real need shows up.

## 5. How the work is done

- One topic per pull request: draft, green CI, then merged on the maintainer's word.
- Every feature comes with a test; every schema change is a migration.
- What could not be verified is named in the pull request.
- Dependencies need a licence allowed in `deny.toml`.

## 6. Risks and open questions

- **Legal.** Archiving and passing on content is only allowed where the user has the right to.
  The documentation must say so clearly.
- **Sources that block automated requests.** Can leave gaps in an archive; they must be visible.
- **Storage.** Archives grow without limit by design; the interface has to show the cost.
- **Directories.** No Podcast Index access; fyyd answers slowly (15 to 30 seconds observed).
- **SQLite.** Fine for one user; to be confirmed by the load test in 0.3.
