# Working on Uguisu

Uguisu, part of Suzora, is a self-hosted podcast archiver: a Rust workspace (`crates/*`), a Svelte SPA (`web/`), one binary `uguisu`.

This file is **rules** — what to do and what never to do. Facts live in `docs/`; do not copy them here.

## Never

- **`crates/uguisu-storage/migrations/*.sql` are immutable, comments included.** `sqlx::migrate!` stores a checksum per file; changing a shipped migration makes every existing database refuse to open, and no test catches it because tests always start empty. New schema goes in the next numbered file.
- **Nothing deletes a media file.** Leftovers, unknown files and conflicts are reported and kept. No command, no recovery path, no cleanup.
- **Never modify the only copy of a media file.** Copy it, write the copy, re-read and re-hash it, then rename over the original. `lofty`'s in-place API is in `clippy.toml`'s `disallowed-methods` so the safe path cannot be bypassed by accident.
- **Write → fsync → atomic rename**, always, for every file Uguisu writes. Temporary files get a per-writer unique suffix (`layout::tmp_suffix`), never a fixed name.
- **Crate boundaries hold.** Only `uguisu-http` builds HTTP clients, only `uguisu-storage` uses `sqlx`, only `uguisu-metadata` uses `lofty`. `cargo deny` enforces the first one. Do not route around a layer.
- **`.uguisu` under the media root is reserved** and must be skipped by every enumerator.
- **`docs/api/openapi.json` and `web/src/lib/api/generated/schema.d.ts` are generated.** Regenerate with `cargo run -p openapi-doc` and `pnpm --dir web api:types`; never edit either by hand. A wire shape changes in Rust or not at all.
- **A credential never reaches a URL, a log, an argument or an environment variable.** No query parameter carries a token, `tracing` never gets a secret, and a password arrives on stdin or from a terminal.
- **Exit codes are an API.** The `Exit` enum in `crates/uguisu-cli/src/output.rs` and the JSON `schema` fields are contracts; changing one is a breaking change.
- **Tests never use the network.** Use `wiremock`, the fixtures, or the scenario media server in `uguisu-download`'s `testing` feature.

## Where to look

| For | Read |
|---|---|
| Commands, flags, exit codes | `docs/CLI.md` |
| The HTTP contract: auth, errors, pagination, SSE | `docs/API.md`; the machine-readable one is `docs/api/openapi.json`, generated — never hand-edited |
| Binding, a reverse proxy, what is not trusted | `docs/DEPLOYMENT.md` |
| Environment variables and defaults | `docs/CONFIGURATION.md`; the code's source of truth is `SETTINGS` in `crates/uguisu-core/src/settings.rs` |
| Why something is the way it is | `docs/DECISIONS/` (index in its `README.md`) |
| Feed, download, archive, service behaviour | `docs/FEED_ENGINE.md`, `docs/DOWNLOAD_ENGINE.md`, `docs/ARCHIVE_ENGINE.md`, `docs/SERVICE.md`, `docs/DISCOVERY.md` |
| Schema, episode identity, sidecars | `docs/DATA_MODEL.md` |
| Threat model and controls | `docs/SECURITY.md` |
| Lint levels | `Cargo.toml` `[workspace.lints]` and `clippy.toml` — never restated in prose |
| Layering, crate responsibilities | `docs/ARCHITECTURE.md` |
| Toolchain, layout, how to add a crate | `docs/DEVELOPMENT.md` |
| The desktop shell, its packages and how they are verified | `docs/DESKTOP.md`; for users, `INSTALL.md` |

**Before adding an event, a config key, an `ArchiveErrorKind`, a `uguisu-http` profile or a managed tag field**, read `docs/DEVELOPMENT.md` § "Adding things that have a fixed cost". Each of those touches more places than it looks like, and the compiler does not catch all of them.

## Checks

```
python3 scripts/check.py              # fmt, clippy, test, web, docs
python3 scripts/check.py test         # one check; prints bare PASS
python3 scripts/check.py all          # everything, including deny, bench, smoke
python3 scripts/check.py --list       # what exists
```

Success prints one line per check. A failure prints `FAIL <name>`, the command to reproduce it, and the full output. `just` is optional and may not be installed. On failure the full log is also at `target/check/<name>.log`. Use `--stream` when you need to watch a long run.

**Never paste a successful check's output into a reply.** Report the `PASS` lines.

## CI

- Every pull request push runs every job of `ci.yml` but `win-test-full` at once, and `ci` is the check a merge needs ([ADR 0063](docs/DECISIONS/0063-ci-within-five-minutes.md)). Every check passes locally before a push, and a push is finished work, not each commit.
- No pull-request job takes more than 4.5 minutes from a cold cache; `win-desktop` and `win-desktop-lint` are the accepted exceptions. A change that makes a job slower than ADR 0063 measured brings it back in the same pull request.
- Merge only a head that contains the current `main` and has a green `ci`. The push to `main` runs the whole Windows suite; a red `main` is fixed before the next merge.
- A `#[cfg(windows)]` test goes in a binary that `check.py test-windows` runs, or Windows checks it only after the merge.
- Packaging runs on a `v*` tag or a manual `desktop.yml` run, never on a pull request.

