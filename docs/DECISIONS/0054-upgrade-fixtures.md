# ADR 0054 — Upgrade fixtures: what a tagged build left, opened by every later one

**Status:** accepted · **Date:** 2026-10-03

## Context

Phase 10 asks for upgrade migrations tested from every tagged pre-release, and its acceptance says upgrading from the previous pre-release preserves all data. There is no tag yet. `crates/uguisu-storage/tests/migration_upgrade.rs` starts a database at each earlier schema version and opens it with the current one, but it seeds those databases with SQL written for the test: it proves the migrations, not that a database a real build wrote — its event payloads, settings, hashes, sidecars, manifests, credential — still reads the same afterwards. Nothing checked the CLI's `--json` output across versions either, although that output is a contract (`docs/API.md`).

## Decision

### A fixture per tagged pre-release

`scripts/upgrade_fixture.py --bin <build> --name <tag>` makes a data directory the way a person makes theirs, through that build's CLI and a publisher on this machine: a podcast added, both episodes downloaded and archived with their sidecars and manifest, artwork fetched, a policy and a setting stored, a later refresh that leaves a candidate duplicate and an unverified feed announcement, a password set and a read token issued. It then records what the same build's `--json` read commands say: the podcast list and page, the archive, the policies, the manifests, the artwork, the queue, the candidates, the setting, the tokens and each sidecar.

The result goes to `tests/fixtures/upgrade/<tag>/`: `data/` as the build left it, and `fixture.json` with the recorded output, the build's version and commit, and the password and token secret — test data that protects nothing. `.gitattributes` keeps `data/` byte for byte, since its hashes and manifests describe those bytes. A fixture is never rewritten.

### Every later build opens every fixture

`crates/uguisu-cli/tests/upgrade.rs` copies each fixture's `data/`, runs this build's CLI against it and requires:

- every recorded command to succeed and to say at least what it said: a field may be added, nothing recorded may be missing or different;
- `archive verify --all --full` and every podcast's `archive manifest verify --full` to pass, so every archived byte is what its record and its manifest say;
- the password to sign in and the token to open the API, through `uguisu serve`.

A difference is a regression, or a change of meaning that the API's versioning rules do not allow within `v1`.

### The first fixture comes before the first tag

`untagged-3bb9e20` is the build of commit `3bb9e20` (pre-publication history, not in this repository), at schema 6, with every table in use. It is not a pre-release, but it is what a database looks like today, and it makes the next migration prove itself against real data from the day it is written. Tagging a pre-release adds its own fixture (`docs/DEVELOPMENT.md`).

The SQL-seeded storage tests stay: they start from schemas 1 to 5, which no tagged build will ever have written.

## Consequences

- Each fixture is about half a megabyte, nearly all of it the SQLite file. A handful of pre-releases is a few megabytes in the repository; a fixture can be dropped once no supported upgrade starts from it.
- The fixture is a binary database, against the fixture rule that media is built in code: an old build's output cannot be rebuilt by new code, which is the point of keeping it. `fixture.json` records how it was made.
- Covered is what the recorded commands show and what verification checks. A field no read command prints, or a file no check reads, is not.
- An upgrade from a version that was never tagged is not tested.

## Alternatives considered

- **Downloading the previous release's binary in CI** and running it there. Rejected: tests never use the network, and it costs a release build per run.
- **An SQL dump** instead of the database file. Rejected: the full-text index's shadow tables do not round-trip cleanly through a dump, and the file is what an installation actually has.
- **Comparing outputs exactly.** Rejected: a newer build may add fields, and that is allowed.
