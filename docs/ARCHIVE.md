# The Uguisu archive format

Format version: **5** (early development – may still change before Uguisu 1.0)

An Uguisu archive is a directory of ordinary files. This document describes it so that an
archive can be read, checked and used without Uguisu – today or in twenty years.

The database `uguisu.db` next to the archive is an index for fast access. It is not part of the
format.

## Overview

```text
archive/
  archive.json                               format marker
  uguisu.json                                settings of this installation (private)
  <author> - <title>/                        one directory per podcast
    podcast.json                             podcast metadata
    uguisu.json                              settings of this installation (private)
    cover.<ext>                              cover art (jpg, png, webp or gif)
    feeds/
      <timestamp>-<hash>.xml                 source feed, byte for byte
    episodes/                                the media files, and nothing else
      <time> - <title>.<ext>                 one per episode: the current file
    metadata/
      <time> - <title>/                      everything else about that episode
        episode.json                         episode metadata
        item.xml                             the episode's entry from the feed, unmodified
        item-<timestamp>-<hash>.xml          earlier states of that entry
        media.<ext>.blake3                   checksum of the media file
        transcript-<hash>.<ext>              files the episode links to
        chapters-<hash>.<ext>
        image-<hash>.<ext>
        notes-image-<hash>.<ext>
        versions/
          v1.<ext>, v1.<ext>.blake3          earlier media files, if the source replaced one
```

All text files are UTF-8. JSON files are pretty-printed. Times are in UTC.

## `archive.json`

```json
{ "format": 4, "software": "Uguisu" }
```

`format` is the version of this document. Uguisu refuses to open an archive with a higher
format version than it knows, so that an older program never damages a newer archive. An
archive in an older format is brought up to date in the background after the start.

| Version | Change |
| --- | --- |
| 1 | First format |
| 2 | `episode.json` also holds `hash`, `first_seen`, `removed` and `files` |
| 3 | Earlier states of a feed entry are kept as `item-….xml`; `uguisu.json` per podcast; `namespaces` in `podcast.json` |
| 4 | `id` for episodes and files |
| 5 | Names as people write them; media files in `episodes/`, everything else in `metadata/<episode>/`; `path` for files in `episode.json` |

The `uguisu.json` next to it holds the secret part of the address of the feed over all podcasts
(`combined_feed_token`) and, once they have been changed, the options that podcasts get when
they are added (`new_podcasts`: `include_removed`, `keep_versions`, `only_new`). Like the file of the same name in each podcast's directory it belongs to
the installation, not to the record of the sources; leave it out when passing an archive on.

## Names

Names are made for people; nothing depends on parsing them.

- Podcast: `<author> - <title>` as the feed gives them, or the title alone if the feed names no
  author.
- Episode: the publication time in UTC as `YYYY-MM-DD HH.MM`, so that the names sort in order,
  then ` - ` and the title as the feed gives it. Without a publication time: `undated - <title>`.
  The media file is this name with the file's extension; the episode's metadata directory is
  this name.

Titles are kept as written, except for what file systems and network shares forbid: the
characters `/ \ : * ? " < > |` and control characters are left out, spaces and dots at either
end are dropped, and a name is cut after 150 characters (or 200 bytes). Two names that would be
the same, also in different case, get ` (2)`, ` (3)` and so on.

A name stays as it is when the title changes later.

## `podcast.json`

| Field | Meaning |
| --- | --- |
| `title`, `author`, `description`, `link`, `language` | As given in the feed |
| `image` | Source address of the cover art |
| `podcast_guid` | `<podcast:guid>` of the feed, if it has one |
| `feed_url` | Address the feed is fetched from |
| `original_feed_url` | Address the podcast was added with |
| `namespaces` | XML namespace declarations of the feed's root element as `[prefix, address]` pairs; the entries in `item.xml` use these prefixes |
| `last_fetched` | Time of the last successful fetch |

Addresses never contain a user name or password.

## `uguisu.json`

Everything else in the archive is a record of the source. This file holds what the installation
decided about the podcast:

| Field | Meaning |
| --- | --- |
| `feed_token` | The secret part of the address under which Uguisu publishes this podcast |
| `include_removed` | Whether episodes removed from the source feed are still published |
| `keep_versions` | Whether replaced media files are kept as versions |
| `archive_from` | Start date: files of episodes published before it are not downloaded; `null` for none |
| `added` | When the podcast was added to the archive |

Leave this file out when you pass an archive on: whoever knows the token can read the published
feed of the installation it came from.

## `feeds/`

