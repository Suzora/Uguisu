# ADR 0063 — Every pull request's CI within five minutes

**Status:** accepted, supersedes in part [ADR 0046](0046-ci-within-a-minutes-budget.md) · **Date:** 2026-10-09

## Context

ADR 0046 shaped CI around a minutes quota: a private repository on GitHub Free, 2000 minutes a month, a Windows minute billed as two. The repository is public now. GitHub-hosted standard runners cost nothing there, and rulesets can require a check before a merge.

What 0046's shape still cost was time. The first run in the public repository (run 37950883844, no cache) took 48 minutes:

| Job | Took | Where the time went |
|---|---|---|
| `quick` | 0:23 | no compiler |
| `linux`, after `quick` | 20:48 | `test` 651 s: 190 s compiling, then 83 test binaries one after another; `desktop` 262 s; `clippy` 102 s |
| `windows`, after `linux` | 27:12 | `test` 777 s, of which 329 s compiling; `desktop` 384 s; `clippy` 199 s |

- The jobs waited for each other only to save minutes.
- argon2 ran at production cost in an unoptimised build, about 2 s per hash: `uguisu-server`'s auth tests took 63 s.
- Nothing ran on `main`, so every pull request started with a cold cache.
- The C build of aws-lc-sys is the longest single step of every cold job: 55–67 s on Linux, 87–118 s on Windows.

The owner set the target: every pull request run within five minutes, from a cold cache too, on free runners. Windows runs a reduced set on a pull request and the whole suite after the merge.

## Decision

**Every job starts at once, and `ci` is the one check a merge needs.** `ci` needs every other job except `win-test-full`, runs `if: always()`, and passes only when all of them succeeded (`check_needs.py --ci`). `check_needs.py --workflow` fails when that shape drifts.

| Job | Runner | Checks (`scripts/check.py`) |
|---|---|---|
| `quick` | Linux | `fmt docs migrations version docker desktop-meta deny web api-types`: no compiler |
| `clippy` | Linux | `clippy` |
| `test-engine` | Linux | `test-engine`: `uguisu-engine`'s tests |
| `test-rest` | Linux | `test-rest`: every other package's tests |
| `e2e` | Linux | `smoke e2e` |
| `desktop-lint` | Linux | `web-build desktop-lint` |
| `desktop` | Linux | `web-build desktop-test`, then the unpackaged desktop smoke under Xvfb |
| `win-clippy` | Windows | `clippy`, the only lint of the `cfg(windows)` code |
| `win-cli` | Windows | `smoke` |
| `win-tests` | Windows | `test-windows`: the binaries with `#[cfg(windows)]` tests |
| `win-desktop-lint` | Windows | `web-build desktop-lint` |
| `win-desktop` | Windows | `web-build desktop-test`, then the unpackaged desktop smoke |
| `win-test-full` | Windows | `test`, the whole suite: on a push to `main`, a tag, the schedule or by hand, never on a pull request |

- **The budget is per job: 4.5 minutes from a cold cache.** The rest of the five minutes is runner setup and `ci` itself. A change that pushes a job over brings it back under in the same pull request, by splitting the job or by making what it runs cheaper.
- **The tests run under cargo-nextest**, every test in its own process and many at once, then `cargo test --doc`, which nextest does not run. Where nextest is not installed, `check.py` runs the same tests under `cargo test` and says so on the `PASS` line. CI passes `--require-tools`, so it never falls back.
- **argon2, blake2 and sha2 build optimised in the dev profile** (`Cargo.toml`). Release builds are unchanged.
- **CI builds without debug info** (`CARGO_PROFILE_DEV_DEBUG=0`): linking is faster and the caches smaller. A panic still names its file and line.
- **Caches are saved only from branch runs**: `main`, the schedule and manual runs. A pull request restores `main`'s caches and saves nothing, so it never evicts them. A run twice a week reads them, because GitHub evicts a cache nobody read for seven days. When `Cargo.lock` changes, the cache of the previous lockfile is restored and only what changed is built.
- **Every push to a pull request runs everything, drafts included.** The Markdown-only shortcut is gone, because a skipped job would fail `ci`.

**Kept from ADR 0046:** `--locked` on every cargo command that resolves dependencies, the pinned toolchain, a timeout on every job (`check_needs.py --workflow`), packaging only on a `v*` tag or by hand, and `build`, `openapi` and `bench` as local checks only.

## Consequences

- A Windows-only defect outside `test-windows`' binaries is found by `win-test-full` on `main`, minutes after the merge. A red `main` is fixed before the next merge (CLAUDE.md).
- A run is 13 jobs, 14 on `main`. GitHub Free runs 20 jobs at once across the organisation, so two overlapping runs queue.
- A cold run follows every toolchain change and every Rust update on the runner image: the cache key covers the installed toolchains.
- A backtrace in a CI log names functions but no lines.
- A Markdown-only pull request runs everything too.

## Alternatives considered

- **Larger runners.** Rejected by the owner: they need a paid plan and bill per minute even for a public repository.
- **The whole Windows suite on every pull request.** Rejected: compiling the test suite alone took 5.5 minutes on Windows from a cold cache.
- **ring instead of aws-lc-rs as the TLS crypto provider.** Deferred. It would shorten the longest step of every cold job, but changes a production dependency and drops post-quantum key exchange. It gets its own ADR if a job stays over the budget.
- **A nextest archive built once and run in partitions.** Rejected: the archive's upload and download add a step to every job's critical path.
- **sccache.** Rejected: it does nothing for a cold run.
