# Running Uguisu in Docker

The image runs `uguisu serve` as uid 65532 on a distroless base, with the web UI built in ([ADR 0061](docs/DECISIONS/0061-the-docker-image.md)). It is built from a checkout; no image is published to a registry. Everything about the server itself, the reverse proxy included, is in [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md); this page is what is different in a container.

| Path in the container | What | Set by |
|---|---|---|
| `/data` | the database, its lock and backups | `UGUISU_DATA_DIR` |
| `/media/podcasts` | the archive | `UGUISU_MEDIA_DIR` |
| `/usr/share/uguisu/web` | the web UI | `UGUISU_WEB_DIR` |
| `0.0.0.0:8484` | the listener | `UGUISU_BIND` |

## First start

Build the image, then set a password **before** the server starts. A server that binds `0.0.0.0` with no password refuses to start, exit code 12, and a restart policy turns that into a restart loop (`docker compose logs` says `refusing to bind 0.0.0.0:8484 without authentication`).

```bash
docker build -t uguisu .
docker volume create uguisu-data
docker volume create uguisu-media
```

The password is read from stdin, never from an argument or the environment, so it reaches neither `docker inspect` nor your shell history:

```bash
docker run --rm -i -v uguisu-data:/data -v uguisu-media:/media/podcasts \
  uguisu auth set-password --username admin < password.txt
```

Type it instead with `-it` in place of `-i` and no redirect. A token for scripts and the CLI comes the same way, while the server is not running, since a running server holds the data directory's lock; or later from the web UI's Settings page:

```bash
docker run --rm -v uguisu-data:/data -v uguisu-media:/media/podcasts \
  uguisu --json auth token create scripts --scope write
```

Every one-shot mounts both volumes: opening the database checks that the archived files are there, and with the media volume left out it would find none and mark every one `missing`.

## Compose

```yaml
services:
  uguisu:
    image: uguisu
    init: true                 # a stop during start-up reaches the server too
    restart: unless-stopped
    stop_grace_period: 60s     # running downloads are parked, not cut off
    ports:
      - "127.0.0.1:8484:8484"  # a reverse proxy in front; see docs/DEPLOYMENT.md
    volumes:
      - uguisu-data:/data
      - uguisu-media:/media/podcasts
volumes:
  uguisu-data:
    external: true             # the volumes created above, not new empty ones
  uguisu-media:
    external: true
```

`docker stop` sends SIGTERM. The server parks running downloads as `queued(shutdown)` within `UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS` (15 s) and the next start resumes them with a range request; Docker's default of 10 s would kill it first, which is what `stop_grace_period` (`docker run --stop-timeout 60`) is for. The image's health check is `uguisu health`, which `docker ps` shows as `healthy` once the server answers.

Settings go in as environment variables, all of them listed in [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md); `env_file:` is the file-based configuration (ADR 0028).

## Behind a reverse proxy

To give each client its own login bucket, name the proxy with `UGUISU_TRUSTED_PROXIES` ([`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md), "What Uguisu trusts from a proxy"), by the address the container sees, which is never `127.0.0.1`. A proxy on the host reaches the container from its network's gateway, and a proxy in another container from that container's address. Pin the network's subnet so the gateway stays put:

```yaml
services:
  uguisu:
    environment:
      UGUISU_TRUSTED_PROXIES: 172.30.84.1    # the gateway: a proxy on the host
networks:
  default:
    ipam:
      config:
        - subnet: 172.30.84.0/24
```

A proxy service in the same file gets a fixed `ipv4_address` on that network, and that address is the one to name. Named wrongly, nothing breaks: every client shares the proxy's bucket, as with no proxy named.

## Bind mounts

A named volume starts owned by 65532. A host directory keeps its own owner, so run the container as that owner:

```yaml
    user: "1000:1000"          # the owner of /srv/podcasts
    volumes:
      - /srv/uguisu:/data
      - /srv/podcasts:/media/podcasts
```

The two directories must be writable by that user. A media directory on another file system than the data directory is fine; one under the data directory is the default outside a container.

## Upgrading

Back up, build the new image, start it. Migrations run when the server opens the database, and an older image refuses a database a newer one migrated (see `docs/DEPLOYMENT.md`, "Upgrading").

```bash
docker compose stop uguisu
docker compose run --rm uguisu db backup     # into /data/backups
docker build -t uguisu .                       # from the new checkout
docker compose up -d
```

## Backups

What to keep is in `docs/DEPLOYMENT.md`, "Backups": the media volume first, then the database. `db backup` as a one-shot with the service stopped, as above, or `uguisu --server … db backup` from outside while it runs, writes a consistent copy into `/data/backups`. Copy the volumes with the service stopped; a database file copied while it is written can be torn.

## Moving from Podgrab

Mount Podgrab's two directories read-only and import with the service stopped ([`docs/MIGRATION.md`](docs/MIGRATION.md)):

```bash
docker compose stop uguisu
docker compose run --rm \
  -v /srv/podgrab/assets:/podgrab/assets:ro \
  -v /srv/podgrab/config:/podgrab/config:ro \
  uguisu archive import /podgrab/assets --podgrab-db /podgrab/config/podgrab.db
# read the plan, then the same command with --apply
docker compose start uguisu
```

The web UI's import page takes the same paths, as the server sees them, while it runs.

## What the image does not have

No shell, no package manager and no root user: `docker exec` can run `uguisu` and nothing else. Look at a running server with `docker compose logs`, `uguisu health`, or `uguisu --server http://127.0.0.1:8484 …` from the host. The image is built for amd64 only.
