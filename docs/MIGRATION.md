# Moving to Uguisu

Two things move: the **subscriptions**, as an OPML file ([ADR 0049](DECISIONS/0049-opml-import-and-export.md)), and the **files**, by an archive import ([ADR 0025](DECISIONS/0025-rebuild-and-import.md), [ADR 0050](DECISIONS/0050-podgrab-migration.md)). The import copies; it never moves, renames or deletes anything in the old tool's directories, and it never downloads an episode whose file it found.

The order matters: subscriptions, then a refresh, then the files, and only then automatic downloads.

| For | Read |
|---|---|
| Every flag and the JSON shapes | [`CLI.md`](CLI.md): `podcast import`, `archive import` |
| How a file is matched to an episode | [`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §18 |
| What the import reads and never writes | [`SECURITY.md`](SECURITY.md) §3.2 |

## From Podgrab

Podgrab keeps two directories (in its Docker image, the volumes `/assets` and `/config`):

| Directory | Holds |
|---|---|
| data (`/assets`) | one folder per podcast, named after it in lowercase with dashes; each episode as `[<n>-][<YYYY-MM-DD>-]<title-in-dashes>.<ext>`, beside `folder.jpg`, `album.nfo` and `images/` |
| config (`/config`) | `podgrab.db`, which records for every downloaded episode its GUID, enclosure, date and the file Podgrab wrote |

### Steps

1. **Stop Podgrab.** Its database must be at rest: Uguisu refuses to read it while a `podgrab.db-journal` or `podgrab.db-wal` lies beside it.
2. **Move the subscriptions.** Download Podgrab's OPML export (`http://<podgrab>/opml`), look at the plan, then add the feeds:

   ```text
   uguisu podcast import podgrab.opml
   uguisu podcast import podgrab.opml --apply
   ```

   Keep the default policy. **Do not pass `--mode auto` here:** the first refresh would queue downloads of episodes whose files are about to be imported.
3. **Fetch the episodes.** `uguisu podcast refresh --all`, or let `uguisu serve` do it. A file can only be matched to an episode the library has. If `uguisu episode duplicates` lists anything, resolve it first: a candidate duplicate looks like its original to the import, and a file for either would be `ambiguous`.
4. **Move the files.** Point the import at the directory Podgrab wrote to, give it Podgrab's database, read the plan, then apply it:

   ```text
   uguisu archive import /srv/podgrab/assets --podgrab-db /srv/podgrab/config/podgrab.db
   uguisu archive import /srv/podgrab/assets --podgrab-db /srv/podgrab/config/podgrab.db --apply
   ```

   With `--server`, both paths are on the server's machine. The web UI does the same on **Archive → Import an archive** (`/archive/import`): type both paths, read the plan, then copy ([ADR 0059](DECISIONS/0059-archive-import-in-the-web-ui.md)). The Flatpak app cannot read folders outside its sandbox; use the CLI there. A run that stops halfway is continued by running it again: what was copied is then `already_present`.
5. **Check the copies.** `uguisu archive verify --all`.
6. **Now turn on automatic downloads**, per podcast (`uguisu archive policy set <id> --mode auto`) or for all (`UGUISU_ARCHIVE_AUTO_DOWNLOAD=true`). Episodes that were imported are already archived and are not fetched again: queueing one, by hand or by the policy, is refused. A job that was queued before the import is not stopped by it; the import's plan names it, so cancel it first.

When Uguisu runs in a container, mount Podgrab's two directories read-only into it and use the paths inside the container. The import needs nothing more than reading them.

### Reading the plan

Each file has a line; `matched_by` says how it was matched:

| `matched_by` | Meaning |
|---|---|
| `source_database` | Podgrab's database named the episode by its GUID or its enclosure |
| `embedded_guid` | the file's own tags carry the episode's GUID |
| `scored` | name, date, number, duration and size agreed well enough and clearly enough |

| Action | What to do |
|---|---|
| `import` | Nothing: it is copied with `--apply`. |
| `already_present` | Nothing: the archive holds these bytes already, or another file in the same run does. |
| `conflict` | Read the detail. An episode already archived from a different file, or several different files for one episode, are never decided for you. |
| `ambiguous` | The file fits more than one episode, or its tags contradict its name. It is not imported. |
| `unmatched` | No episode fits, or the podcast is not in the library (the detail says which). |
| `invalid` | The file does not look like audio. |

### What does not move

- Played state, bookmarks, Podgrab's own tags and which podcasts were paused: OPML has no field for them. Pause again with `uguisu podcast pause <id>`.
- Covers and episode images. Uguisu fetches podcast artwork itself when `UGUISU_ARCHIVE_ARTWORK_FETCH` is on.
- Episodes Podgrab kept after the feed dropped them. Uguisu has no episode for them, so they are reported `unmatched` and stay in Podgrab's directory.
- Files whose extension is not an audio or video one (Podgrab takes it from the download URL, which can end in `.php`). The import does not see them; rename them in a copy of the folder.

### Without the database

Without `--podgrab-db`, files are matched by their names, which Podgrab lowercases and strips of everything but ASCII letters and digits. Most still match. A title without Latin letters leaves only the date prefix, if Podgrab wrote one; Podgrab's episode-number prefix is ambiguous with a title that begins with a number, so both readings are tried. When a folder name does not identify its podcast, pass `--podcast <id>` and import that folder alone.

## From another app

Export the subscriptions as OPML from the old app and import them as in step 2. Files in a folder per podcast, named after the episodes, are read with `uguisu archive import <dir>` (the layout is detected; `--format generic` forces the plain one). Every file's own tags are read: an embedded episode GUID names its episode, and a title tag rescues a file whose name says nothing. Steps 3 to 6 are the same.
