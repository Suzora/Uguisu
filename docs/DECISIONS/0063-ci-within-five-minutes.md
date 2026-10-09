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

- **The budget is per job: 4.5 minutes from a cold cache**, leaving the rest of the five minutes for queueing and `ci` itself. `win-desktop` and `win-desktop-lint` are over it (below), and the owner accepted that. A change that makes any job slower than measured here brings it back in the same pull request, by splitting the job or by making what it runs cheaper.
- **The tests run under cargo-nextest**, every test in its own process and many at once, then `cargo test --doc`, which nextest does not run. Where nextest is not installed, `check.py` runs the same tests under `cargo test` and says so on the `PASS` line. CI passes `--require-tools`, so it never falls back.
- **argon2, blake2 and sha2 build optimised in the dev profile** (`Cargo.toml`). Release builds are unchanged.
- **CI builds without debug info** (`CARGO_PROFILE_DEV_DEBUG=0`): linking is faster and the caches smaller. A panic still names its file and line.
- **Rust caches are saved only from branch runs**: `main`, the schedule and manual runs. A pull request restores `main`'s and saves none, so it never evicts them; only setup-node's small pnpm cache is saved from any run. A run twice a week reads them, because GitHub evicts a cache nobody read for seven days. When `Cargo.lock` changes, the cache of the previous lockfile is restored and only what changed is built.
- **Every push to a pull request runs every job but `win-test-full`, drafts included.** The Markdown-only shortcut is gone, because a skipped job would fail `ci`.

**Measured** on pull request #1: cold in its first run (37972426744), every job without a cache; warm in a manual run on the branch (37976378733), after an earlier run saved its caches.

| Job | Cold | Warm | Job | Cold | Warm |
|---|---|---|---|---|---|
| `quick` | 1:08 | 0:47 | `win-clippy` | 3:45 | 1:24 |
| `clippy` | 1:51 | 0:37 | `win-cli` | 3:26 | 1:53 |
| `test-engine` | 2:26 | 2:02 | `win-tests` | 3:38 | 1:59 |
| `test-rest` | 3:42 | 2:28 | `win-desktop-lint` | 4:38 | 2:01 |
| `e2e` | 2:09 | 1:07 | `win-desktop` | 5:34 | 3:22 |
| `desktop-lint` | 2:39 | 1:18 | `ci` | 0:04 | 0:05 |
| `desktop` | 4:23 | 2:10 | `win-test-full` (not on a pull request) | — | 6:45 |

A cold run took 5:44 and a warm one about 3:30, not counting time queued behind another run. Runners vary: across four cold runs `win-desktop` took between 4:29 and 6:43.

The two Windows desktop jobs compile the desktop workspace — Tauri, WebView2 and the server — which takes about four minutes on the runner's four cores before anything runs. A Dev Drive for cargo's and rustup's homes was measured and gained nothing net (alternatives). The owner kept them as they are rather than turning off Defender's real-time scan or moving them after the merge: a cold run follows only a toolchain change or an evicted cache.

**Kept from ADR 0046:** `--locked` on clippy, the tests and the desktop crate, the pinned toolchain, a timeout on every job (`check_needs.py --workflow`), packaging only on a `v*` tag or by hand, and `build`, `openapi` and `bench` as local checks only.

## Consequences

- A Windows-only defect outside `test-windows`' binaries is found by `win-test-full` on `main`, minutes after the merge. A red `main` is fixed before the next merge (CLAUDE.md).
- A run is 13 jobs, 14 on `main`. GitHub Free runs 20 jobs at once across the organisation, so two overlapping runs queue.
- A cold run takes about 5:45, and up to about seven minutes on a slow runner, because of `win-desktop`. It follows every toolchain change and every Rust update on the runner image, because the cache key covers the installed toolchains.
- A backtrace in a CI log has no line numbers; a panic message still names its file and line.
- A Markdown-only pull request runs everything too.

## Alternatives considered

- **Larger runners.** Rejected by the owner: they need a paid plan and bill per minute even for a public repository.
- **The whole Windows suite on every pull request.** Rejected: compiling the test suite alone took 5.5 minutes on Windows from a cold cache.
- **A Windows Dev Drive for cargo's and rustup's homes.** Measured and reverted: unpacking the crates took 23 s less and compiling 8 s less, but setting up the drive took 8 s and installing the toolchain into a new rustup home 14 s more; `win-desktop` took 5:46.
- **ring instead of aws-lc-rs as the TLS crypto provider.** Not done. aws-lc's C build is the longest single step of every cold job, but in the desktop jobs the Tauri chain is about as long, so it would save an estimated 10–30 s there, for a change to a production dependency that also drops post-quantum key exchange.
- **A nextest archive built once and run in partitions.** Rejected: the archive's upload and download add a step to every job's critical path.
- **sccache.** Rejected: it does nothing for a cold run.
