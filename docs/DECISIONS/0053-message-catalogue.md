# ADR 0053 — A message catalogue in the web UI

**Status:** accepted · **Date:** 2026-10-03

## Context

Phase 10 asks for localization readiness: the web UI should be translatable without touching its views, while only English ships. About four hundred pieces of interface text sat in the markup, attributes and scripts of twenty Svelte files, plural endings were `n === 1 ? '' : 's'`, and `format.ts` fixed its locale to `en` in a constant of its own. ADR 0034 keeps the SPA free of runtime dependencies.

## Decision

### One typed catalogue per locale

`web/src/lib/i18n/en/` holds every piece of interface text, one file per area of the UI (`common`, `app`, `library`, `podcast`, …), assembled in `en/index.ts`. An entry is a string, or a function for text that carries values:

```ts
episodes: (n: number) => plural(n, { one: `${n} episode`, other: `${n} episodes` }),
```

`lib/i18n/index.ts` exports the active catalogue as `m`, its `locale`, and two helpers. Views write `{m.library.empty}`. The type of a catalogue is the type of the English one, so another locale that leaves out an entry, or gives a function the wrong parameters, does not compile.

- **Plurals** go through `Intl.PluralRules` of the catalogue's locale, so a language with more than two forms names them (`few`, `many`) instead of guessing from `n === 1`.
- **Dates, times and relative times** are formatted by `Intl` with the catalogue's locale. The browser's own locale is still not used: a German date inside English text is the mismatch `format.ts` was written to avoid.
- **Closed vocabularies** — the state names, kinds and reasons the API sends (`not_modified`, `invalid_podcast_feed`) — are shown through `label()`: the catalogue's `values` entry when it has one, otherwise the value with its underscores as spaces. English needs no entries; a translation fills them from the enums of `docs/api/openapi.json`.
- `index.html` keeps `lang="en"`, and the shell sets `<html lang>` from the catalogue.

### What stays as it is

- **What the server says**: error messages, verification and check reasons, a refresh's warnings. They come from the engine in English, and translating them would mean a second vocabulary on the server; a view shows them as sent, beside text of its own.
- **What a feed says**: titles, show notes, author names.
- **The CLI, the desktop shell's own dialogs and the documentation.** Readiness here is the SPA's.

### A guard

A test parses every Svelte file and fails on text in the markup, or in a `title`, `aria-label`, `placeholder` or `alt` attribute, that is not an expression. Text in a script is moved by hand, and a second test fails on a string literal in a view's or a component's script that reads like a sentence.

### Adding a language

Copy `en/` to `<locale>/`, translate it, give it the locale's code, fill `values`, and choose it at start-up. Choosing is not built: with one catalogue there is nothing to choose between. A second locale brings the choice with it — the browser's languages, then a setting that overrides them.

## Consequences

- An interface text changes in the catalogue, not in the view. The tests still assert the English text a person reads, which is what they should assert.
- A translator works in TypeScript files, not in a translation tool's format. With a few hundred entries a typed file is reviewable, and the compiler checks completeness; a format for a translation service can be generated from it when one is wanted.
- The catalogue is part of the bundle: one locale's text, a few kilobytes compressed.

## Alternatives considered

- **A library** (`svelte-i18n`, Paraglide, `typesafe-i18n`). Rejected for one locale: each is a runtime dependency or a build plugin, and ICU message syntax inside strings is checked at run time, where a typed function is checked by the compiler.
- **JSON catalogues with message keys.** Rejected: a missing key is found by a person reading the screen, and interpolation and plurals need a syntax of their own.
- **Choosing the locale from the browser now.** Rejected while only English exists.
