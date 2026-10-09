# ADR 0009 — Custom deterministic path template language

**Status:** accepted, amended by [ADR 0022](0022-template-grammar-and-path-safety.md) · **Date:** 2026-09-17

## Context

Users must define arbitrary nested layouts (`{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}`), with Windows/Linux/macOS-safe sanitization, handling of empty values, reserved names, Unicode, length limits and collisions. Output must be deterministic and testable, and must never be able to escape the archive root.

## Decision

A small purpose-built template language in `uguisu-archive`:

- Variables: `{podcast.title}`, `{podcast.author}`, `{episode.title}`, `{episode.guid}`, `{episode.identity}` (short hash), `{episode.date}` (ISO), `{episode.year}`, `{episode.month}`, `{episode.day}`, `{episode.season}`, `{episode.number}`, `{episode.type}`, `{episode.duration}`, `{episode.published:%Y-%m-%d}` (strftime subset), `{extension}`.
- Filters: `{episode.title|truncate:80}`, `|lower`, `|upper`, `|pad:3` (numbers), `|slug`, `|default:"Unknown"`, `|ascii`.
- Optional segments: `[S{episode.season|pad:2}E{episode.number|pad:2} - ]` renders only if every variable inside is non-empty.
- Directory separators are only the literal `/` between template parts; a variable's value can never introduce a separator (sanitized per segment).
- Sanitization profiles: `windows`, `posix`, `portable` (default; the intersection), applied per segment; deterministic truncation keeps the extension and appends the identity short hash when truncation or a collision occurs.
- Collisions are resolved deterministically by identity order (` (2)`, ` (3)`), recorded on the archive file.

The renderer is a pure function `(template, context, profile) -> Result<RelativePath, TemplateError>` with property tests.

## Consequences

- Guaranteed determinism and root containment; no arbitrary logic in templates.
- Users coming from podcast-archiver/podcast-dl get familiar variables and date formatting.
- Adding a variable is a code change with tests; acceptable for an archive tool.

## Alternatives considered

- **Tera / minijinja:** powerful, but loops/conditionals and filters that can inject separators make determinism and safety harder to guarantee; a sandboxed subset would be most of the work anyway.
- **Fixed layout with switches (Podgrab):** fails the customization requirement.
