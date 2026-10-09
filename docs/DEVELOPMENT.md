# Development Guide

## Toolchain

- Rust, pinned to an exact release in `rust-toolchain.toml`, edition 2024, with the `rustfmt` and `clippy` components. A newer release is adopted in its own change, together with fixes for any new lints ([ADR 0046](DECISIONS/0046-ci-within-a-minutes-budget.md)).
- Node 22 + pnpm 10 for `web/`.
- Optional: [`just`](https://github.com/casey/just) (task runner; every recipe is also a plain `check.py`, cargo or pnpm command), `cargo-deny`, [`cargo-nextest`](https://nexte.st) (runs the tests many at once; without it `check.py test` runs the same tests under `cargo test`, one binary after another, and says so on its `PASS` line).
- Docker is only needed for the image (`docker build -t uguisu .`) and `scripts/docker_smoke.py`.

## Commands

```bash
python3 scripts/check.py              # fmt, clippy, test, web, docs, openapi, api-types, version, docker — one line per check
python3 scripts/check.py all          # everything, incl. deny, audit, bench, smoke, migrations, e2e, desktop
python3 scripts/check.py audit        # the web dependencies against the npm advisory database; needs the network
python3 scripts/check.py docker-smoke # builds the Docker image and runs its scenarios; needs Docker
python3 scripts/check.py desktop      # the desktop workspace; needs WebKitGTK (Linux) or WebView2 (Windows)
python3 scripts/check.py desktop-meta # the desktop harnesses and the workflows' shape; Python only
python3 scripts/check.py --list       # every check and the command it runs
python3 scripts/check.py --stream …   # inherit stdio instead of capturing
```

Every check command lives in `scripts/check.py`, and the `justfile` and CI both call it, so there is exactly one definition of each. A failure prints the command, the log path (`target/check/<name>.log`) and the full output. `just` is optional. CI runs neither `build` nor the benchmarks: `clippy --all-targets` type-checks them, and `check.py build`, `check.py bench` and `cargo bench` are run locally. Which CI job runs which check, and when, is [ADR 0063](DECISIONS/0063-ci-within-five-minutes.md); `test-engine`, `test-rest`, `test-windows`, `desktop-lint` and `desktop-test` are its slices of `test` and `desktop`, and `all` leaves them out. Every step runs with `TMPDIR`, `TEMP` and `TMP` pointing at `target/check/tmp-<pid>`, which a passing run removes and a failing one keeps; a bare `cargo test` leaves its test databases in the system's temporary directory.

Not owned by the runner, because they write rather than check:

```bash
cargo fmt --all                       # rewrite formatting
cargo bench --workspace               # the real benchmark run (`check.py bench` only compiles them)
cargo run -p uguisu-cli -- --help     # or `just run -- …`
pnpm --dir web dev                    # the UI on :5173, proxying /api to a running `uguisu serve`
UGUISU_TEST_LARGE=1 cargo test -p uguisu-download --test large   # adds the 1 GiB case
python3 scripts/desktop_bundle.py deb rpm appimage   # packages, from a fresh link (needs tauri-cli 2.11.5)
python3 scripts/desktop_smoke.py --help              # smoke an installed desktop package (docs/DESKTOP.md)

# One podcast's manifest, checked without Uguisu:
cd "$UGUISU_MEDIA_DIR" && sha256sum -c .uguisu/manifests/<podcast-id>/manifest.sha256
```

## Repository layout

```text
crates/          Rust workspace members (see docs/ARCHITECTURE.md §2)
web/             Svelte 5 + Vite + TypeScript SPA (see docs/WEB_UI.md)
desktop/         Tauri 2 shell (see docs/DESKTOP.md)
docs/            product, architecture, research, ADRs
tests/fixtures/  feed, website, provider and upgrade fixtures (see its README.md)
crates/*/benches/ criterion benchmarks (10 targets)
.github/         CI workflows
```

## Offline discovery demo

Tests never touch the network; the same fixtures can drive the binary by hand:

```bash
# start a wiremock-backed provider in a test, or point the CLI at any HTTP mock serving
# tests/fixtures/discovery/spec/apple/search_darknet.json at /search:
UGUISU_DISCOVERY_APPLE_BASE_URL=http://127.0.0.1:<port> UGUISU_HTTP_ALLOW_PRIVATE_HOSTS=127.0.0.1 \
  cargo run -p uguisu-cli -- search podcast "darknet diaries" --explain
```

Live fixtures are recorded with `cargo run -p record-fixtures -- provider apple --case search_darknet --query "Darknet Diaries"` (or `just record …`) into `tests/fixtures/discovery/live/`; the spec-derived fixtures under `tests/fixtures/discovery/spec/` are never overwritten, so a re-recording shows schema drift as a diff (`record-fixtures diff spec/... live/...`). Podcast Index recordings need `UGUISU_PODCASTINDEX_KEY`/`SECRET` in the environment; credentials are stripped before writing. Replay tests live in `crates/uguisu-discovery/tests/live_fixtures.rs`.

## Conventions

- **Lints:** `Cargo.toml` `[workspace.lints]` and `clippy.toml` are the source of truth; CI denies warnings, so every level there is effectively an error.
- **Errors:** `thiserror` enums per crate; `UguisuError` in core for cross-crate cases.
- **Async:** tokio; bounded concurrency (`Semaphore`); no unbounded channels; cancellation via `CancellationToken`.
- **Logging:** `tracing` with structured fields (`podcast_id`, `episode_id`, `job_id`, `provider`, `url`, `status`, `bytes`, `duration_ms`, `error`); never log secrets.
- **Determinism:** generators (paths, tags, identity keys, sidecars, manifests) are pure functions with golden tests. Two consequences worth stating: rendering the same manifest entries in any order must produce byte-identical output (a property test asserts it), and writing the same tags twice must leave the file untouched the second time — a managed field that does not survive a write-then-read moves the archive's hash on every run, so `crates/uguisu-metadata/tests/tags.rs` guards the whole field set against exactly that.
- **Filesystem safety:** write → fsync → atomic rename; never delete user data without an explicit, logged action. Since Phase 6: **nothing deletes**, full stop — leftovers, unknown files and conflicts are reported and kept. A temporary file gets a per-writer unique suffix (`layout::tmp_suffix`), never a fixed name; a fixed one let two concurrent writers rename each other's file away, which a test found and now prevents. `.uguisu` under the media root is reserved and must be skipped by every enumerator. Never modify the only copy of a media file: copy, write, re-read, re-hash, rename.
- **Tests live next to code** for units, under `crates/<crate>/tests/` for integration, and `scripts/smoke.py` and `scripts/e2e.py` drive the built binary end to end; `tests/fixtures/` holds the fixtures crates share. Network is never used in tests; use `wiremock` and fixtures. Media transfers use the scenario server in `uguisu-download`'s `testing` feature (18 routes: ranges, weak and rotating ETags, redirects including to private addresses, 416, truncated and stalled bodies, wrong lengths, rate limits); `uguisu-engine`'s own `testing` feature enables it and also arms the crash injector (`Engine::open_with_injector`). Tests that download more than 100 MiB live behind `UGUISU_TEST_LARGE=1`. Media fixtures for tag tests are **built in code** rather than checked in (a hand-written MPEG frame and a FLAC `STREAMINFO` block are auditable; a binary blob is not); the foreign-archive fixtures under `crates/uguisu-archive/tests/fixtures/archives/podgrab/` are empty-ish `.mp3` files whose *names* are the fixture.
- **Docs:** update the relevant doc in the same PR as the change; architectural decisions get an ADR (`docs/DECISIONS/README.md`).

## Commits and branches

- Conventional prefixes: `docs:`, `feat(<crate>):`, `fix(<crate>):`, `chore:`, `ci:`, `test:`, `refactor:`, `perf:`.
- One logical change per commit; the repository must build at every commit.
- Feature branches → draft PR → ready for review → `ci` green on a head that contains the current `main` → squash or rebase per the PR's size (keep logical commits when they tell a story). Every push runs all of `ci.yml`; Windows runs a reduced set on a pull request and the whole suite on `main` after the merge ([ADR 0063](DECISIONS/0063-ci-within-five-minutes.md)).

## Tagging a pre-release

A tag `v*` starts packaging ([ADR 0046](DECISIONS/0046-ci-within-a-minutes-budget.md)). Before it is pushed, record what that build leaves behind, so every later build is tested against it ([ADR 0054](DECISIONS/0054-upgrade-fixtures.md)):

```bash
cargo build -p uguisu-cli --bin uguisu
python3 scripts/upgrade_fixture.py --bin target/debug/uguisu --name v0.2.0-rc.1
```

Commit `tests/fixtures/upgrade/v0.2.0-rc.1/` as it was written, and tag that commit: the fixture changes no code, so the tagged build is the one that wrote it. An existing fixture is never regenerated.

## Adding a crate

1. `cargo new --lib crates/uguisu-<name>`; add to `[workspace.members]`; use `workspace = true` for `edition`, `version`, `license`, `lints`, `rust-version`.
2. Write the crate-level doc comment: responsibility, allowed dependencies, phase in which it becomes real.
3. Add it to the dependency table in `docs/ARCHITECTURE.md`.

## Adding things that have a fixed cost

Some additions touch more places than they look like they do, and the compiler does not catch all of them:

- **An event:** the `EventKind` variant, an arm in `name()`, a sample in `every_kind_round_trips` (which names a kind it has no sample for), the name in `KINDS` in `web/src/lib/events.svelte.ts` (SSE dispatches by name, and nothing ties that list to the server), and regenerated contract files, because `EventKind` is in the OpenAPI document.
- **A config key:** one `SettingSpec` row in `core::settings::SETTINGS`, one arm in `Config::from_layers`, and a row in `docs/CONFIGURATION.md`. Every key list in the crate is derived from that table.
- **An `ArchiveErrorKind`:** the variant, `ALL`, `as_str`/`parse`, and the HTTP mapping in `uguisu-server`'s `status_for` — the last one is an exhaustive match, so it will not compile without it, which is deliberate.
- **A `uguisu-http` profile:** two exhaustive matches (`max_body_bytes`, `accept`).
- **A managed tag field:** `Field::ALL`, the `ItemKey` mapping, the capability table per tag type, and the round-trip guard test. A field that does not round-trip does not belong in the set.
- **A route:** `#[utoipa::path]` beside the handler, the handler's name in `ApiDoc`'s `paths(…)`, and a regenerated `docs/api/openapi.json` and `web/src/lib/api/generated/schema.d.ts` (`cargo run -p openapi-doc`, `pnpm --dir web api:types`). Authorization needs no decision: anything not on `access_of`'s public list is authenticated. Three tests fail if any of that is missed.
- **A wire type:** `#[derive(utoipa::ToSchema)]` beside `Serialize`, and `#[schema(as = …)]` if a type with that short name already exists in another crate — `no_two_types_share_a_schema_name` says so before the document silently describes the wrong shape.
- **A scratch file:** a name from `layout::tmp_suffix` or `layout::tmp_path`, and a `layout::Scratch` held from before the file is created until it is renamed or abandoned. Without the hold, `archive orphans` reports a write in progress as a leftover.
- **A secret setting:** `SettingSpec::secret` on the row. That one flag redacts the value in `GET /api/v1/settings` and `config list`, in `value`, in `stored`, and in the rejected and unused lists. A secret key that is also persistable would need encryption at rest, which does not exist (`docs/SECURITY.md` §3.7).

## Phase discipline

Follow `docs/ROADMAP.md`: a phase is finished when its tests and acceptance criteria are met and documented. Do not start feature work in a later phase's crate before its dependencies exist; stubs must stay stubs until then.
