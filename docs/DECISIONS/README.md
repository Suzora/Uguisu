# Architecture Decision Records

Decisions that shape Uguisu, in the MADR-light format: context, decision, consequences, alternatives considered. A decision is changed by a new ADR that amends or supersedes the old one, and both status lines say so; an old ADR is otherwise edited only to correct a statement about the system that is no longer true.

| # | Title | Status |
|---|---|---|
| [0001](0001-workspace-layout.md) | Cargo workspace layout and crate boundaries | accepted |
| [0002](0002-sqlite-storage.md) | SQLite via sqlx as the only v1 store | accepted, amended (Phases 3, 4), amended by 0016, 0018, 0056 |
| [0003](0003-single-binary-cli-over-api.md) | One `uguisu` binary; CLI commands run over the API | accepted, amended (Phases 2, 3), amended by 0016 |
| [0004](0004-web-ui-and-desktop-stack.md) | Svelte 5 + Vite + TypeScript SPA served by axum; Tauri 2 desktop | accepted, amended by 0031, 0034, 0041 |
| [0005](0005-discovery-provider-strategy.md) | Discovery provider strategy | accepted, amended (Phases 2, 10) |
| [0006](0006-episode-identity.md) | Episode identity and deduplication | accepted, amended by 0014, 0051 |
| [0007](0007-integrity-hashing.md) | SHA-256 as canonical integrity hash | accepted |
| [0008](0008-feed-parser.md) | Own feed parser on quick-xml | accepted, amended (Phase 3), amended by 0017 |
| [0009](0009-path-templates.md) | Custom deterministic path template language | accepted, amended by 0022 |
| [0010](0010-event-system.md) | In-process event bus with persisted event log | accepted, amended (Phases 3, 4), amended by 0016 |
| [0011](0011-http-client-policy.md) | Single HTTP client crate with SSRF policy | accepted, amended (Phases 2, 4) |
| [0012](0012-metadata-tagging.md) | Metadata tagging via lofty with format capabilities and policies | accepted, amended (Phase 10), amended by 0026 |
| [0013](0013-auth-and-network-binding.md) | Authentication and network binding model | superseded in part by 0035, 0036, 0037, amended by 0061, 0062 |
| [0014](0014-identity-fallback-and-guid-changes.md) | Identity fallback and GUID changes | accepted, amends 0006, amended by 0051 |
| [0015](0015-change-log-instead-of-versioning.md) | Episode change log instead of row versioning | accepted, amended (Phase 10), amended by 0055 |
| [0016](0016-refresh-coalescing-lock-and-post-commit-events.md) | Refresh coalescing, single-process lock and post-commit events | accepted, amended (Phase 10), amends 0002, 0003, 0010 |
| [0017](0017-partial-parse-failures-and-conservative-removal.md) | Partial parse failures and conservative removal detection | accepted, amends 0008 |
| [0018](0018-download-state-machine-and-queue.md) | Download state machine and persistent queue | accepted, amends 0002, amended by 0060 |
| [0019](0019-resume-and-finalization.md) | Validated resume and atomic finalization | accepted, amended 2026-10-04 |
| [0020](0020-id-based-media-paths.md) | Identifier-based media paths for Phase 4 | accepted (interim), layout superseded by 0022 |
| [0021](0021-archive-file-and-verification.md) | The archive record and what verification may do | accepted, amended 2026-10-10 |
| [0022](0022-template-grammar-and-path-safety.md) | Template grammar, sanitization and path safety | accepted, amended 2026-10-04, amends 0009, supersedes 0020's interim layout |
| [0023](0023-archive-policy.md) | The automatic archive policy | accepted |
| [0024](0024-sidecars-and-manifests.md) | Sidecars, manifests and where they live | accepted, amended by 0051 |
| [0025](0025-rebuild-and-import.md) | Rebuilding the index, and importing someone else's archive | accepted, amended by 0050 |
| [0026](0026-tagging-and-artwork.md) | Writing tags, and fetching artwork | accepted, amends 0012, amended by 0047 |
| [0027](0027-service-and-refresh-scheduler.md) | The service, and the refresh scheduler | accepted |
| [0028](0028-persisted-settings-and-precedence.md) | Persisted settings, and what wins | accepted, amended (Phase 10, 2026-10-04) |
| [0029](0029-local-search-index.md) | Local search over the library | accepted |
| [0030](0030-discovery-persistence.md) | Discovery that outlives the process | accepted |
| [0031](0031-serving-the-web-ui.md) | Serving the built web UI from a directory | accepted, amends 0004, amended by 0061 |
| [0032](0032-media-and-artwork-over-http.md) | Archived media and artwork over HTTP | accepted |
| [0033](0033-sse-reconnection.md) | One event stream per tab, and what a reconnect means | accepted |
| [0034](0034-web-ui-architecture.md) | The web UI's layers, and a typed adapter before a generated client | accepted, amends 0004, amended by 0039 |
| [0035](0035-credentials-sessions-and-tokens.md) | One credential, server-side sessions, hashed tokens | accepted, supersedes in part 0013, amended by 0042, 0062 |
| [0036](0036-three-access-levels.md) | Three access levels, classified in one place | accepted, supersedes in part 0013 |
| [0037](0037-exposure-gate.md) | A network bind without a credential is a startup failure | accepted, supersedes in part 0013, amended by 0058, 0061 |
| [0038](0038-one-error-envelope.md) | One envelope on every failure, one kind vocabulary | accepted |
| [0039](0039-openapi-from-the-types.md) | OpenAPI derived from the types, drift-checked in CI | accepted, amends 0034 |
| [0040](0040-one-cursor-contract.md) | One cursor contract for every page | accepted, amended by 0057 |
| [0041](0041-desktop-shell-and-embedded-server.md) | The desktop shell embeds the server and grants one origin | accepted, amends 0004 |
| [0042](0042-launch-credential-exchange.md) | A per-launch token, exchanged once for an ordinary session | accepted, amends 0035 |
| [0043](0043-desktop-packaging.md) | Packaging: six formats, one version, and a gate that installs them | accepted, amended by 0046, 0064 |
| [0044](0044-archive-folder-and-flatpak.md) | The archive folder is the desktop's setting, and the Flatpak stays narrow | accepted |
| [0045](0045-uguisu-part-of-suzora.md) | Uguisu, part of Suzora | accepted |
| [0046](0046-ci-within-a-minutes-budget.md) | CI within a minutes budget | accepted, amends 0043, amended by 0061, 0064, superseded in part by 0063 |
| [0047](0047-artwork-on-refresh-when-asked.md) | Artwork on refresh, when asked | accepted, amends 0026 |
| [0048](0048-agpl-licence.md) | Licence: AGPL-3.0-or-later | accepted |
| [0049](0049-opml-import-and-export.md) | OPML import and export | accepted |
| [0050](0050-podgrab-migration.md) | Podgrab migration: its real names, its database and the tags of foreign files | accepted, amends 0025, amended by 0059 |
| [0051](0051-candidate-duplicates-and-orphans.md) | Resolving candidate duplicates, and reporting what nothing owns | accepted, amends 0006, 0014, 0024 |
| [0052](0052-moving-a-feed-by-hand.md) | Moving a feed by hand | accepted |
| [0053](0053-message-catalogue.md) | A message catalogue in the web UI | accepted |
| [0054](0054-upgrade-fixtures.md) | Upgrade fixtures: what a tagged build left, opened by every later one | accepted |
| [0055](0055-archiving-and-removing-a-podcast.md) | Archiving and removing a podcast | accepted, amends 0015 |
| [0056](0056-database-maintenance.md) | Database maintenance: migrate, back up, check, vacuum | accepted, amends 0002 |
| [0057](0057-library-paged-by-the-server.md) | The library list is filtered, sorted and paged by the server | accepted, amends 0040 |
| [0058](0058-refusing-cross-site-changes.md) | A change sent from another site is refused | accepted, amended 2026-10-09, amends 0037 |
| [0059](0059-archive-import-in-the-web-ui.md) | Importing an archive from the web UI: a server path, typed | accepted, amends 0050 |
| [0060](0060-repairing-a-missing-file.md) | Repairing a missing file: the exact bytes, or a new download | accepted, amends 0018 |
| [0061](0061-the-docker-image.md) | The Docker image | accepted, amends 0013, 0031, 0037, 0046 |
| [0062](0062-trusted-proxies.md) | Trusted proxies name the client, and nothing else | accepted, amends 0013, 0035 |
| [0063](0063-ci-within-five-minutes.md) | Every pull request's CI within five minutes | accepted, supersedes in part 0046 |
| [0064](0064-linux-floor-in-a-container-and-every-update-tested.md) | Packaging: the Linux floor is a container, and every update is tested | accepted, amends 0043, 0046 |

Template: copy `0001` and keep the four sections. Number sequentially; never reuse a number.
