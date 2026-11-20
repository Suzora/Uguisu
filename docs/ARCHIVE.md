# The Uguisu archive format

Format version: **2** (early development – may still change before Uguisu 1.0)

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
    cover.<ext>                              cover art (jpg, png, webp or gif)
    feeds/
      <timestamp>-<hash>.xml                 source feed, byte for byte
    episodes/
      <date>-<title>-<hash>/                 one directory per episode
        episode.json                         episode metadata
        item.xml                             the episode's entry from the feed, unmodified
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
{ "format": 2, "software": "Uguisu" }
```

`format` is the version of this document. Uguisu refuses to open an archive with a higher
format version than it knows, so that an older program never damages a newer archive. An
archive in an older format is brought up to date in the background after the start.

| Version | Change |
| --- | --- |
| 1 | First format |
| 2 | `episode.json` also holds `hash`, `first_seen`, `removed` and `files` |

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
| `last_fetched` | Time of the last successful fetch |

Addresses never contain a user name or password.

## `feeds/`

Every distinct version of the source feed is stored exactly as the server sent it. The file name
is the time of the fetch (`YYYYMMDDTHHMMSSZ`) and the first eight characters of the file's BLAKE3
hash. A feed split into pages (RFC 5005) is stored page by page, all with the time of that fetch.
A version identical to an earlier one is not stored again.

These files are the most complete record: everything else in a podcast's directory except the
media can be derived from them.

## `episode.json` and `item.xml`

`item.xml` is the `<item>` (RSS) or `<entry>` (Atom) element of the episode, cut out of the feed
without any change. It is rewritten whenever the publisher changes the entry; earlier states are
found in the feed snapshots.

`episode.json` holds what Uguisu read from it:

| Field | Meaning |
| --- | --- |
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
| `files` | The archived files of the episode, see below |

Each entry of `files` describes one file in the episode's directory:

| Field | Meaning |
| --- | --- |
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

## Linked files

Transcripts, chapter files, the episode's own image and the images shown in its show notes are
stored next to the media file. The name is the kind of file and a short hash of its source
address. Their checksums are in the `files` list of `episode.json`.

## Temporary files

Files ending in `.part`, and `download.part.json`, belong to a download or a write in progress.
They can be deleted at any time while Uguisu is not running.

## What is only in the database

Known gaps of format version 2, to be closed before 1.0:

- earlier states of an episode's feed entry (they can be found in the feed snapshots, but not
  per episode);
- the time each file was last verified;
- settings per podcast and the secret addresses of the published feeds.
