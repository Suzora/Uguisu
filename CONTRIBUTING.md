# Contributing

Thank you for wanting to help. This page says how the project works, so that a contribution
does not get stuck on something avoidable.

## Before you start

- **Read the goal.** [PLAN.md](PLAN.md) says what Uguisu is for and, just as important, what it
  will not do: no player, no apps, no multiple users. A contribution outside that will be
  declined however good it is.
- **Open an issue first** for anything larger than a fix, so that the direction is agreed
  before you write the code.
- **Security problems** are not reported in public issues; see [SECURITY.md](SECURITY.md).

## Working on the code

You need Linux or macOS (on Windows: WSL) and Rust 1.88 or newer.

```bash
cargo run -p uguisu
```

- `crates/uguisu-core` is the archive engine: feeds, the archive on disk, downloads, checks,
  published feeds, search.
- `crates/uguisu` is the server: web interface, login, the command line.

Before you open a pull request, these must pass; CI runs the same:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
```

```bash
cargo test --workspace --locked
```

```bash
cargo deny check
```

```bash
typos
```

## What a pull request needs

- **One topic.** Two unrelated changes are two pull requests.
- **A test** for every feature and every fixed bug. Most tests are in
  `crates/uguisu-core/tests/archive_flow.rs` (the engine against a local feed server) and at the
  end of `crates/uguisu/src/web.rs` (the web interface).
- **A migration** for every change of the database: a new file in
  `crates/uguisu-core/migrations`. Existing migrations are never edited.
- **An honest description.** Say what you tested and what you could not test.
- **The plan and the changelog.** Tick or add the item in `PLAN.md`, and add a line to
  `CHANGELOG.md` under "Unreleased" if someone running Uguisu would notice the change.

Things that are easy to miss:

- **Texts of the interface** go through `t("…")` and need a German translation in
  `crates/uguisu/src/i18n.rs`. A test fails if one is missing.
- **The archive format** (`docs/ARCHIVE.md`) is a promise to people's archives. A change needs
  a new format number, an upgrade step for existing archives and a line in the format's
  history.
- **Content from feeds is untrusted.** It is shown as text, fetched only from public addresses
  and never used to build a file name.
- **New dependencies** need a licence allowed in `deny.toml` and a reason.

## Licence

Uguisu is licensed under the GNU Affero General Public License, version 3 or later. By
contributing you agree that your contribution is published under the same licence.

## Versions

Uguisu follows [semantic versioning](https://semver.org). What counts as its public interface:

- the archive on disk (`docs/ARCHIVE.md`),
- the addresses of published feeds and media files,
- the command line and the environment variables,
- the documents sent to a webhook.

Until 1.0, a minor version (0.**4**.0) may change these, and the changelog says how to
upgrade. From 1.0 on:

- a **patch** version (1.0.**1**) fixes bugs and changes none of the above;
- a **minor** version (1.**1**.0) adds things; whatever worked keeps working, and archives
  written by any 1.x version stay readable;
- a **major** version (**2**.0.0) may break one of them, and then reads the archives of the
  version before it and upgrades them.

The database is not part of the interface: it is an index that Uguisu migrates itself and can
rebuild from the archive.

## Releasing

For maintainers: set the version in `Cargo.toml`, move the "Unreleased" section of
`CHANGELOG.md` under the new version, and push a tag with the same version (`v0.4.0`). The
release workflow builds and publishes the image and creates the GitHub release.

After a release, add a small data directory written by that version to
`crates/uguisu-core/tests/fixtures/<version>`: a podcast with two episodes and a transcript,
with the logins removed. The test `archives_of_released_versions_are_upgraded` then makes sure
every later version still opens it.
