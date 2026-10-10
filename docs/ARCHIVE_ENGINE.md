# Archive Engine

How Uguisu turns a transferred file into an archive artifact it can vouch for: the record, the path template, the sanitization profiles, collision handling, verification, relocation, the automatic archive policy, reconciliation, and everything the API and CLI expose. Built (`uguisu-archive` for the pure logic, `uguisu-storage` for the tables, `uguisu-engine` for the wiring) and extended in Phase 6 with everything that makes the archive **portable**: the per-episode sidecar, the per-podcast manifest, rebuilding the index from disk, importing a foreign archive, podcast artwork and metadata tag writing (§§15–20).

The lifecycle diagram is in [`STATE_MACHINES.md`](STATE_MACHINES.md) §4, the tables in [`DATA_MODEL.md`](DATA_MODEL.md) §§9–10 and 13, the decisions in ADRs [0021](DECISIONS/0021-archive-file-and-verification.md), [0022](DECISIONS/0022-template-grammar-and-path-safety.md), [0023](DECISIONS/0023-archive-policy.md), [0024](DECISIONS/0024-sidecars-and-manifests.md), [0025](DECISIONS/0025-rebuild-and-import.md) and [0026](DECISIONS/0026-tagging-and-artwork.md), and the path-safety rules in [`SECURITY.md`](SECURITY.md) §3.2.

## 0. Guarantees

1. **Nothing is deleted to restore consistency.** A file whose record is gone is reported by `archive orphans` (§21), never removed. A record whose file is gone keeps its hash, so the artifact can be recovered by putting the file back or downloading it again (§22). There is no code path in Phase 5 that deletes a media file.
2. **Verification never writes.** A pass reads; it does not truncate, rename, re-hash-to-fit or repair. A file that was edited in place is reported `invalid` and left exactly as the user left it.
3. **Every archive path stays under the media root.** Containment is decided component by component after symlinks are resolved, never by comparing strings. A template, a feed title, a stored path and a relocation target all go through the same check.
4. **One active record per episode.** `archive_files.episode_id` is unique, so a concurrent or repeated registration converges on one row rather than racing. `relative_path` is unique too, so two artifacts can never claim one path.
5. **The same episode always lands in the same place.** Rendering is a pure function of the template, the episode and the profile, and the collision suffix is derived from the episode's own id — not from a counter, which would depend on the order downloads happened to finish.
6. **A preview creates nothing.** `path-preview` renders and resolves without touching the filesystem, so it is safe on a read-only archive and before anything has been downloaded.
7. **Automatic archiving is off until it is turned on.** With the shipped configuration, discovering an episode queues nothing. A manual `download` is never routed through the policy and never blocked by it.
8. **The archive describes itself.** Every artifact has a sidecar beside it and appears in its podcast's manifest, so the database can be lost and rebuilt from what is on disk. A sidecar is metadata, not evidence: a rebuilt record says `unchecked` until something reads the bytes (§17).
9. **A foreign archive is only ever read.** Import copies; it does not move, modify or delete anything in the tree it was pointed at, and a file that matches more than one episode is reported rather than placed (§18).
10. **Uguisu never modifies the only copy of a file.** Tag writing copies the artifact, tags the copy, proves it readable and renames it into place. A marker is committed before a byte moves, so an interrupted write can be told from tampering (§20).

## 1. Pipeline

```text
download completes           (Phase 4 transaction, unchanged)
   → register                read the job, stat the file, upsert one `archive_files` row,
                             `archive.registered`; own transaction, no hashing inside it
   → verify                  existence / size+mtime / SHA-256, depending on configuration
   → record                  verification columns + the episode's archive_state + an event
```

and, independently:

```text
feed refresh commits
   → policy                  decide(policy, episode, position, now) per new episode, newest first
   → enqueue                 DownloadService::enqueue_episode for a Queue decision
   → announce                archive.policy_queued / archive.policy_skipped
```

Registration deliberately happens **after** the completion transaction rather than inside it: hashing a multi-gigabyte artifact inside the single writer transaction would block every other command for as long as the hash takes. The cost of that choice is a crash window — a completed job with no record — which §8 repairs on the next start. The step is driven by the `download.completed` event on the bounded bus; when the watcher falls behind and skips events, it runs §8's first step itself once the bus is quiet, so a long-running server does not wait for a restart.

## 2. The record

One row per episode in `archive_files`:

| Group | Columns | Changes when |
|---|---|---|
| Identity | `id`, `episode_id` (unique), `podcast_id`, `registered_at` | never, once written |
| The bytes | `size_bytes`, `hash_algo`, `hash_value` | a new download, or a tag write (§20) |
| Where | `relative_path` (unique), `mtime_unix` | a relocation; `mtime_unix` also whenever a check or a tag write stats the file |
| What | `content_type`, `sniffed_type` | a new download |
| Last check | `verification_state`, `verification_reason`, `verified_at` | every verification, and a tag write (§20) |

The hash is the **download's** hash, computed while the bytes streamed in, until a tag write records the hash of the bytes it wrote; the received one stays in `source_hash_value` ([`DATA_MODEL.md`](DATA_MODEL.md) §10). Verification compares against it; it never replaces it. That is what makes "this file is not what we received" a statement Uguisu can make at all.

## 3. Verification

`VerificationState` (persisted in `archive_files.verification_state`):

| State | Meaning |
|---|---|
| `unchecked` | registered, or a check could not complete (permissions, I/O) |
| `verified` | the file is there and matches, as far as the depth looked |
| `missing` | nothing is at the recorded path |
| `invalid` | something is there, but it is not the recorded artifact |

`VerifyDepth` decides the cost:

