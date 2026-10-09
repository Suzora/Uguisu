# ADR 0056 — Database maintenance: migrate, back up, check, vacuum

**Status:** accepted, amends [ADR 0002](0002-sqlite-storage.md) · **Date:** 2026-10-03

## Context

`uguisu db migrate` and `uguisu db backup <path>` have answered "not implemented" since Phase 3. ADR 0002 promises a backup through SQLite's online backup API, `SECURITY.md` and `RISKS.md` R15 counted that backup among the controls before it existed, and `ARCHITECTURE.md` names a database integrity check and a vacuum that nothing runs. `DEPLOYMENT.md` tells an operator to back up the database with the server stopped, because nothing else was safe.

Every command that opens the database already applies pending migrations, and a running `uguisu serve` holds the data directory's lock, so an embedded command cannot reach the database while the server runs.

## Decision

Four commands, each embedded and, through the API, against a running server:

| Command | Route | What it does |
|---|---|---|
| `db migrate` | — | opens the database, applies what is pending and says which schema it found and which it left |
| `db backup [<path>]` | `POST /api/v1/db/backup` | writes a consistent copy of the database |
| `db check` | `POST /api/v1/db/check` | `PRAGMA integrity_check` and `PRAGMA foreign_key_check`; exit 1 on any finding |
| `db vacuum` | `POST /api/v1/db/vacuum` | `VACUUM`, and the file's size before and after |

### Backup

- The copy is `VACUUM INTO` a temporary file next to the destination, named with the writer's unique suffix, then fsynced and renamed into place, and the directory fsynced. `VACUUM INTO` reads one snapshot, so the copy is consistent while the server keeps writing, and it is compact.
- An existing destination is refused, never overwritten; it is looked at before the copy starts.
- Without a path, the copy goes to `<data dir>/backups/uguisu-<UTC time>.db`.
- Through the API the destination is always the server's own `backups/` directory, with a name the server chooses: a request names no path on the server's machine, so a credential that can call the route cannot make the server write anywhere else. `db backup <path> --server` is a usage error.
- Nothing removes old backups, and a copy that failed leaves its `.tmp` file: what to keep is the operator's decision.

### Migrate

The schema version is read before the migrator runs and after it, and the command reports both with the versions it applied. Through the API there is nothing to do — the server migrated when it opened — so `db migrate --server` is a usage error that says so.

### Check and vacuum

`db check` is read-only and runs the full `integrity_check`, which reads every page; it is a command, not something a start-up or the maintenance pass runs. `db vacuum` rewrites the whole file through the writer connection, so writes wait for it, and it needs free space about the size of the database.

## Consequences

- Backing up a running server is one request, and `DEPLOYMENT.md`'s "with the server stopped" becomes one of two ways.
- The `backups/` directory grows until the operator prunes it.
- A backup holds what the database holds: token digests, the password hash, every feed URL with any token a private feed carries in it. It is written with the data directory's permissions, and `DEPLOYMENT.md` says to treat it like the database.
- The CLI has no scaffolded command left once `episode list|show` exist, so "command not implemented yet" leaves the exit-2 row of `CLI.md`.

## Alternatives considered

- **SQLite's backup API** (`sqlite3_backup_*`), as ADR 0002 wrote. Rejected: `sqlx` does not expose it, and `VACUUM INTO` gives the same consistent snapshot through a statement.
- **A destination path in the request.** Rejected: it would let an API credential create files anywhere the server's user can write.
- **Copying the file.** Rejected: copying a database that is being written to is how a WAL gets torn.
- **Running `integrity_check` in the maintenance pass.** Rejected for now: it reads the whole file, and a check whose result nobody asked for has nowhere to go but a log line.