Every distinct version of the source feed is stored exactly as the server sent it. The file name
is the time of the fetch (`YYYYMMDDTHHMMSSZ`) and the first eight characters of the file's BLAKE3
hash. A feed split into pages (RFC 5005) is stored page by page, all with the time of that fetch.
A version identical to an earlier one is not stored again.

These files are the most complete record: everything else in a podcast's directory except the
media can be derived from them.

## `episode.json` and `item.xml`

`item.xml` is the `<item>` (RSS) or `<entry>` (Atom) element of the episode, cut out of the feed
without any change. When the publisher changes the entry, the earlier state is kept as
`item-<timestamp>-<hash>.xml`, named after the time it was first seen and the start of its hash.

`episode.json` holds what Uguisu read from it:

| Field | Meaning |
| --- | --- |
| `id` | Number of the episode in this installation; part of the addresses in the published feed |
| `guid` | The episode's identifier in the feed |
| `title`, `description`, `link` | As given in the feed; `description` is HTML |
| `published` | Publication time |
| `duration_seconds` | Duration, if the feed gives one |
| `enclosure.url`, `.type`, `.length` | Source address, media type and size of the media file as the feed states them |
| `image` | Source address of the episode's own image |
| `season`, `episode` | Numbers, if the feed gives them |
| `hash` | Identifies this state of the feed entry; it changes when the publisher changes anything |
| `first_seen` | When the episode first appeared in the source feed |
| `removed` | When the episode was found missing from the source feed; `null` while it is listed |
| `revisions` | Every state of the feed entry, oldest first: `seen` (time), `hash` and `file` |
| `files` | The archived files of the episode, see below |

Each entry of `files` describes one file of the episode:

| Field | Meaning |
| --- | --- |
| `id` | Number of the file in this installation; part of the address of a linked file in the published feed |
| `name` | File name |
| `path` | Where the file is, relative to the podcast's directory, e.g. `episodes/2024-10-23 22.07 - Trailer.mp3` (from format 5; before, files lay in the episode's directory) |
| `kind` | `media`, `transcript`, `chapters`, `image` or `notes-image` |
| `source` | Address the file was downloaded from, without user name and password |
| `blake3` | BLAKE3 checksum, 64 hexadecimal characters |
| `size` | Size in bytes |
| `type` | Media type |
| `downloaded` | Time of the download |

Media files come first, the oldest version first.

## Media files and checksums

`episodes/<episode>.<ext>` is the file the feed's enclosure pointed at, unchanged – or, when the
feed offers the same episode in better quality (`podcast:alternateEnclosure`, `media:content` or
a further Atom enclosure of the same kind, with a higher bitrate or, failing that, a larger
size), that file. `source` in `episode.json` says which one it was; `enclosure` keeps what the
feed states. Its BLAKE3
checksum is in `metadata/<episode>/media.<ext>.blake3`, in the format of the `b3sum` tool, with
the path to the file as seen from there:

```text
<64 hexadecimal characters>  ../../episodes/2024-10-23 22.07 - Trailer.mp3
```

so a whole archive can be checked without Uguisu:

```bash
find archive -name '*.blake3' -execdir b3sum -c {} \;
```

When a podcast is set to keep replaced files and the publisher replaces one, the new file takes
the place in `episodes/` and the earlier one moves to `metadata/<episode>/versions/v1.<ext>`
(then `v2`, and so on), its checksum next to it. The highest number is the newest of the
earlier ones.

A file named `damaged-<hash>.<ext>` in the metadata directory is a copy that no longer matched
its checksum and could not be repaired, because the source by then offered a different file. It
is kept because it may be the only trace of the original; `<hash>` is the start of the checksum
it should have. It is not listed in `episode.json`.

## Linked files

Transcripts, chapter files, the episode's own image and the images shown in its show notes are
stored in the episode's metadata directory. The name is the kind of file and a short hash of its source
address. Their checksums are in the `files` list of `episode.json`.

## Temporary files

Files ending in `.part`, and `download.part.json`, belong to a download or a write in progress.
They can be deleted at any time while Uguisu is not running.

## Rebuilding the database

`uguisu reindex` fills an empty database from these files, in this format and in the layout of
formats 1 to 4 (which is moved to the current one at the next start): podcasts with their
settings, feed snapshots, episodes with every recorded state of their feed entry, and the
archived files with their checksums. The published feeds keep their addresses. Stop Uguisu and
move `uguisu.db` away before running it.

## What is only in the database

Not part of the archive, by design:

- user names and passwords of feeds behind a password;
- the time each file was last verified, and the state of downloads that have not succeeded yet;
- the login of the web interface.