| Depth | Reads | Used by |
|---|---|---|
| `existence` | one `symlink_metadata` | a server's check once it is up; `archive reconcile` |
| `light` | type, size, and mtime when one was recorded | the default `verify`, completion |
| `full` | the whole file through SHA-256, 1 MiB at a time | `verify --full` |

A light pass cannot see an edit that preserved the length and the mtime; that is the price of not reading gigabytes, and `--full` is the answer. When the size matches but the mtime moved, a light pass says `unchecked` (`mtime_changed`) rather than `verified`. It never clears an `invalid` record either way: only a full pass does, since the size and mtime are what the failed hash already looked past. Only a pass that comes back `verified` records the mtime it found, so asking again does not make a light pass trust the size. A file larger than memory costs 1 MiB regardless of its size.

Reasons come from a closed vocabulary (`registered`, `present`, `size_match`, `mtime_changed`, `hash_match`, `not_found`, `not_a_file`, `size_mismatch`, `hash_mismatch`, `empty`, `permission_denied`, `io_error`, `outside_root`, `relocated`, `rebuilt`, `imported`, `tagged`, `retag_recovered`), so a client can translate them rather than parse prose.

Three distinctions the states depend on:

- **Only `NotFound` means gone.** A permission error or a broken disk leaves the record `unchecked`, not `missing` — marking it missing would invite a re-download of a file that is sitting right there.
- **The final component is not followed.** A symlink standing in for the artifact is `not_a_file` whatever it points at, and a symlinked parent that leaves the root is `outside_root`, refused before the file is opened.
- **Empty is its own failure.** A zero-length file where bytes were expected is what an interrupted copy or a full disk leaves behind, so it is reported as `empty` rather than as a size mismatch.

### Projection onto the episode

| Verification | `episodes.archive_state` |
|---|---|
| `verified` | `archived` |
| `missing` | `missing` |
| `invalid` | `modified` |
| `unchecked` | left as it is |

No new `ArchiveState` variant was needed, so Phase 5 required no data migration of episode state.

## 4. Path templates

The grammar of ADR 0009, as amended by ADR 0022:

```text
template := part ('/' part)*
part     := element*
element  := literal | variable | '[' element* ']'
variable := '{' name (':' format)? ('|' filter (':' arg)?)* '}'
```

A `/` at the top level separates path components. A `/` inside a value cannot: values are sanitized per segment, so a title containing a slash produces one segment, not two.

**Variables** (17): `podcast.title`, `podcast.author`, `podcast.id`, `episode.title`, `episode.id`, `episode.guid`, `episode.identity` (a short hash), `episode.date` (ISO), `episode.year`, `episode.month`, `episode.day`, `episode.season`, `episode.number`, `episode.type`, `episode.duration`, `episode.published` (with a strftime subset: `%Y %m %d %H %M %S %%`), `extension`.

**Filters** (7): `|lower`, `|upper`, `|slug`, `|ascii`, `|pad:n` (numbers only), `|truncate:n`, `|default:"…"`.

**Optional groups** render only when every variable inside them has a value, which is how `[S{episode.season|pad:2}E{episode.number|pad:2} - ]` disappears for a podcast without seasons instead of leaving `SE - ` behind. A `|default:"…"` inside a group satisfies it.

The default template is:

```text
{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}
```

### What the renderer owns

- **The extension.** The template writes `.{extension}`, but the renderer splits it off, sanitizes and truncates the stem, adds any collision suffix, and appends the extension again. So it is never doubled, never truncated away, and present even in a template that forgot it.
- **The fallback stem.** A file name that renders to nothing but punctuation — an untitled episode in a `{date} - {title}` template — becomes the episode id, which keeps the file addressable and unique.
- **The length.** At most 120 characters and 200 UTF-8 bytes per segment, and 200 characters per path. Linux counts a name in bytes (255 at most), so a segment of CJK or emoji text is cut by the byte limit first; the 55 bytes left over are for the `.json` of a sidecar and the suffix of a scratch copy. When the whole path is too long, directory components are shortened deterministically from the longest one and the file name is left intact.

Rendering never touches the filesystem.

## 5. Sanitization profiles

Applied to every segment, under `UGUISU_ARCHIVE_PATH_PROFILE`:

| Rule | `posix` | `windows` | `portable` (default) |
|---|---|---|---|
| `/` and `\` → `-` | yes | yes | yes |
| `< > : " \| ? *` → `-` | no | yes | yes |
| trailing dots and spaces trimmed | spaces only | yes | yes |
| device names (`CON`, `NUL`, `LPT1`, `COM¹`, …) escaped | no | yes | yes |
| control, bidi and zero-width characters dropped | yes | yes | yes |
| whitespace collapsed, NFC normalization | yes | yes | yes |
| a name of only dots dropped | yes | yes | yes |
| `C:` prefix → `C-` | yes | yes | yes |

Three properties hold on all three profiles, and are property-tested against hostile input (traversal, mixed separators, absolute and UNC forms, drive letters, device names, NUL and bidi overrides, full-width confusables, 200-character values):

1. **Idempotence.** Sanitizing an already sanitized segment returns it unchanged, so a path read back from the database does not drift. A device name is escaped *inside* the name (`nul.mp3` → `nul_.mp3`), not at the end, precisely so the second pass sees a name that is no longer reserved.
2. **Determinism.** The same input always produces the same output.
3. **Containment.** A rendered path is relative, has no `.` or `..` component, and joins onto any root without leaving it.

## 6. Collisions

Two episodes of one podcast published on the same day with the same title render to one path. The rule:

