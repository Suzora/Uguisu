# ADR 0014 — Identity fallback and GUID changes

**Status:** accepted, amended by [ADR 0051](0051-candidate-duplicates-and-orphans.md) · **Date:** 2026-09-17 · amends [ADR 0006](0006-episode-identity.md)

## Context

ADR 0006 fixes the identity cascade (GUID → enclosure URL → fingerprint) and says that items whose key differs but whose secondary signals match are recorded as candidate duplicates. Real feeds change in two very different ways: a host that adds GUIDs to a feed that never had them (or drops them, or moves the media to a new CDN) still describes the *same* episodes, while a feed whose GUIDs are rewritten (`v-1` → `https://…/one-new-guid`) may or may not. Treating both as candidate duplicates would double every episode of a feed that merely gained GUIDs; merging both would silently fuse distinct episodes that happen to share a title and length.

## Decision

When an incoming item's cascade key matches no stored `identity_key`, the planner looks the item up by its secondary signals in order: stored `guid_key`, `enclosure_key`, `fingerprint_key`.

- **No contradiction → same episode.** A match counts when the two identities do not contradict each other: at most one side carries a GUID (stored `url:`/`fp:` vs incoming `guid:`, or vice versa), or the enclosure URL is the same. The stored `identity_key` is kept, the signal columns are refreshed with the incoming values, the change is logged (`episode_changes.field = identity_signals`, `episode.updated`) and the report warns. On the next refresh the item matches by signals again and is treated as unchanged/updated like any other.
- **Contradiction → candidate duplicate.** When both sides carry different non-empty GUIDs, or both identities are enclosure-based on different URLs and only the fingerprint agrees, the item is inserted as a new episode with `duplicate_of_episode_id`, `duplicate_reasons` from `probable_same_episode` (same enclosure URL, same fingerprint, same publish minute, same title, same media length), `archive_state = skipped` and `skip_reason = duplicate of <id>`, and `episode.identity_ambiguous` fires. The stored episode is untouched and becomes "missing" for removal detection like any other absent item.
- Nothing is ever merged or deleted automatically. Resolving candidates is a user action ([ADR 0051](0051-candidate-duplicates-and-orphans.md)); Phase 4's policies must treat `skipped` candidates as not-to-download until then.

## Consequences

- Feeds that gain or lose GUIDs, or move media between hosts, keep one episode per item and keep their archive state.
- Rewritten GUIDs produce visible candidates rather than silent fusion; a feed that rewrites every GUID at once produces one candidate per episode — noisy but honest, and the identity notes in the change log explain each one.
- The identity index (`episodes::index`) must load every stored episode's keys per refresh (one query, hash maps); at 10 000 episodes this costs ≈ 30 ms.

## Alternatives considered

- **Always candidate duplicates (ADR 0006 read literally):** doubles feeds that add GUIDs later — the most common real-world change.
- **Always merge on any secondary match:** fuses re-published or retitled episodes that share a title and length; forbidden by the brief's "never delete/merge silently".
- **Ask the user at refresh time:** refreshes are unattended.
