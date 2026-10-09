# ADR 0006 — Episode identity and deduplication

**Status:** accepted, amended by [ADR 0014](0014-identity-fallback-and-guid-changes.md), [ADR 0051](0051-candidate-duplicates-and-orphans.md) · **Date:** 2026-09-17

## Context

Feeds rotate GUIDs, reuse GUIDs, change enclosure URLs and filenames, re-upload media and republish old items. Identifying episodes by title or filename produces duplicates and lost archive state.

## Decision

- Each feed item gets a stored `identity_key` computed by a fixed cascade: unique non-empty GUID → unique normalized enclosure URL → fingerprint hash of (normalized title, publish day, enclosure length). The chosen signals are stored in `fingerprint`.
- `(podcast_id, identity_key)` is unique. Later items with the same key update metadata in place.
- Items whose key differs but whose secondary signals (enclosure URL, fingerprint, or the downloaded content hash) match an existing episode are recorded as **candidate duplicates** with reasons and surfaced in the UI; they are never merged or deleted automatically.
- Episodes removed from the feed keep their archive state (`removed_from_feed_at`).

## Consequences

- Deterministic and explainable: the UI can show "identified by GUID" or "identified by enclosure URL because GUIDs are duplicated in this feed".
- Some feeds will produce visible duplicate candidates that the user resolves once; this is preferred over silent merges.
- Changing the cascade later requires a migration that recomputes keys; keep the raw signals stored.

## Alternatives considered

- **GUID only:** breaks on ~5–10 % of real feeds (empty or duplicated GUIDs).
- **Title + date only:** collides on retitled and re-published episodes.
- **Automatic merge on content-hash match:** safe for identical bytes, but re-encoded uploads would still duplicate; and the brief forbids silent deletion — so even hash matches are surfaced.
