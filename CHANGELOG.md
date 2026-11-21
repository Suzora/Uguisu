# Changelog

What changed from release to release, for people who run Uguisu. The newest release is on top.
Versions follow [semantic versioning](CONTRIBUTING.md#versions).

## Unreleased

Nothing yet.

## 0.4.0

The first release published as a container image.

### Added

- The web interface in German; the language is chosen in the settings.
- Search in transcripts (WebVTT, SubRip, JSON, HTML, plain text), next to titles and show notes.
- Notifications to a webhook (`UGUISU_WEBHOOK_URL`) about damaged or missing files and about
  feeds that keep failing, with a test button in the settings.
- Settings for the options a new podcast starts with.
- A skip link, a marked current page and other improvements for keyboard and screen reader
  users; a layout for phones that no longer scrolls sideways.
- Provenance and an SBOM attached to released images.
- Earlier versions of a replaced media file can be downloaded from the episode page.

### Changed

- Stopping takes a few seconds at most: requests still being answered are cut off after three
  seconds instead of holding the process.
- Removing a podcast no longer rebuilds the whole search index.
- The ARM64 image is cross-compiled instead of built in an emulator.
- Password hashing moved to a new version of the Argon2 library. Existing passwords keep
  working.

### Fixed

- A feed could exhaust memory through XML entities it declared itself. Such feeds and OPML
  files are refused now.
- IPv6 addresses that lead to private IPv4 addresses (6to4, NAT64) were not recognised as
  private, so the protection against requests into the local network could be bypassed.
- `uguisu reindex` followed file names in a prepared archive out of the archive directory.

### Upgrading

- The search index is rebuilt once at the first start. On a large archive with many
  transcripts this takes a moment; searching works again when it is done.

## 0.3.0

The state of the code under this version number. No image was published for it; everything
listed here is part of 0.4.0.

### Archiving

- Source feeds in RSS 2.0, Atom and RSS 1.0; feeds split into pages (RFC 5005); the feed is
  found when the address of a podcast's web page is given.
- Every distinct version of a source feed is kept byte for byte. Episodes that are changed or
  removed by the publisher stay in the archive, with every earlier state of their feed entry.
- Media files, covers, transcripts, chapter files, episode images and images in show notes.
- Replaced media files can be kept as versions, per podcast.
- Interrupted downloads are continued; sources are asked politely (a pause per host,
  `Retry-After`); feeds with a password are supported and the password is never published.
- A start date per podcast: earlier episodes are listed, their files are not downloaded.
- Existing files are taken over with `uguisu import`.

### A dependable archive

- BLAKE3 checksums for every file, checked again regularly and on demand (`uguisu verify`).
  A damaged file is downloaded again and its damaged copy kept until the new one is verified.
- The archive on disk explains itself (`docs/ARCHIVE.md`, format 4); the database can be
  rebuilt from it with `uguisu reindex`.
- Daily copies of the database; a reserve of free disk space that downloads leave untouched.

### Feeds for podcast apps

- One feed per podcast behind a secret address, built from the original feed entries, with
  range requests, conditional requests and compression; `?limit=N` for a shorter feed.
- One feed over all podcasts.
- OPML import and export.

### Web interface

- Library, podcast and episode pages, the history of a source feed, a status page, full-text
  search over titles and show notes, settings.
- Search for podcasts in iTunes and fyyd.
- One account with an Argon2 password hash; repeated wrong passwords are slowed down;
  `uguisu reset-password` for a lost password.

### Running it

- A container image for x86-64 and ARM64 with a health check.
- An operations guide: HTTPS behind a reverse proxy, backup and restore, moving, upgrading.
