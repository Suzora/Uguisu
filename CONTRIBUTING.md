# Contributing to Uguisu

Thank you for considering a contribution. Uguisu is pre-alpha: the engine, the CLI, the web UI and the desktop application are built, and the [roadmap](docs/ROADMAP.md) is in its last two phases before v1. The most valuable contributions right now are messy real-world feed fixtures, provider knowledge and reviews of the design documents.

## Ground rules

- Read [`docs/PRODUCT.md`](docs/PRODUCT.md) first. Changes that conflict with the principles there (archive-first, original quality, deterministic, never silently destroy data) will not be merged.
- Follow [`docs/ROADMAP.md`](docs/ROADMAP.md); do not start feature work in a phase whose dependencies are not built.
- Architectural changes need an ADR in [`docs/DECISIONS/`](docs/DECISIONS/README.md).
- Every feature comes with tests; nothing is complete without them.
- Never add network access to tests; use fixtures and `wiremock`.
- Keep commits logical and the repository buildable at every commit (see [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md)).

## Workflow

1. Open an issue describing the change and which phase it belongs to.
2. Branch from `main`, implement with tests and docs.
3. Run `python3 scripts/check.py`.
4. Open a draft pull request, and mark it ready for review when it is done. Every push runs all of CI in about five minutes, and the `ci` check must be green on a head that contains the current `main` ([ADR 0063](docs/DECISIONS/0063-ci-within-five-minutes.md)).

## Fixtures

Real-world broken feeds are gold. Contribute them under `tests/fixtures/feeds/parser/` with the publisher's name removed if you prefer, a short note on what is broken, and no media files (enclosure URLs may be replaced by placeholders).

## License

By contributing you agree that your contributions are licensed under the project's license, the GNU Affero General Public License, version 3 or (at your option) any later version (`AGPL-3.0-or-later`, see [`LICENSE`](LICENSE)).
