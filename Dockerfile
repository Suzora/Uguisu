# syntax=docker/dockerfile:1
#
# The Uguisu server image (ADR 0061). The web UI is built with the pnpm the
# repository pins, the binary with the toolchain it pins, and both land on a
# distroless glibc base that has no shell and runs as uid 65532. Every base is
# pinned by digest; a new digest is a reviewed change, like a Cargo.lock bump.

FROM node:22-trixie-slim@sha256:b26b04c123d9ff8ab646ceb18b9d75a1173acf64b9a401094b906d27b29338d4 AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1.98.1-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7 AS server
WORKDIR /src
COPY . .
# The registry and target directories are caches kept between builds on this
# machine, never layers: a changed source file rebuilds what changed, not the
# dependency tree.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p uguisu-cli \
 && cp target/release/uguisu /usr/local/bin/uguisu \
 && mkdir -p /volumes/data /volumes/media/podcasts

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2
COPY --from=server /usr/local/bin/uguisu /usr/local/bin/uguisu
COPY --from=web /src/web/dist /usr/share/uguisu/web
# Empty, owned by the user that runs: a named volume mounted here starts with
# that owner. A bind mount keeps the host's, which DOCKER.md explains.
COPY --from=server --chown=65532:65532 /volumes/data /data
COPY --from=server --chown=65532:65532 /volumes/media /media
ENV UGUISU_BIND=0.0.0.0:8484 \
    UGUISU_DATA_DIR=/data \
    UGUISU_MEDIA_DIR=/media/podcasts \
    UGUISU_WEB_DIR=/usr/share/uguisu/web
USER 65532:65532
VOLUME ["/data", "/media/podcasts"]
EXPOSE 8484
# serve parks running downloads on SIGTERM and needs up to its shutdown grace
# (UGUISU_DOWNLOAD_SHUTDOWN_GRACE_MS) to do so: give `docker stop` that long.
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --start-interval=2s --retries=3 \
    CMD ["/usr/local/bin/uguisu", "health"]
ENTRYPOINT ["/usr/local/bin/uguisu"]
CMD ["serve"]
