# Uguisu — Product Definition

> **"I search for a podcast, Uguisu finds the right feed, and from there my archive just takes care of itself."**
>
> **"My podcast collection is safe with Uguisu."**

Uguisu, a podcast downloader and archive manager by **Suzora**, is a self-hosted **podcast archive engine**: it discovers podcasts, resolves the authoritative RSS feed, ingests it, downloads every episode the user wants, preserves the original media, normalizes metadata, organizes files deterministically and keeps proving that the archive is complete and healthy.

It is **not** another podcast player. Playback is a convenience, never the reason Uguisu exists.

Uguisu combines **consumer-grade discovery** (nobody has to know what RSS is) with **professional-grade archival** (every file is accounted for, hashed and explainable).

---

## 1. Who Uguisu is for

People who care about at least one of:

| Need | What it means for Uguisu |
|---|---|
| Long-term archival | Files stay meaningful without Uguisu. Sidecar metadata, deterministic names, no opaque blobs. |
| Original quality | Never transcode by default. The publisher's bytes are the archive. |
| Reliability | Downloads resume, survive restarts, never expose partial files. |
| Huge libraries | Thousands of podcasts and hundreds of thousands of episodes without loading everything into memory. |
| Deterministic organization | Same input + same config = same path, every time, on every OS. |
| Excellent metadata | Clean ID3 / MP4 / Vorbis / Opus tags, podcast artwork, with explicit write policies; the feed's chapter, transcript and episode-artwork references in the sidecar. |
| Aggressive customization | Global defaults, per-podcast overrides, path templates, archive policies. |
| Automation | CLI + API for everything the UI can do. |
| Recoverability | Rebuild the archive records from the files' sidecars once the feeds are added again; import an existing archive. |
| Integrity | Cryptographic hashes, verification runs, repair/reconcile mode. |
| Local ownership | Self-hosted; Docker, native Windows, native Linux/Flatpak. |
| Effortless discovery | Fuzzy search across directories; no RSS URL required. |

Secondary audiences: home-lab operators migrating from Podgrab, archivists preserving shows that may disappear, and power users scripting bulk operations.

## 2. Product philosophy

These principles are the tie-breakers for every design decision. They are listed in priority order.

1. **Archive-first.** Downloaded media is an archive, not a cache. The filesystem must remain usable and understandable if Uguisu is deleted.
2. **Original quality first.** No silent transcoding. Optional conversion (later) always produces a *derived* file and never replaces the original.
3. **Deterministic.** Same metadata + same configuration ⇒ same filenames, directories and tags. Every generator is a pure, testable function.
4. **Recoverable.** The database is a rebuildable index. Sidecar files (a `<media file>.json` beside every archived file, a `manifest.sha256` per podcast) carry enough truth to reconstruct its archive records once the feeds are added again ([ADR 0025](DECISIONS/0025-rebuild-and-import.md)).
5. **Observable.** For every episode a user can answer: why was it downloaded, why was it skipped, why did it fail, is the file healthy, did metadata change, is the archive complete.
6. **Fast.** Asynchronous I/O, bounded concurrency, indexed queries, measured (not claimed) performance.
7. **Configurable.** Global defaults, per-podcast overrides, environment variables, stored settings, UI; a config file is not built. Validation with actionable errors.
8. **Discovery-first for humans.** RSS is the source of truth and is hidden from people who do not care about it. Advanced users keep full control.
9. **Never silently destroy user data.** Deletion, overwriting and merging are explicit, auditable actions.

## 3. Core concept

Uguisu separates four concerns that most existing tools blur together:

```text
Discovery   → find candidate podcasts for a human query        (uguisu-discovery)
Resolution  → turn a candidate / URL into a verified feed URL   (uguisu-discovery)
Ingestion   → fetch + parse the feed, detect episodes           (uguisu-feed)
Archival    → download, verify, tag, organize, maintain         (uguisu-download, -archive, -metadata)
```

Once a podcast has been resolved to a feed, the archival side behaves exactly as if the user had pasted the RSS URL. Discovery providers can be down, rate-limited or removed without affecting a single existing subscription.

Searching never subscribes. The flow is always *search → inspect → select → add*.

## 4. Non-goals (v1)

Explicitly out of scope so that the core stays focused:

- A streaming-first player UI, playlists, listening statistics, social features.
- Multi-user accounts with per-user subscriptions (single admin identity in v1; the auth model leaves room for more).
- Transcoding pipelines, loudness normalization, silence trimming.
- Hosting or publishing podcasts.
- gpodder sync server (PinePods/PodFetch cover this well; Uguisu imports and exports OPML instead).
- Cloud storage backends (S3 etc.) — designed for, not built in v1.
- AI transcription / summarization — designed for, not built in v1.
- Web pages for archive maintenance: reconciliation, relocation and its path preview, tag, sidecar and manifest writing, showing a sidecar or a file's tags, manifest verification, artwork fetching, the orphan report, the list of files whose source changed, and database backup, check and vacuum. The CLI and the API do all of them.

