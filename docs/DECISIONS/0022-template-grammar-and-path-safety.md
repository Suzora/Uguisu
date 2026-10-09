# ADR 0022 — Template grammar, sanitization and path safety

**Status:** accepted, amended 2026-10-04 · **Date:** 2026-09-18 · **amends** [ADR 0009](0009-path-templates.md) · supersedes the interim layout of [ADR 0020](0020-id-based-media-paths.md)

## Context

ADR 0009 specified a template language and left it to Phase 5 to build. ADR 0020 shipped an interim identifier layout so Phase 4 could download at all. Implementing the language raised three questions ADR 0009 had answered provisionally or not at all: who owns the extension, what happens when two episodes render to one path, and how containment is actually proven.

Every part of this is security-relevant: the values come from feeds, which are attacker-controlled text.

## Decision

### The grammar is ADR 0009's, in full

Dotted variables, `|filters`, `[optional groups]`, literals, and a strftime subset on `episode.published`. Seventeen variables — ADR 0009's list plus `podcast.id` and `episode.id`, which give a template a deterministic unique piece. No aliases and no flat alternative syntax: one grammar, documented once.

### The renderer owns the extension and the fallback

The template writes `.{extension}`, but the renderer splits it off, sanitizes and truncates the stem, appends any collision suffix, and adds the extension back. So it is never doubled, never the part truncation cuts, and present even when a template omits it. A file name that renders to nothing but punctuation becomes the episode id.

### Collisions use a stable suffix, not a counter

**This amends ADR 0009**, which specified ` (2)`, ` (3)` by identity order. A counter depends on the order in which downloads happen to finish, so the same episode could land on a different path after a restart or a re-scan — which contradicts the determinism ADR 0009 itself demands.

Instead: ` [xxxxxx]` before the extension, where the six characters are the **tail** of the episode's ULID. The tail is the random part; the leading characters encode the timestamp, so episodes added in the same millisecond would share them. The choice is identical on every run and on every machine. A path held by a file Uguisu has no record of is avoided, never overwritten, and a preferred path whose suffixed form is also taken is reported rather than guessed at a third time.

### Containment is component-wise and symlink-aware

Never a string prefix: `/media/podcasts-evil` starts with `/media/podcasts` as text and is a different directory. Paths are compared component by component, and the deepest existing ancestor is canonicalized first, so a symlinked directory planted inside the archive cannot point out of it. The same check runs on a rendered path, a stored path, a relocation target and a path proposed to the download queue.

### Sanitization is per segment, profile-driven and idempotent

`windows`, `posix`, `portable` (the default, the intersection). Separators, control, bidi and zero-width characters, the Windows character set, reserved device names, trailing dots and spaces, drive-letter prefixes, NFC normalization, and deterministic length limits (120 characters per segment, 200 per path). *(Amended 2026-10-04: and 200 UTF-8 bytes per segment. Linux allows 255 bytes per name, and 120 characters of CJK text are 360; such a name could not be created there.)*

Idempotence is a requirement, not an accident: a device name is escaped *inside* the name (`nul.mp3` → `nul_.mp3`), never at the end, so a second pass sees a name that is no longer reserved and changes nothing. Without this, a path read back from the database would drift each time it was checked.

### New downloads land on the rendered path; old files move only on request

`uguisu-download` asks a `DestinationResolver` the engine supplies and checks the answer; anything unusable, unrenderable or already claimed by another job falls back to the ADR 0020 identifier layout, which needs nothing but identifiers and is therefore always available. That fallback is reported, never fatal.

Files archived before a template change stay where they are. `archive relocate` moves them, explicitly, with `--dry-run` to look first.

## Consequences

- Guarantee: a rendered path is relative, stays under the root, has no traversal component, and renders identically twice — property-tested against traversal, mixed separators, absolute and UNC forms, drive letters, device names, NUL and bidi overrides, full-width confusables and 200-character values.
- A value can never introduce a path component, because segments are sanitized individually and `/` is only ever a literal between template parts.
- Rendering touches no filesystem, so `path-preview` is safe on a read-only archive and before anything is downloaded.
- Adding a variable or a filter is a code change with tests. Accepted: a template language that can be extended at runtime is a template language that can escape its sandbox.
- A stable suffix is slightly less pretty than ` (2)`, and two episodes whose ids share a six-character tail *and* a rendered name produce a reported conflict rather than a third guess. Both are acceptable next to paths that never move on their own.

## Alternatives considered

- **Keep ADR 0009's counter.** Rejected above: order-dependent, so not deterministic across runs.
- **A content hash as the suffix.** Stable, but unknown until the download finishes, so a path could not be chosen at enqueue time.
- **Full ULID as the suffix.** Unambiguous but 26 characters of noise in every colliding name; six characters of the random tail is enough to separate two episodes that already collide on title and date.
- **A general template engine (Tera, minijinja).** Loops, conditionals and filters that can emit separators make containment and determinism much harder to prove; a sandboxed subset would be most of this work anyway.
- **Canonicalizing the whole path before every operation.** Requires the path to exist, which it does not when a destination is being chosen. Canonicalizing the deepest existing ancestor gives the same guarantee for the part that could hide a symlink.
- **Moving existing files automatically when the template changes.** A configuration edit would rewrite an entire archive without being asked. Rejected in favour of an explicit command.
