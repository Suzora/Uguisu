# ADR 0061 — The Docker image

**Status:** accepted, amends [ADR 0013](0013-auth-and-network-binding.md), [ADR 0031](0031-serving-the-web-ui.md), [ADR 0037](0037-exposure-gate.md) and [ADR 0046](0046-ci-within-a-minutes-budget.md) · **Date:** 2026-10-04

## Context

The brief names Docker as a way to run Uguisu, and the release checklist asks for an image under 60 MB compressed that starts with its health check green. Until now there was none: ADR 0013 said what a container binds, ADR 0031 that a packaged build would carry the web UI somewhere fixed, and ADR 0037 that a container binding `0.0.0.0` must set a password or pass the override. Each left the image itself for later.

## Decision

**One `Dockerfile` at the repository root, three stages.** The web UI is built on `node:22-trixie-slim` with the pnpm `web/package.json` pins, the binary on `rust:1.98.1-slim-trixie` with `cargo build --release --locked -p uguisu-cli`, and both are copied onto `gcr.io/distroless/cc-debian13:nonroot`. Every base is pinned by digest, so a rebuild cannot pick up a different base; moving a digest is a reviewed change.

**Distroless, glibc, uid 65532.** The binary needs glibc and nothing else at run time. The base has no shell and no package manager, which leaves nothing to exploit in a container that only runs `uguisu`. It runs as the base's `nonroot` user, 65532; there is no `PUID`/`PGID`, because changing the user at start needs a root entrypoint, and a root entrypoint is what this image avoids. A bind mount whose owner differs is matched with compose's `user:` (`DOCKER.md`).

**Fixed paths, set as environment.** `UGUISU_BIND=0.0.0.0:8484`, `UGUISU_DATA_DIR=/data`, `UGUISU_MEDIA_DIR=/media/podcasts` and `UGUISU_WEB_DIR=/usr/share/uguisu/web` (ADR 0031's packaged location). `/data` and `/media/podcasts` are volumes, created empty and owned by 65532 in the image, so a named volume starts with the right owner.

**No exposure override in the image.** ADR 0037 holds inside the container: with no password, `serve` exits 12. The first password is set by a one-shot container from stdin, before the server starts; nothing reads it from an argument or an environment variable. The override stays one deliberate `-e` away for a network that is already private.

**Health and stopping.** `HEALTHCHECK` runs `uguisu health`, which asks the server on loopback and needs neither a credential nor a shell. `STOPSIGNAL SIGTERM`: `serve` parks running downloads as `queued(shutdown)` within `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS` (15 s), so a stop timeout of 60 s (`stop_grace_period`, `--stop-timeout`) is documented, against Docker's default of 10. A process that is PID 1 ignores signals it installed no handler for, which `serve` has done only once it is up: `init: true` (`--init`) is documented so a stop during start-up ends it too.

**amd64, built locally, not published.** The image is built from a checkout with `docker build`. v1 publishes no image to a registry and builds no arm64 variant: a published image is a release channel with its own signing and update story, and it is decided with the distribution channel (Phase 11, by a person).

**Checked twice.** `scripts/check-docker.py` holds the rules a reader can check without Docker (digests, the user, the health check, no override, no secret in the context) and runs in the default check set and in CI's `quick` job, which needs no compiler. `scripts/docker_smoke.py` builds and runs the image and plays the release checklist's container scenarios; it needs Docker and runs on request.

## Consequences

- `docker run` without a password exits 12, as the binary does. `DOCKER.md` starts with setting one.
- Nothing in the image can be inspected with a shell; `docker run --rm --entrypoint "" uguisu …` has nothing to run but `uguisu`. Debugging goes through `uguisu` commands and the logs.
- ADR 0013's "Docker defaults to `0.0.0.0:8484` inside the container" is now the image's `UGUISU_BIND`, and ARCHITECTURE.md's `PUID/PGID` is dropped.
- An arm64 image, a registry and image signing are post-v1.

## Alternatives considered

- **`debian:trixie-slim` as the runtime.** A shell and `apt` for debugging, at the cost of about 75 MB and everything a shell makes possible. The binary needs neither.
- **A static musl binary on `scratch`.** Smaller still, but SQLite and the TLS stack would build for a libc no test runs on, and DNS behaves differently under musl. The distroless base costs a few megabytes for the libc the tests use.
- **An entrypoint that takes `PUID`/`PGID` and drops privileges.** A root process in every container's start-up, for a convenience `user:` provides.
- **Setting the first password from an environment variable.** It would sit in `docker inspect`, in compose files and in shell history; CLAUDE.md forbids a credential in the environment.