## Investigate

- Read the code you are about to change. Read the callers too.
- Never state how something behaves unless you have read it. "Probably" is not an answer.
- Look for an existing type, helper or abstraction before adding one: shared model in `uguisu-core`, paths in `archive::layout`, test servers behind the `testing` features.
- Use the existing architecture. If a change seems to need a new layer, that is a signal to re-read the old one.

## Scope

- Do what was asked and stop. Nothing hypothetical, nothing speculative.
- No opportunistic refactoring of code the task did not touch.
- No configuration knob nobody asked for.
- Found an unrelated problem? Say so; do not fix it in the same change.

## Code

- The simplest direct implementation that holds the invariant.
- No abstraction with one caller. No wrapper that only forwards. No trait for one implementation.
- No defensive check inside an invariant the module already guarantees — state the invariant in one line instead.
- Validation at trust boundaries — feed input, HTTP responses, paths, API bodies, CLI arguments — stays complete and is never "simplified".

## Comments

- None by default. The code says what; a comment is for why.
- Write one only for: a reason that is not visible locally, an invariant, a workaround, an ordering that matters, or a rejected alternative somebody will otherwise retry.
- One line by default.
- Security, concurrency, crash-recovery, `unsafe` and trigger/cascade-ordering reasoning may take as many lines as it needs. **Never delete a comment that carries an invariant.**
- No commented-out code. No banner rules. No `// ---- section ----` headers.
- `TODO`/`FIXME` only with a concrete, actionable task.
- Do not record history in a comment. "Phase 6 added…", "this used to be…" belongs in the commit message. State what is true now.

## Rustdoc

- `missing_docs` is on and CI denies warnings: every public item needs at least one doc line. **Rustdoc is shortened, never deleted.**
- One line, saying what a caller must guarantee, what the return means, or what the function will never do.
- No rustdoc that restates an obvious name. Invariant documentation may be longer.
- A design argument belongs in an ADR, with the doc comment pointing at it — not in both places.

## Tests

- Test behaviour and invariants, never implementation details.
- Name the behaviour asserted. Prefer 3–5 words, at most 6. Never a sentence; no `should`, `correctly`, `properly`, `when_we`.
- Never re-implement production logic in a test.
- No fixture for a single caller. Keep every edge case: unicode, path traversal, malformed feeds, redirects to private addresses, collisions, crash boundaries, import ambiguity.
- Rich diagnostics on failure are welcome — but only messages that print state worth seeing. A message that restates the assertion is noise.
- No `println!`, no `dbg!`. **Never delete or `#[ignore]` a test to reduce output.**

## Output

- A successful check is its `PASS` line and nothing else.
- A failure is the complete diagnostics, never truncated, never summarised away.
- No progress narrative, no repeated status summaries, no re-reading a file to confirm an edit the tool already confirmed.
- Final report: `DONE <area>` lines, `TEST PASS`, `COMMIT <hash>`, then only what genuinely needs saying.

## Logging

- `tracing` with structured fields (`podcast_id`, `episode_id`, `job_id`, `provider`, `url`, `status`, `bytes`, `duration_ms`, `error`), never prose interpolation.
- No `info!` in a loop that succeeds. One log per state change, at the layer that owns it — never the same event at two layers.
- An event that `EventKind` already persists does not also need a log line, unless the log carries an error the event does not.
- Never log a secret, a token or a credential.

## CLI output

- Two audiences. `--json` is for machines and is the path an agent should use. Text output is for a person and stays understandable.
- Success is a fact, not a sentence about itself: `Added podcast 42`, not `Successfully added podcast 42 to the database`.
- Errors keep everything the user needs to act.
- Changing a message means grepping `crates/*/tests` and `docs/CLI.md` for the literal first.

## Documentation

- One canonical home per concept (see the table above). Link to it; never restate it.
- Tables and examples over prose when they carry more with less.
- A new architectural decision is a new ADR, and the ADR index gets its row. Never a paragraph in a code comment instead.
- Implementation and phase history go in the commit message, not into a document.

## Workspace hygiene

- Scratch files, notes and experiments live outside the repository.
- `git status --porcelain` must be empty before reporting a task done.
- Leave no debug script behind.
- Generated files are regenerated from their source, never hand-edited.

## Commits

- Conventional prefixes: `feat(<crate>)`, `fix(<crate>)`, `refactor`, `test`, `perf`, `docs`, `chore`, `ci`.
- One logical change per commit; the workspace builds and the tests pass at every commit.
- Never hide a behaviour change inside a cleanup commit. If bytes a user sees change, the commit message says so.