## 5. Why someone migrates to Uguisu

The question every phase must keep answering: *"Why would somebody move from Podgrab / PinePods / PodFetch / gPodder to Uguisu?"*

| Differentiator | What the user gets | Where competitors stop |
|---|---|---|
| **Discovery without RSS** | Type "darknet diaries", get ranked, deduplicated results from several directories, one click to add, feed resolved and verified automatically. Explainable ranking. | Most tools offer a single-directory lookup (usually iTunes) or require pasting a feed URL. |
| **Archive integrity** | Every file has size + SHA-256 + source URL + GUID recorded. `uguisu archive verify` reports `verified` / `missing` / `invalid` (size or hash mismatch, …). A missing file is restored with its exact bytes from a backup folder, or downloaded again. | No competitor hashes files or can tell you the archive is incomplete. |
| **Superior metadata** | Format-aware tagging (ID3v2.4, MP4, Vorbis, Opus), podcast artwork, explicit modes (`fill_missing`, `sync`; neither removes a tag Uguisu does not manage), sidecar JSON carrying the episode artwork URL and the chapter and transcript references. Downloading episode artwork, chapters and transcripts comes after v1. | Tagging is absent, partial or destructive. |
| **Deterministic file organization** | Template language with variables, filters, optional segments and OS-specific sanitization profiles; identical results on Windows and Linux; collision handling. | Fixed layouts or a single naming switch. |
| **Performance at scale** | Designed for 10k feeds / 100k+ episodes: pagination, streaming, incremental scans. Benchmarked up to 100 000 episodes; the 10 000-podcast benchmark is not run yet. | UIs that load everything; refreshes that hit every feed at once. |
| **Recovery** | Resume partial downloads, survive crashes/restarts, rebuild the archive records from sidecars, import existing archives (Podgrab layouts included) without re-downloading. | Partial files left behind, duplicates after restarts, no import. |
| **Transparency** | Every step is an event — discovered → policy evaluated → queued → attempts → completed → verified, with reasons for every skip and failure — on `GET /api/v1/events`; a per-episode timeline view is not built. | Logs at best. |
| **Automation** | Everything in the UI exists in the CLI (`--json`) and the versioned REST API with OpenAPI docs. An event stream; webhooks are post-v1. | Partial APIs, UI-only settings. |
| **Migration** | OPML import and export, existing-archive import with explained matching (an ambiguous file is reported, never placed; `--podcast` names a folder's podcast by hand), feed-URL change detection without duplicate podcasts, `podcast move-feed`. | Manual re-subscribe and re-download. |

## 6. v1 Definition of Done

A user can:

1. Search for a podcast using normal language.
2. Find podcasts without knowing RSS.
3. Use fuzzy search (typos, word order, partial titles). No directory provider is fuzzy, so a typo is recalled by relaxing the query when a search finds nothing ([`DISCOVERY.md`](DISCOVERY.md) §7).
4. Compare multiple discovery results, with provenance shown.
5. Resolve a selected podcast to its RSS feed automatically.
6. Add podcasts using an RSS URL directly.
7. Add podcasts using a website URL.
8. Import podcasts from OPML.
9. Refresh feeds automatically on a jittered schedule.
10. Discover new episodes.
11. Download episodes concurrently with global and per-host limits.
12. Resume interrupted downloads.
13. Recover from restarts and crashes without duplicate or partial files.
14. Apply filename/path templates.
15. Apply metadata with a chosen policy.
16. Download and manage podcast artwork (embedded + sidecar). The episode artwork URL and the chapter and transcript references are kept with the episode and in its sidecar; downloading episode artwork, chapters and transcripts comes after v1.
17. Organize media deterministically.
18. Import an existing archive.
19. Detect duplicates (explained, never auto-deleted).
20. Detect missing files.
21. Verify archive integrity.
22. Reconcile database and filesystem state.
23. Configure per-podcast archive policies.
24. Manage the library, downloads, the archive's state, an archive import and the repair of missing files through the web UI. The maintenance commands stay CLI and API only in v1 (§4).
25. Automate everything important using CLI/API.
26. Run Uguisu on Docker, from an image built from the checkout ([`DOCKER.md`](../DOCKER.md)); no image is published to a registry in v1.
27. Run Uguisu natively on Windows.
28. Run Uguisu natively on Linux and as a Flatpak.

Plus the release gates from the brief (§53, Phase 11): no known data-loss bugs, benchmarks completed, threat model reviewed, all three packages tested, recovery scenarios tested, upgrade tested, an archive with tens of thousands of episodes verified, and documentation good enough that a new user installs Uguisu without reading source code.

## 7. Product constraint

Uguisu is optimized for **being trusted with the archive**, not for having the most features. When a feature request conflicts with principle 1 (archive-first) or principle 9 (never silently destroy data), the principle wins.

---

*Status: Phase 1 definition; what is not built is marked where it is listed. Open question: whether OPML import and export, both built, stay the only "sync" story in v1 (leaning yes). A minimal player ships: the web UI plays an archived episode in a native `<audio>` bar.*
