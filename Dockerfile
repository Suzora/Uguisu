# syntax=docker/dockerfile:1
# The list of contents (SBOM) made at release also covers the sources and the build stage, so
# that it names the Rust libraries from Cargo.lock and not only the files of the final image.
ARG BUILDKIT_SBOM_SCAN_CONTEXT=true

FROM rust:1-bookworm AS build
ARG BUILDKIT_SBOM_SCAN_STAGE=true
WORKDIR /src
COPY . .
# Each platform gets caches of its own: two platforms built at once must not write into the
# same registry and target directories.
ARG TARGETARCH
RUN --mount=type=cache,id=cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-${TARGETARCH},target=/src/target \
    cargo build --release --locked -p uguisu \
    && cp target/release/uguisu /uguisu \
    && mkdir /data

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /uguisu /usr/local/bin/uguisu
COPY --from=build --chown=nonroot:nonroot /data /data
ENV UGUISU_DATA_DIR=/data \
    UGUISU_LISTEN=0.0.0.0:8484
VOLUME /data
EXPOSE 8484
HEALTHCHECK --interval=30s --timeout=6s --start-period=20s --retries=3 \
    CMD ["/usr/local/bin/uguisu", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/uguisu"]
