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
- [x] Archive images embedded in show notes and point the published show notes at the copies.
- [x] Several transcripts per episode, in whatever formats the feed offers.
- [x] Resume interrupted downloads with range requests.
- [x] Be polite to sources: a pause between requests per host, respect `Retry-After`.
- [x] Feeds that need credentials (HTTP Basic, for example paid feeds). Credentials never appear
      in logs or published feeds.
- [!] Podcast Index search. Needs an API key; registration is closed at the moment.
- [ ] Verify the fyyd search from a real instance (so far only the response format is verified).
- [x] Find out why single hosts answer 403 to media requests (seen with the BBC). It was the
      development environment: its network refuses plain `http` to that host. Over `https` the
      same file is served to Uguisu without complaint.

### 0.3 – A dependable archive

The promise "readable without Uguisu" is made good and tested.

- [x] Write down the archive format (`docs/ARCHIVE.md`) and give it a version number
      (`archive.json` in the archive root).
- [x] Close the gaps in the files on disk, part 1: `episode.json` lists the episode's files with
      their checksums and says whether the episode is still in the feed (archive format 2).
- [x] Close the gaps, part 2: earlier states of a feed entry per episode, and the settings and
      feed addresses per podcast.
- [x] `uguisu reindex`: rebuild the database completely from the archive on disk. Test: delete
      the database, reindex, the published feeds are identical.
- [x] `uguisu import <directory>`: take over an existing collection of files with its feed.
- [x] Repair: re-download a damaged file and keep the damaged copy until the new one is
      verified; show clearly when the source no longer offers the file.
- [x] Disk space: check the reserve before downloading, fail with a clear message instead of
      leaving half files.
- [x] Regular consistent backup of the database into the data directory.
- [x] Optional rules per podcast: archive only from a date on, or only new episodes. The
      default stays "everything".
- [x] Load test with a large archive: 100 podcasts with 500 episodes each, tiny media files,
      on a development machine. Library page query 0.19 s, one page of episodes 6 ms, publishing
      a feed of 500 episodes 32 ms, checking an unchanged feed 51 ms, verifying 50,000 files
      12 s, copying the database 0.5 s, rebuilding the index from the archive 19 s. Database
      4.5 KB per episode (223 MB), peak memory 67 MB. Adding a podcast costs about 1.2 s per
      500 episodes, spent on writing the files safely. Nothing here needs work before 1.0; the
      library query is the first candidate should archives grow much larger.

### 0.4 – Feeds that work everywhere

- [x] Conditional requests (ETag) and compression for published feeds. Caching the generated
      feed is dropped: building one takes about 30 ms for 500 episodes (see the load test).
- [x] Very large feeds: `?limit=N` on a feed address publishes only the newest N episodes.
- [ ] Very large feeds, continued: publish in pages (RFC 5005) – only if an app turns out to
      need it.
- [x] Cover thumbnails for the web interface. Originals stay untouched in the archive.
- [x] One combined feed over all podcasts.
- [x] Access to removed episodes and earlier media versions. Removed episodes stay in the
      published feed (an option per podcast). Earlier versions of a media file each have an
      address of their own, linked from the episode page; they are not put into the feed,
      where an app would show them as duplicate episodes.
- [ ] Compatibility check with real apps – AntennaPod, PinePods, Apple Podcasts, Pocket Casts,
      gPodder – documented as a table. Not checked with any app so far.
- [x] Validate generated feeds in the test suite.

### 0.5 – Web interface

- [x] Episode page: show notes, attachments, media versions, revisions with their differences.
- [x] History per podcast: feed snapshots, changes, errors.
- [x] Full-text search in the archive over titles and show notes (SQLite FTS5).
- [x] Full-text search over transcripts as well (WebVTT, SubRip, JSON, HTML, plain text).
- [x] Overview: running, waiting and failed downloads, state of verification, disk usage.
- [x] Settings: change the password, sign out other devices.
- [x] Settings: defaults for new podcasts.
- [x] German and English, chosen in the settings. Error messages that come from feeds, servers
      and the archive engine stay in English.
- [x] Accessibility (keyboard, contrast, screen readers) and a layout for phones. Checked with
      axe-core at phone and desktop width, in the light and the dark colour scheme. A test with a
      real screen reader is still to be done by a person.
- [ ] Final logo and icons. The current logo is provisional.

### 0.6 – Operation and security

- [x] Images for x86-64 and ARM64. Both are compiled on the build machine, the ARM one by
      cross-compiling; the finished ARM image is started once in an emulator before a release
      is published.
- [x] Provenance and SBOM for released images, attached to the image in the registry. (A
      signature was considered and dropped.) First used by the release after 0.3.0, where it
      is to be checked in the registry.
- [x] Automatic dependency updates and a scheduled `cargo deny` run.
- [x] Notifications about damaged files and feeds that keep failing, as a webhook that other
      services can be attached to (`UGUISU_WEBHOOK_URL`).
- [x] `uguisu reset-password` for a lost password.
- [x] Login: slow down repeated wrong passwords. (`Secure` cookies behind HTTPS were already
      in place.)
- [x] `SECURITY.md`: how to report a problem, what is protected and what is out of scope.
- [x] Security review (requests to private networks, paths, XML, headers). Found and fixed: a
      feed could exhaust memory through its own XML entities; IPv6 addresses that lead to
      private IPv4 addresses were let through; `reindex` followed file names out of the archive.
- [x] Operations guide: reverse proxy examples, backup and restore, moving, upgrading.
- [x] Stopping: at most a few seconds, and safe in the middle of a download. Measured: under
      0.2 s when idle or downloading; a download interrupted by a stop or a kill is continued
      and ends byte-identical. Requests still running are cut off after three seconds.

### 1.0 – The finished product

- [ ] Archive format and feed addresses are frozen. Later versions read 1.0 archives.
- [x] Migrations are tested from every released version: a real data directory written by
      each release is kept as a test fixture and must open, verify, publish and reindex with
      the current code. So far: 0.3.0.
- [x] Complete documentation: installation, operation, archive format, FAQ, legal notes.
- [x] `CHANGELOG.md`, `CONTRIBUTING.md`, versioning rules (SemVer).

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
- `podcast:alternateEnclosure`: further versions of an episode (other formats, video), as an
  option per podcast. These files can be large.
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