1. The preferred path, when it is free or already this episode's artifact.
2. Otherwise the preferred path with ` [xxxxxx]` before the extension, where the six characters are the **tail** of the episode's ULID — its random part, since the leading characters encode the timestamp and episodes added in the same millisecond share them.
3. If the suffixed path is taken by another episode too, the condition is reported rather than guessed at a third time.

ADR 0009 originally proposed a counter (` (2)`, ` (3)`). ADR 0022 replaced it: a counter depends on the order in which downloads happened to finish, so the same episode could land somewhere else after a restart, which breaks guarantee 5. A file Uguisu has no record of is avoided, never overwritten.

Occupancy is answered from two sources — which episode owns a path in the database, and whether anything exists on disk — so an imported archive, a leftover from an older layout and a user's own file are all respected.

**Titles that differ only in case.** The database compares paths exactly, the disk as the file system does. An import or a relocation asks the disk, so on a file system that folds case (NTFS, APFS) it sees the other file and takes the suffix. A download asks the disk when it is queued and keeps the identifier layout if the rendered name is taken. Two downloads queued before either file exists both get the name: the later one finds it taken when it finishes and ends `failed(target_exists)`, keeping the earlier file and its own `.part`, and a retry renders its target again, sees the taken name and finishes under the identifier layout. On Linux both files are kept under their names. Nothing is overwritten either way (`crates/uguisu-engine/tests/archive.rs` `case_variant_titles_never_overwrite`).

## 7. Destinations and relocation

New downloads are written straight to the rendered path: `uguisu-download` asks a `DestinationResolver` the engine supplies. The queue knows nothing about templates; it checks the answer (containment, and no other job already aiming there) and falls back to the `<podcast id>/<episode id>.<ext>` layout when anything is off. That fallback is never an error — the download proceeds and `archive relocate` can move the file later. `part_path` is untouched, so finalization remains a rename inside the media directory.

**Relocation is explicit.** Changing the template does not move files that are already archived. `archive relocate` renders the current template for one episode, a podcast or everything, and:

```text
render → resolve collisions → containment check → create parents
       → refuse an occupied target → move without replacing → fsync the directory
       → update the record (unique path index is the real guard) → archive.relocated
```

The move never replaces a file (a hard link, then removing the old name; [ADR 0019](DECISIONS/0019-resume-and-finalization.md)): the file is at one path, the other, or briefly both, never neither, so a crash during a relocation leaves an archive reconciliation can make sense of. A **cross-device target is refused**, not copied: a copy would double the archive's size without saying so and leave two files that both look authoritative. If the record update fails because another artifact took the path in between, the file is moved back.

`--dry-run` reports the moves without making them.

## 8. Reconciliation

`Engine::open` runs the first step after the download queue's recovery; `serve` and the desktop run the second in the background once they are up, and `archive reconcile` runs both:

1. Register every completed download that has no record, **where its file lies** — the path is not recomputed, so reconciliation never moves anything.
2. Confirm that every recorded file exists (one `stat` each).

