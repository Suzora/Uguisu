# web/ — Uguisu web UI

Svelte 5 (runes) + Vite + TypeScript single-page application, with no runtime dependencies. No SvelteKit: the API is `uguisu-server`, and `uguisu serve` serves the built `dist/` from disk (ADR 0031, amending ADR 0004).

```bash
pnpm install
pnpm dev        # http://localhost:5173, proxies /api to a local `uguisu serve`
pnpm check      # svelte-check (type errors)
pnpm test       # vitest + jsdom + @testing-library/svelte
pnpm build      # → dist/
```

**[`docs/WEB_UI.md`](../docs/WEB_UI.md) is the document to read**: the pages, the API boundary and its error taxonomy, the event stream, media serving, the security properties, and the known limitations. The decisions behind them are ADRs [0031](../docs/DECISIONS/0031-serving-the-web-ui.md)–[0034](../docs/DECISIONS/0034-web-ui-architecture.md).

```text
src/lib/api/     the only place that calls fetch — client, types, one function per operation
src/lib/         router, navigation, the event stream, the player, formatting, resource
src/lib/components/   the pieces more than one view needs
src/views/       one file per page
src/tests/       the harness, and the structural accessibility checks
```

`src/lib/api/index.ts` is deliberately flat: one exported function per route, so the client generated from the OpenAPI document in a later phase replaces that file and the views do not move.
