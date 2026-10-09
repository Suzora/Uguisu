# ADR 0060 — Repairing a missing file: the exact bytes, or a new download

**Status:** accepted, amends [ADR 0018](0018-download-state-machine-and-queue.md) · **Date:** 2026-10-04

## Context

`archive verify` finds a file that is gone and marks its record `missing` (ADR 0021). Until now nothing could put it back. An import refuses an archived episode, because the record is the archive's word on the episode (ADR 0050). A download refuses too: the job is `completed`, which ADR 0018 made terminal, and since Phase 10 the queue also refuses an episode whose file came from an import or a rebuild. The only way out was to remove the podcast and add it again. A person who moved a folder by mistake, or whose disk failed and who has a backup, had no repair short of that.

ADR 0021 rejected repairing on detection. That still stands: a missing file is a finding, and the repair happens only when somebody asks for it.

## Decision

**Two repairs, each started by hand, and neither ever replaces a file.**

**Restore copies the record's exact bytes back from a folder.** `archive restore <folder>` and `POST /api/v1/archive/restore` scan the folder for files the size of a `missing` record. A file is used only when its SHA-256 equals the record's `hash_value`. A file that matches only `source_hash_value`, the bytes as they were before Uguisu tagged them, is reported as `source_only` and left alone: putting it back would quietly undo a tag write the record describes. The copy goes to a scratch file and is hashed again. It lands with a rename that never replaces (a hard link, then the scratch name is removed), so a file that appeared at the record's path is reported as `taken` and kept. A full verification then records the result. The folder is only read. The media directory is refused as a source, as an import refuses it. Without `--apply` (`apply: true`) it is a dry run that reports the same plan. A file already back at its path with the record's bytes is `returned` and needs nothing.

**Redownload fetches the episode again.** `archive redownload <episode>` and `POST /api/v1/archive/{episode}/redownload` refuse unless the episode has a record and nothing is at the record's path. Only `NotFound` counts as gone, so a file that cannot be checked is never downloaded over. The episode must still be downloadable, with the same refusals as any enqueue.
- A `completed` job is queued again, reason `redownload`, from the episode's current primary enclosure. Its target is rendered from the template again. It is refused while the job's old `.part` name holds a file, since a finalization that could not take that name back leaves a second link to the archived bytes there.
- A record that an import or a rebuild made gets a job of its own.
- When the download completes, its registration takes the record over. The record keeps its id and the time it was first archived. Everything else, including the provenance, describes the new bytes (ADR 0026). If the original came back to the record's path meanwhile, the registration is refused and the record stays with the original.

**`completed` is no longer terminal.** The transition table gains one edge, `completed → queued` on `Redownload`, and nothing else leaves `completed`: a retry, a resume or the worker cannot. The table's test asserts exactly that. `download_jobs.episode_id` stays unique, so a redownload is the same row queued again.

**The already-archived guard looks at the disk.** An episode whose record no completed download of its own stands behind is refused by enqueue, retry, resume and `retry-failed` only while its file is in place. Without that, a redownload that was paused or failed could never be resumed or retried. With the file gone, downloading it is the repair.

## Consequences

- A restore hashes candidate files, so it costs one read of every file of a matching size. Only files with an extension a missing record has are looked at, whatever their name, since a backup may have renamed them; a file whose extension changed is not found.
- A redownload may bring different bytes than the lost file had: a host that inserts ads or re-encodes does that. The record then describes the new bytes, and the old hash is gone with the old file. Restore is the repair that keeps the bytes; redownload is the one that needs no backup.
- The web UI does both on one page, `/archive/repair`, linked from the Archive page's missing count, the dashboard and an episode whose file is missing. It sends the folder as typed, in the body, as the import page does (ADR 0059), and the same Flatpak limit applies.
- An import that finds a missing record's bytes still reports `already_present` and copies nothing. Its detail now names `archive restore`.
- `docs/STATE_MACHINES.md` §3 and `docs/DOWNLOAD_ENGINE.md` §2 list the new edge and reason.

## Alternatives considered

- **Let an import put the file back.** An import matches by name, tags and score; a restore must match the bytes exactly. Mixing the two would let a guess repair an archive. Rejected.
- **Restore on a `source_hash_value` match.** It would bring back the file as it was before a tag write. The record would then disagree with the bytes until a verification marked them `invalid`. Reported instead.
- **A second job row per redownload.** It keeps `completed` terminal but breaks one job per episode (ADR 0018), and every query that finds an episode's job would have to choose between them. Rejected.
- **Redownload to the record's old path rather than the template's.** The template is how every other download picks its path; an old path may come from an import's layout or a template since changed. The registration moves the record to where the file landed.