A start reads only the database: one `stat` per file took 12.6 s for 100 000 files on NTFS, and every command would wait for it ([ADR 0021](DECISIONS/0021-archive-file-and-verification.md), amended). Nothing at a start hashes; a full check is an explicit `archive verify --full`. A deep reconciliation (`--deep`, or the API's `?deep=true`) adds a light pass. A completed download whose file cannot be found is reported and left alone; the job stays completed and the file, wherever it is, stays, until somebody repairs it (§22).

**A changed source is reported, never acted on.** When a refresh changes an archived episode's primary enclosure — another URL or another declared length — the record gets `source_changed_at` and an `archive.source_changed` event names the old and new values. The file is kept and nothing is downloaded: the episode has one archived file, and replacing it would discard what was received. `archive list --source-changed` (`GET /api/v1/archive?source_changed=true`) lists these files for a person to look at. Nothing downloads such an episode again while its file is in place: a new download would have to replace a file Uguisu keeps. A re-registration of the file clears the flag. Hosts that rotate tracking prefixes or insert ads dynamically will trip it; that is a reason to look, not a fault.

## 9. Archive policy

Whether a newly discovered episode is downloaded without being asked. **Off by default**, opt-in globally (`UGUISU_ARCHIVE_AUTO_DOWNLOAD`) or per podcast.

The decision is a pure function of the policy, the episode and the clock — no database, no filesystem, no queue — so it is testable as a table. The engine asks it and then calls the same `enqueue_episode` a user command uses, which is why `uguisu-download` never learns what a policy is and why the unique `download_jobs.episode_id` remains the real idempotency boundary: three refreshes of one feed produce one job per episode.

Checks run in order of cost and certainty, each with a reason from a closed vocabulary:

| Reason | When |
|---|---|
| `disabled` | the podcast (or the installation) is in manual mode |
| `already_archived` | the episode has an artifact, or is `archived` |
| `already_queued` | a job exists, or the episode is queued/downloading |
| `skipped` | the episode was marked skipped |
| `duplicate` | it is a duplicate candidate of another episode |
| `removed_from_feed` | it is gone from the feed |
| `no_enclosure` | there is nothing to download |
| `too_old` | published before the age limit |
| `backlog_exceeded` | the podcast already has its limit of episodes awaiting archiving |

`archive.policy_skipped` is emitted for every reason **except** `disabled`: announcing "the policy is off" for every episode of every refresh on a default installation would be noise.

Two readings worth stating:

- **`max_backlog` counts outstanding work**, not what one evaluation queued: queued, retrying, downloading and finalizing jobs of that podcast. Counting per evaluation would let every refresh top the queue back up to the limit, making it mean nothing.
- **An episode with no publication date is undated, not ancient.** An age limit therefore does not silently archive nothing from a feed that omits dates, and `sort_at` is not used as a stand-in because that would make the answer depend on when the podcast was added.

Precedence: global defaults, then the podcast's stored override field by field. `mode` is always the podcast's, which is what makes both directions work — one podcast archived automatically on an otherwise manual installation, and one held back on an otherwise automatic one.

## 10. Events

All persisted; none is transient.

| Event | Payload | Emitted when |
|---|---|---|
| `archive.registered` | `archive_file_id`, `path`, `size_bytes`, `hash_algo`, `hash_value` | a finished download became a record |
| `archive.verified` | `archive_file_id`, `path`, `depth`, `reason` | a pass found intact an artifact that was not `verified` before; one that stays intact is not announced |
| `archive.missing` | `archive_file_id`, `path` | the recorded file is gone |
| `archive.invalid` | `archive_file_id`, `path`, `depth`, `reason` | something is there but wrong |
| `archive.relocated` | `archive_file_id`, `from`, `to` | an artifact moved |
| `archive.source_changed` | `archive_file_id`, `path`, `old_url`, `new_url`, `old_length`, `new_length` | a refresh found the feed pointing at different audio for an archived episode (§8) |
| `archive.policy_queued` | `job_id`, `policy_version`, `priority` | the policy queued an episode |
| `archive.policy_skipped` | `policy_version`, `reason` | the policy passed one over |
| `archive.sidecar.written` | `archive_file_id`, `path` | a sidecar was written |
| `archive.manifest.written` | `path`, `entries` | a manifest was rewritten from the index |
| `archive.manifest.mismatch` | `path`, `changed`, `missing`, `added` | a check disagreed with the files |
| `archive.rebuild.completed` | `applied`, `scanned`, `rebuilt`, `unchanged`, `conflicts`, `unknown_episode` | a rebuild pass finished |
| `archive.imported` | `archive_file_id`, `path`, `confidence`, `matched_by` | one file was copied in |
| `archive.import.completed` | `applied`, `format`, and the counts | an import run finished |
| `archive.tagged` | `archive_file_id`, `path`, `mode`, `fields`, `size_bytes`, `hash_value` | tags were written |
| `archive.tags.skipped` | `archive_file_id`, `reason` | a file was left untagged |
| `podcast.artwork.fetched` | `artwork_id`, `path`, `format`, `size_bytes`, `hash_value` | artwork was stored |
| `podcast.artwork.unchanged` | `artwork_id` | the server said the stored image is current |
| `podcast.artwork.failed` | `url`, `reason` | artwork was refused or could not be fetched |

The rebuild and import events carry **counts, not lists**: a list of half a million paths would be a worse problem than the one it describes. Findings are named in the command's own report, up to a hundred. `archive.sidecar.invalid` and `archive.import.skipped` exist in `EventKind` but nothing emits them: an unreadable sidecar is a `malformed` finding of the rebuild report, and a file that was not imported is a line of the import report and a count in `archive.import.completed`.

A verification that could not complete emits nothing: "could not check" says nothing about the artifact, so it is logged rather than announced.

## 11. API

| Route | Purpose |
|---|---|
| `GET /api/v1/archive?state=&podcast=&source_changed=&limit=` | list artifacts, newest first |
| `GET /api/v1/archive/stats` | how many artifacts are in each state |
| `GET /api/v1/archive/missing` · `…/invalid` | the artifacts needing attention |
| `GET /api/v1/archive/{episode_id}` | one artifact (404 `archive_not_found`) |
| `POST /api/v1/archive/verify` | verify everything a filter selects |
| `POST /api/v1/archive/{episode_id}/verify` | verify one artifact |
| `POST /api/v1/archive/{episode_id}/path-preview` | what the template would produce |
| `POST /api/v1/archive/{episode_id}/relocate` | move it (`{"dry_run": true}` to look first) |
| `POST /api/v1/archive/reconcile?deep=` | register unrecorded downloads, then check |
| `GET /api/v1/archive/policies` | every stored per-podcast policy |
| `GET\|PUT\|POST\|DELETE /api/v1/podcasts/{id}/policy` · `POST …/policy/clear` | read, set or clear one policy |
| `GET /api/v1/archive/manifests` | every podcast's manifest state |
| `POST /api/v1/archive/manifests/write` · `…/{podcast_id}/write` | write stale manifests, or one |
| `GET /api/v1/archive/manifests/{podcast_id}/verify?rehash=` | compare a manifest with the archive |
| `POST /api/v1/archive/rebuild` | rebuild records from sidecars (dry run by default) |
| `POST /api/v1/archive/import` | read a foreign archive (dry run by default) |
| `GET /api/v1/archive/orphans` | what nothing owns under the media root (§21) |
| `POST /api/v1/archive/restore` | put missing files back from a folder (dry run by default, §22) |
| `POST /api/v1/archive/{episode_id}/redownload` | download a missing file again (§22) |
| `GET /api/v1/archive/{episode_id}/sidecar` · `POST …/sidecar/write` | read or write a sidecar |
| `GET /api/v1/archive/{episode_id}/tags` · `POST …/tags/write` | read or write tags |
| `GET /api/v1/archive/{episode_id}/media` | the archived bytes, with `Range` |
| `GET /api/v1/podcasts/{id}/artwork` · `…/artwork/image` · `POST …/artwork/fetch` | show, serve or fetch artwork |

**`import` and `restore` name a directory on the machine the server runs on**, which makes it as privileged as the server process — one reason Uguisu binds to localhost by default. It refuses the media directory as a source and only ever reads.

Every `GET` and `path-preview` works against a read-only media directory. Verification writes to the database alone. Bodies carry `schema: 1`; errors follow the shape in `library.rs`.

## 12. CLI

```console
uguisu archive list [--podcast <id>] [--state <state>] [--source-changed] [--limit <n>]
uguisu archive show <episode-id>
uguisu archive verify [<episode-id>] [--all] [--podcast <id>] [--full]
uguisu archive missing
uguisu archive invalid
uguisu archive path-preview <episode-id>
uguisu archive relocate [<episode-id>] [--all] [--podcast <id>] [--dry-run]
uguisu archive reconcile [--deep] [--rebuild [--apply] [--podcast <id>]]
uguisu archive policy show|set|clear|list …
uguisu archive sidecar show|write <episode-id>
uguisu archive manifest status|write [--podcast <id>]|verify --podcast <id> [--full]
uguisu archive import <path> [--format podgrab|generic] [--apply] [--podcast <id>] [--podgrab-db <file>]
uguisu archive orphans
uguisu archive restore <path> [--apply] [--podcast <id>]
uguisu archive redownload <episode-id>
uguisu archive artwork show|fetch <podcast-id> [--force]
uguisu archive tags show|write <episode-id> [--mode fill_missing|sync]
```

Each runs against the embedded engine, or against a running one with `--server`. `verify` exits **1** when it finds a missing or invalid artifact: a finding is a result the caller should act on, not a crash. `relocate` without a target is a usage error rather than a whole-archive move. `manifest verify` and `orphans` exit **1** on a finding for the same reason `verify` does. `import`, `restore` and `reconcile --rebuild` write nothing without `--apply`: both read trees Uguisu did not necessarily write, so the default has to be the one that cannot surprise anyone.

## 13. Configuration

| Variable | Default | Meaning |
|---|---|---|
| `UGUISU_ARCHIVE_TEMPLATE` | `{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}` | the path template |
| `UGUISU_ARCHIVE_PATH_PROFILE` | `portable` | `windows`, `posix` or `portable` |
| `UGUISU_ARCHIVE_VERIFY_ON_COMPLETION` | `true` | verify right after a download |
| `UGUISU_ARCHIVE_VERIFY_DEPTH` | `light` | how hard that check looks |
| `UGUISU_ARCHIVE_AUTO_DOWNLOAD` | `false` | queue discovered episodes without being asked |
| `UGUISU_ARCHIVE_MAX_BACKLOG` | `3` | episodes of one podcast awaiting archiving (0 = no limit) |
| `UGUISU_ARCHIVE_MAX_AGE_DAYS` | `0` | ignore episodes older than this (0 = no limit) |
| `UGUISU_ARCHIVE_PRIORITY` | `normal` | priority for jobs the policy creates |
| `UGUISU_ARCHIVE_SIDECARS` | `true` | write `<media file>.json` beside every artifact |
| `UGUISU_ARCHIVE_MANIFESTS` | `true` | maintain the per-podcast `manifest.sha256` |
| `UGUISU_ARCHIVE_ARTWORK_FETCH` | `false` | fetch podcast artwork after a refresh that found a new cover URL, or when none is stored |
| `UGUISU_ARCHIVE_ARTWORK_MAX_BYTES` | `16777216` | largest artwork image accepted |
| `UGUISU_ARCHIVE_TAG_MODE` | `fill_missing` | default mode for `archive tags write` |
| `UGUISU_ARCHIVE_IMPORT_MATCH_THRESHOLD` | `85` | percent an import match needs |

Sidecars and manifests are on because they only describe what Uguisu already holds. Artwork fetching is off because installing a release must not start making requests. The match threshold is an integer percentage rather than a fraction so that it stays exactly comparable and round-trips through `config show` unchanged.

An unparseable template is reported once at startup and every download keeps the identifier layout, rather than failing each enqueue in turn.

## 14. Known limitations

- **Relocation is same-filesystem only.** A media root spanning devices needs a manual move; Uguisu will not copy.
- **A light pass cannot see an in-place edit that preserved the length.** Use `--full` when that matters.
- **No automatic mass migration.** Files archived under an older template stay where they are until `archive relocate` is run, and nothing is tagged or given a sidecar retroactively without a command.
- **No retention or auto-deletion.** Nothing deletes media, by design — not old episodes, not superseded artwork, not leftovers from an interrupted run. Every one of those is kept and reported; the leftovers by `archive orphans` (§21).
- **Four tag fields cannot be embedded** and stay in the sidecar (§20).
- **Chapters and per-episode artwork are not written into files.** ADR 0012 lists them; v1 does not implement them. `lofty` exposes chapter frames only through its ID3v2-specific API and not at all for MP4, so chapters would work for MP3 and silently not for M4A; and `podcast:chapters` is a reference to a JSON document at a URL that nothing fetches. The reference stays in the feed data and in the sidecar (§16; ADR 0026, RISKS R5).
- **Transcoding, waveforms and audio analysis** are out of scope entirely.

## 15. The portable archive

Phase 5 left the database as the only index: lose it and the media files are anonymous bytes. Phase 6 writes two things beside the media so that is no longer true.

```text
<media>/
  .uguisu/                                   reserved; skipped by every scan
    manifests/<podcast-id>/manifest.sha256
    artwork/<podcast-id>/<sha256>.<ext>
    tmp/<id>.import · <id>.tagtmp            work in progress
  Show/2024/2024-01-05 - Folge 1.mp3         the artifact
  Show/2024/2024-01-05 - Folge 1.mp3.json    its sidecar
  <podcast-id>/.uguisu-tmp/<job-id>.part     Phase 4 scratch, unchanged
```

`.uguisu/` is anchored at the **media root**, not inside a podcast's directory, because there is no such thing as a podcast directory: where a podcast's files land is decided entirely by the template, which a user may point at `{episode.year}/{podcast.title}/…` or at a flat layout with no podcast directory at all. A manifest under the template's output would be stranded by the next template change; under the media root a template change only marks it stale. One reserved name also means one exclusion rule for every scan instead of one per asset kind.

That reservation needed a fix: `sanitize::segment` preserved a leading dot, so a podcast titled `.uguisu` rendered a segment that shadowed Uguisu's own bookkeeping. It is now escaped to `._uguisu`, on every profile, using the same inside-the-name idiom as the reserved-device-name escape so the result is stable on a second pass.

## 16. Sidecars

`<media file>.json`, derived from the final media name and never templated on its own — which is what makes it impossible to move one without the other. Copy one episode out of the archive and its metadata goes with it.

The document carries the podcast, the episode, the archive record and the provenance. The episode half includes the artwork URL and the chapter and transcript references (`url`, `mime_type`, and for a transcript `language` and `rel`) as the feed declared them; nothing behind them is downloaded. `reconcile --rebuild` does not read them: it restores archive records for episodes the library already has from the feed (§17). Two fields matter more than they look: the **feed URL** and the episode's **identity key**. After a total loss of the database the podcast is added again and the feed refreshed, so every identifier in every sidecar names something that no longer exists — and those two are what still point at anything.

- Written after the record exists and after the verification that follows it, so it describes bytes that are already registered and already checked.
- Rendered from the record, so there is exactly one place a sidecar can be wrong.
- A sidecar that could not be written is a warning and a degraded rebuild source. It is never a failed download: by that point the media file and its record are both durable.
- Unknown **fields** are ignored so a newer Uguisu's sidecars still read; an unknown **schema** is refused by name, because that is a statement this build cannot interpret.
- Reading is capped at 256 KiB: a rebuild reads whatever is on disk, and a scan of an untrusted archive must not become an out-of-memory because one file has a `.json` name.

## 17. Manifests and rebuilding

### The manifest

One `manifest.sha256` per podcast, in GNU coreutils' checksum format, so `cd <media directory> && sha256sum -c .uguisu/manifests/<podcast-id>/manifest.sha256` checks a podcast **without Uguisu**. Paths are relative to the media directory, which makes them byte-identical to `archive_files.relative_path`: nothing to rewrite when the template changes, and no `../../` chains to get wrong.

It is rendered **from the index**, not by re-reading the files, and its header says so where `sha256sum -c` skips it as a comment. Hashing while writing would prove nothing to the external tool the format exists for, would cost hours of disk I/O on every registration, and — the part that settles it — would have nowhere to record a disagreement it found. Reading the bytes is `archive verify --full`'s job, which already batches, streams and writes its findings onto the record.

Writing is **deferred**. A change marks the manifest stale inside the transaction that made the change, so the flag and the fact commit together and a crash can only leave "marked stale but actually fresh". The file is written when the event bus goes quiet for five seconds, on an explicit command, and on `close`. Writing one per finished episode would mean five hundred rewrites of a growing file for a five-hundred-episode backlog.

> **After a clean close, every manifest agrees with the index.** That is the guarantee worth remembering; between closes, `archive manifest status` says which have fallen behind.

### Rebuilding

`archive reconcile --rebuild` walks the sidecars and puts back records the database has lost. It resolves through two doors — the identifier, then the durable keys — because after a total loss the identifiers no longer exist.

A rebuilt record is always `unchecked` with reason `rebuilt`, and `origin = rebuild`. **A sidecar says what Uguisu knew, not what the bytes are.** A record that already carries a checked finding is reported as a conflict rather than overwritten, and `verify --full` is what turns a rebuilt archive into a verified one — including telling you about the file that was quietly edited while the database was gone.

Nothing is invented: a document whose media file is missing, that does not parse, that names an episode this library does not have, or whose path another episode owns is counted and named (up to a hundred, then counted only). Dry run is the default.

## 18. Importing a foreign archive

`archive import <path> --format podgrab` reads an archive another tool wrote. Four rules shape it:

1. **Copy, never move.** The source belongs to the user and is opened read-only; a test hashes the whole tree before and after every operation, including the one that copies out of it. There is no `--move`.
2. **An ambiguous file is never imported.** A file placed under the wrong episode is a quiet, permanent error that nobody notices until they play it; an unmatched file is a line in a report.
3. **Nothing is overwritten.** A target holding the same bytes is `already_present`; one holding different bytes goes through collision handling, and when the disambiguated path is taken too it is a reported conflict. An episode that is already archived keeps its record: a file with its bytes, current or as received, is `already_present`, any other is a conflict (ADR 0050).
4. **Nothing is deleted** — not unmatched files, not leftovers, not anything. The scratch copy an interrupted run leaves in `.uguisu/tmp/` is reported by `archive orphans` (§21).

One file per episode and one episode per path: files matching one episode import once when identical and are all conflicts when they differ, and a target planned for one file counts as taken for the next.

Matching is a deterministic, pure function: title (weight 40), publication date (25), season and episode number (15), duration (12) and declared size (8), **renormalized over the signals actually available on both sides**. Without that renormalization a feed carrying no season numbers would cap every score below the threshold and nothing would ever import. A match needs `UGUISU_ARCHIVE_IMPORT_MATCH_THRESHOLD` percent (85 by default) *and* a ten-point margin over the runner-up.

Two mechanisms do the hard work. The digits in a title are compared separately, because `Teil 1` and `Teil 2` are about 97% alike to any string metric. And two dates more than three days apart cap the total below any usable threshold, so a rerun titled identically to the original can never be matched on its title.

Before scoring, a file may already be named exactly: by Podgrab's database (`source_database`), when `--podgrab-db` is given and its row for the file carries a GUID or enclosure one episode has, or by an episode GUID in the file's own tags (`embedded_guid`), unless the scored match clearly names a different episode. Each media file's tags are read: the title tag and, for Podgrab names, the reading without a leading number are **fallback titles**, tried in order only when the name does not match clearly, and the audio's length fills a missing duration. Tag dates and track numbers are not used (ADR 0050).

An unresolved candidate duplicate (ADR 0051) carries its original's title and enclosure, so a file for either scores both alike and is `ambiguous`; resolve candidates before importing.

The podcast is decided once per **directory**, or per file where Podgrab's database names its feed: that bounds the memory an import needs to one podcast's episode index, and an unrecognisable directory is reported once rather than once per file inside it.

Execution copies into `.uguisu/tmp/` while hashing, then moves it into place without replacing anything (a hard link, then removing the scratch name; [ADR 0019](DECISIONS/0019-resume-and-finalization.md)) — so the target only ever holds a complete file, and the move stays inside one filesystem even when the source was on another disk. The record carries `origin = import` and `unchecked`/`imported`: the bytes were hashed as they were copied, which makes the record true, but nobody has read them back since.

Re-running converges. If the bytes are already at the template's path and only the record is missing — what a crash between the rename and the registration leaves — the run finishes the record instead of copying the file again under a suffixed name.

A new layout implements `SourceFormat` and gets an `ImportFormat` variant. Podgrab's naming, as its source writes it, is read in `podgrab.rs`; its database is read, never written, by `uguisu_storage::podgrab` (ADR 0050).

## 19. Podcast artwork

Artwork comes from a URL in a feed and ends up embedded in files other people's players parse, so it goes through the same HTTP stack as everything else untrusted — `Profile::Artwork`, with the SSRF policy, per-hop re-validation on redirects and a 16 MiB cap applied before a byte is read. There is no second URL validator, because a second one is a second thing to get wrong.

The **bytes** decide the format, then the declared media type, and never the extension. Only JPEG, PNG and WebP are stored. SVG is refused even when honestly declared: it is a document format that can carry script, and embedding one would hand a player a program rather than a picture. `application/octet-stream` counts as silence rather than as a claim, because a great many CDNs send it for ordinary images.

Storage is content-addressed at `.uguisu/artwork/<podcast-id>/<sha256>.<ext>`, so a replacement can never destroy the image before it, and a partial unique index — not a check that could race — keeps exactly one current row per podcast. A conditional request means an unchanged image costs one round trip and no bytes. Fetching is explicit and off by default; with `UGUISU_ARCHIVE_ARTWORK_FETCH` on, a refresh that stored a changed feed also fetches when the cover URL changed or no image is stored yet ([ADR 0047](DECISIONS/0047-artwork-on-refresh-when-asked.md)).

## 20. Writing tags

`archive tags write` writes Uguisu's metadata into an archived file. **Uguisu never modifies the only copy of a file**: the artifact is copied into `.uguisu/tmp/`, the copy is tagged, re-parsed and re-hashed, and only then renamed over the original. A file that does not already match its record is refused outright — tagging it would replace a detectable problem with an undetectable one.

Before the first write to a file, its managed tags are read and recorded as `original_tags` on the record, in the transaction that sets the `pending` marker, so the snapshot exists before a byte moves. Only the first write captures it: a later one, or a file Uguisu had tagged before the snapshot existed, leaves it alone. The cover is described (mime, size, SHA-256), not copied, and the sidecar carries the snapshot so it survives a rebuild. Nothing restores it automatically; it is the record of what a `sync` replaced ([ADR 0012](DECISIONS/0012-metadata-tagging.md)).

A write that fails, or finds nothing to change, removes its own copy. This is the one scratch file Uguisu removes, and only because it is never the only copy: the original was verified before it was copied and is untouched until the rename. Keeping it would leave a full copy of the file behind on every run of `sync` that changes nothing. Only a crash leaves a `.tagtmp`, and `archive orphans` reports it (§21).

Two modes, and both preserve everything Uguisu does not manage, by construction rather than by policy: a write starts from a clone of the tag already in the file and changes only the managed keys.

| Mode | Rule |
|---|---|
| `fill_missing` *(default)* | write a managed field only where the file has none |
| `sync` | make the managed fields match Uguisu; a field Uguisu has no value for is left alone |

Neither removes anything. ADR 0012's `overwrite` and `custom` stay unbuilt rather than half-built.

### After a crash

Between the atomic replace and the record update, "Uguisu retagged this" and "somebody else changed it" are indistinguishable from the bytes alone. So `tag_state = pending` is committed **before a single byte moves**. A crash cannot forge it, and recovery is bounded by a partial index that is empty whenever nothing is in flight:

- the bytes still match the record → the write never landed; the state goes back to `written` if an earlier write completed, `untagged` otherwise.
- the bytes differ → this is Uguisu's own work; the record adopts them with reason `retag_recovered`, `unchecked` until a full pass.

That second case is the one place in Uguisu where a hash is re-read to fit a file. It is deliberately narrow — only a row that said `pending` first — and it is never recorded as a verification.

### What cannot be embedded

A field belongs in the managed set only if it survives a write-then-read unchanged, because `sync` decides by comparing what it wants against what the file reports: a field that comes back different is rewritten on every run, and every rewrite moves the archive's hash for no change at all. A test writes every managed field to an MP3, a FLAC, an M4A, an Ogg Vorbis and an Opus file, reads it back, and writes again asserting the second write does nothing. Another does the same for the cover in all five.

Five candidates were removed by that test and stay in the sidecar, leaving thirteen managed fields:

| Removed | Why |
|---|---|
| ID3v2's `PCST` podcast flag | it made the **whole frame set** fail to encode, taking every other field with it |
| show name | written, not read back |
| year | ID3v2 keeps it in the same frame as the recording date, so the two overwrite each other |
| feed URL, enclosure URL | written, not read back |

The podcast description and the episode GUID are unreadable specifically in Vorbis comments, and MP4 has no atom for the publisher, so the capability table declares them unsupported there rather than dropping them everywhere. MP4 also stores no picture type: every cover comes back as `Other`, which is what Uguisu looks for there.

WAV and AIFF are refused **by policy, not by capability**: `lofty` would write to them, but those chunks hold none of the podcast fields and no podcast client reads them. An unsupported container is a per-file result, never a failed batch.

## 21. What `archive orphans` reports

Nothing in Uguisu deletes a file, so whatever an interrupted write, a hand-moved file or a lost record leaves behind stays on disk. `uguisu archive orphans` (`GET /api/v1/archive/orphans`) is where it shows up ([ADR 0051](DECISIONS/0051-candidate-duplicates-and-orphans.md)). It reads only, walks the whole media root and is never run at start-up.

| Group | What it is | Left by |
|---|---|---|
| `leftovers` | every file in `.uguisu/tmp/`, and files named by `tmp_suffix` (`<target>.<pid>.<n>.tmp`) under `.uguisu/artwork/`, `.uguisu/manifests/` and in the tree | a crash or an error during an import (`<id>.import`), a restore (`<id>.restore`, §22) or a manifest, sidecar or artwork write, and a crash during a tag write (`<id>.tagtmp`, §20) |
| `orphan_parts` | `.part` files under a `.uguisu-tmp/` directory whose name is no download job's | a job deleted with the database, or a copied media directory |
| `unknown_media` | media files no archive record names | a lost database, a file moved or copied in by hand |
| `stray_sidecars` | `<name>.<media extension>.json` whose media file is not beside it | a relocation, which renders the sidecar afresh at the new path, or a hand-moved file |
| `unreadable` | links and directories the walk did not enter | a symlink (never followed) or a permission error |

Each group counts everything and names the first hundred paths, relative to the media root; `clean` is true when all are empty, and the CLI then exits 0, otherwise 1. A media root that does not exist yet holds nothing and is clean.

Three rules keep it from naming work in progress:

- A writer holds its scratch file's name (`layout::Scratch`) from before the file exists until it is renamed or abandoned, and a held file is not a leftover. One process owns a data directory, so no other writer can be running.
- A file is judged after it was seen: the record of a media file is looked up when the walk reaches it and once more at the end, and the job ids are read after the `.part` files were listed. A download or an import that finishes during the walk is not reported.
- A user's own `notes.json` or `draft.tmp` is not Uguisu's: only the names Uguisu writes are judged.

What to do with a finding is the person's decision: delete a leftover by hand, move a file back, `reconcile --rebuild` from its sidecar, or import it from outside the media root. Paths are compared exactly, so on a filesystem that ignores case or normalizes Unicode a file written under another spelling can show up as unknown; the walk goes twelve levels deep.

## 22. Repairing a missing file

A record marked `missing` keeps its hash, so the archive knows exactly which bytes it lost. Two repairs put them back, each only when asked for ([ADR 0060](DECISIONS/0060-repairing-a-missing-file.md)). Neither replaces a file.

| | `archive restore <folder>` | `archive redownload <episode>` |
|---|---|---|
| Source | a folder on the machine Uguisu runs on, only read | the episode's current primary enclosure |
| Acts on | every `missing` record, or one podcast's | one episode whose record's path holds nothing |
| Match | the record's `hash_value` as SHA-256, nothing else | none: the new bytes become the record's |
| Default | a dry run; `--apply` copies | queues at once |
| Ends with | a full verification of each restored file | the download's registration and its verification |

A restore looks at the files in the folder, up to twelve levels deep, whose extension a missing record has and whose size equals one, or the size it had before a tag write. It hashes only those, each at most once. Each record gets one line:

| Action | Meaning |
|---|---|
| `restore` | a file with the record's bytes is in the folder; with `--apply` it was copied back and verified |
| `returned` | the record's bytes are already at its path again, in a plain file; with `--apply` they are verified |
| `source_only` | a file has the bytes as they were downloaded, before a tag write, whatever their size now; it is not used |
| `taken` | something else is at the record's path; it is kept, and a copy made while it appeared stays as a leftover (§21) |
| `changed` | the file in the folder changed while it was copied; the copy stays as a leftover (§21) and the record stays `missing` |
| `not_found` | no file in the folder has the record's bytes |
| `failed` | the record's path could not be checked, the copy failed, or the file is back but a full check did not find the record's bytes; the detail says which, and the rest of the run goes on |

A copy goes to a scratch file, is hashed on the way, and lands with a rename that never replaces. The folder is opened like an import's source (§18): links are not followed, and the media directory is refused.

A redownload is refused while anything is at the record's path, including a file Uguisu cannot read or a path the download engine does not resolve (a `:` the posix profile allowed), and while the old download's `.part` name holds a file: a finalization that could not take that name back left a second link to the archived bytes there. A completed job is queued again with the reason `redownload`. A record an import or a rebuild made gets a new job. The target is rendered from the template as for any download, unless another episode's record holds that path, and the registration moves the record to where the file landed, keeping its id. If the original came back to the record's path meanwhile, the registration is refused: the record stays with it, and the new file is left for `archive orphans` to report. A crash between the download and its registration is repaired at the next start, like a first download's. The new bytes may differ from the lost ones, since hosts re-encode and insert ads; restore is the repair that keeps the bytes.

An import that finds a missing record's bytes still copies nothing: it reports `already_present` and points at `archive restore`.
