# ADR 0046 — CI within a minutes budget

**Status:** accepted, amends 0043, amended by [ADR 0061](0061-the-docker-image.md), [ADR 0064](0064-linux-floor-in-a-container-and-every-update-tested.md), superseded in part by [ADR 0063](0063-ci-within-five-minutes.md) · **Date:** 2026-10-01

## Context

The repository is private and its organisation is on GitHub Free: 2000 Actions minutes a month, a Windows minute billed as two, every job rounded up to a whole minute. In September the minutes ran out before Phase 9a's gate could run. The quota reset on 2026-10-01, and that day alone billed 1566 minutes, measured from the job timestamps of the day's runs:

| What | Billed minutes |
|---|---|
| One pull request push: `ci.yml` + `desktop.yml` | ≈ 259 (89 + 170) |
| The merge to main, on the same tree again | 174–300 |
| Windows share | 62 % (`Rust (windows-latest)` 44–76, `bundle-windows` 80, `desktop-windows` 18–46) |
| Cancelled or superseded runs | 287 |

The packaging tier (ADR 0043) ran on every pull request touching `crates/`, `web/` or `Cargo.*`, which is nearly all of them. `ci.yml` had no job timeouts, and three hung jobs once billed 441 minutes before someone cancelled them.

## Decision

*The repository is public now, and [ADR 0063](0063-ci-within-five-minutes.md) replaces the job layout, the draft rule, the Markdown-only shortcut and "no run on a push to main". The pinned toolchain, `--locked`, the timeouts and packaging only on a tag or by hand still hold.*

**Three jobs in `ci.yml`, each started only after the cheaper one passed.**

| Job | Runs | Checks (`scripts/check.py`) |
|---|---|---|
| `quick` | every pull request event, tag and manual run | `fmt docs migrations version docker desktop-meta deny`: no compiler *(`docker` added by ADR 0061)* |
| `linux` | unless only `.md` files changed | `clippy test smoke api-types web e2e desktop`, then the unpackaged desktop smoke under Xvfb |
| `windows` | as `linux`, but never on a draft pull request, and only after `linux` passed | `clippy test smoke desktop`, then the unpackaged desktop smoke |

- The root and desktop workspaces share `target/`, so one Linux job builds the dependencies once.
- `clippy` stays on Windows: it is the only lint of the `cfg(windows)` code.
- `build` and `openapi` are local checks only. `test` compiles everything `build` did except the benchmarks, which `clippy --all-targets` type-checks. The OpenAPI drift test runs inside `cargo test --workspace`. `bench` stays local, as before.
- `clippy`, `test` and the desktop crate run with `--locked`, so a stale lockfile fails instead of being rewritten.

**No run on a push to main.** The pull request already ran on the tree that is merged. CI evidence (ROADMAP) is therefore a green `ci.yml` run on a pull request head whose tree is the merged tree. A pull request is merged only when its head contains the current main and `ci.yml` passed on exactly that head. Branch protection could enforce this, but GitHub Free offers none for a private repository, so it is a rule for whoever merges.

**Packaging only on a `v*` tag or by hand.** `desktop.yml` builds, installs and smokes the six formats and runs the `.deb` upgrade only on a tag or `workflow_dispatch`, so bundling is always required there. `desktop-packaging` needs the six `package-*` jobs and `upgrade-deb`, and passes only if all seven succeeded *(and the four other upgrade jobs: ADR 0064)*. tauri-cli comes prebuilt through cargo-binstall.

**Every job has `timeout-minutes`.** `check_needs.py --workflow` fails on a job without one in either workflow, and on a pull-request or branch trigger in `desktop.yml`.

**The Rust toolchain is pinned.** `rust-toolchain.toml` names an exact release, and every job installs that one rather than the current stable. Rust 1.99.0 came out on 2026-10-01 with a new default lint, `clippy::assert_is_empty`, that flags 69 existing assertions. A floating `stable` turned this PR's first run red on its release day, and a new release would also change every cache key. Moving to a newer release is its own pull request, which fixes whatever the new lints find.

**Pull requests start as drafts.** A draft runs `quick` and `linux`. Marking it ready runs `windows` once, and each push after that runs it again.

Expected cost, from the same job timestamps:

| Event | Billed minutes |
|---|---|
| Markdown-only push | ≈ 1 |
| Draft push | ≈ 20–35 (cold first run) |
| Ready push | the above plus ≈ 60–125 for Windows |
| Merge | 0 |
| Tag or manual packaging run | ≈ 120–170 |

## Consequences

- A change that breaks only an installed package is found when packaging runs before a release, not on the pull request. Run `desktop.yml` by hand on a pull request that touches `desktop/`, the packaging scripts or the lockfiles, when in doubt.
- A Windows-only failure is found when the pull request is marked ready, not on each draft push.
- Nothing refreshes the main-branch cache any more. A pull request's first run, and every Windows run, starts cold.
- A main that drifts after a merge is noticed by the next pull request, not by a run of its own.
- Phase 9a's gate stays the packaging evidence (ADR 0043); a green gate now includes the upgrade.

## Alternatives considered

- **Nightly Windows or packaging runs.** Rejected: 30 nightly Windows runs cost 1860–3660 minutes, more than the month's quota.
- **A path filter for the Windows job.** Rejected: the Windows defects found in Phase 9a were in `uguisu-cli`'s tests and the storage layer, which no reasonable filter excludes.
- **Running the Windows tier on a label instead of on ready.** Rejected by the owner: a pull request that is ready gets its Windows run without anyone remembering to ask.
- **Keeping a cheap Linux run on main.** Rejected by the owner: the pull request has already run on that tree.
