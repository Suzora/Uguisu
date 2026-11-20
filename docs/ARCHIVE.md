# The Uguisu archive format

Format version: **4** (early development – may still change before Uguisu 1.0)

An Uguisu archive is a directory of ordinary files. This document describes it so that an
archive can be read, checked and used without Uguisu – today or in twenty years.

The database `uguisu.db` next to the archive is an index for fast access. It is not part of the
format.

## Overview

```text
archive/
  archive.json                               format marker
  <podcast>-<hash>/                          one directory per podcast
    podcast.json                             podcast metadata
    uguisu.json                              settings of this installation (private)
    cover.<ext>                              cover art (jpg, png, webp or gif)
    feeds/
      <timestamp>-<hash>.xml                 source feed, byte for byte
    episodes/
      <date>-<title>-<hash>/                 one directory per episode
        episode.json                         episode metadata
        item.xml                             the episode's entry from the feed, unmodified
        item-<timestamp>-<hash>.xml          earlier states of that entry
        media.<ext>                          the episode
        media.<ext>.blake3                   its checksum
        media.v2.<ext>, media.v2.<ext>.blake3    later versions, if the source replaced the file
        transcript-<hash>.<ext>              files the episode links to
        chapters-<hash>.<ext>
        image-<hash>.<ext>
        notes-image-<hash>.<ext>
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

## Directory names

Names are made for people; nothing depends on parsing them.

- Podcast: the title in lower case with everything but letters and digits replaced by `-`,
  followed by a short hash of the feed address it was added with.
- Episode: the publication date (`YYYY-MM-DD`, or `undated`), the title treated the same way, and
  a short hash of the episode's identifier.

A directory keeps its name when the title changes later.

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

Each entry of `files` describes one file in the episode's directory:

| Field | Meaning |
| --- | --- |
| `id` | Number of the file in this installation; part of the address of a linked file in the published feed |
| `name` | File name |
| `kind` | `media`, `transcript`, `chapters`, `image` or `notes-image` |
| `source` | Address the file was downloaded from, without user name and password |
| `blake3` | BLAKE3 checksum, 64 hexadecimal characters |
| `size` | Size in bytes |
| `type` | Media type |
| `downloaded` | Time of the download |

Media files come first, the oldest version first.

## Media files and checksums

`media.<ext>` is the file the feed's enclosure pointed at, unchanged. Next to it,
`media.<ext>.blake3` holds its BLAKE3 checksum in the format of the `b3sum` tool:

```text
<64 hexadecimal characters>  media.mp3
```

so a whole archive can be checked without Uguisu:

```bash
find archive -name '*.blake3' -execdir b3sum -c {} \;
```

When a podcast is set to keep replaced files, a file the publisher replaced is kept, and the new
one is stored as `media.v2.<ext>`, then `media.v3.<ext>` and so on. The highest number is the
newest.

A file named `media.damaged-<hash>.<ext>` is a copy that no longer matched its checksum and
could not be repaired, because the source by then offered a different file. It is kept because
it may be the only trace of the original; `<hash>` is the start of the checksum it should have.
It is not listed in `episode.json`.

## Linked files

Transcripts, chapter files, the episode's own image and the images shown in its show notes are
stored next to the media file. The name is the kind of file and a short hash of its source
address. Their checksums are in the `files` list of `episode.json`.

## Temporary files

Files ending in `.part`, and `download.part.json`, belong to a download or a write in progress.
They can be deleted at any time while Uguisu is not running.

## Rebuilding the database

`uguisu reindex` fills an empty database from these files: podcasts with their settings, feed
snapshots, episodes with every recorded state of their feed entry, and the archived files with
their checksums. The published feeds keep their addresses. Stop Uguisu and move `uguisu.db` away
before running it.

## What is only in the database

Not part of the archive, by design:

- user names and passwords of feeds behind a password;
- the time each file was last verified, and the state of downloads that have not succeeded yet;
- the login of the web interface.
