# syntax=docker/dockerfile:1

# The list of contents (SBOM) made at release also covers the sources and the build stage, so
# that it names the Rust libraries from Cargo.lock and not only the files of the final image.
ARG BUILDKIT_SBOM_SCAN_CONTEXT=true

# The build stage always runs on the machine that builds, also when the image is for another
# kind of processor: the compiler then produces a program for that processor (cross-compiling).
# Compiling inside an emulated processor instead takes more than ten times as long.
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
ARG BUILDKIT_SBOM_SCAN_STAGE=true
ARG BUILDARCH
ARG TARGETARCH
WORKDIR /src

# What the compiler needs for the target: its standard library and, when the target differs
# from this machine, a C compiler and linker for it. /target.env names them for the build.
RUN set -eu; \
    case "$TARGETARCH" in \
        amd64) rust=x86_64-unknown-linux-gnu; gnu=x86_64-linux-gnu; packages="gcc-x86-64-linux-gnu libc6-dev-amd64-cross" ;; \
        arm64) rust=aarch64-unknown-linux-gnu; gnu=aarch64-linux-gnu; packages="gcc-aarch64-linux-gnu libc6-dev-arm64-cross" ;; \
        *) echo "no build set up for $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    rustup target add "$rust"; \
    echo "RUST_TARGET=$rust" > /target.env; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
        apt-get update; \
        apt-get install -y --no-install-recommends $packages; \
        rm -rf /var/lib/apt/lists/*; \
        variable=$(echo "$rust" | tr 'a-z-' 'A-Z_'); \
        echo "export CARGO_TARGET_${variable}_LINKER=$gnu-gcc" >> /target.env; \
        echo "export CC_$(echo "$rust" | tr '-' '_')=$gnu-gcc" >> /target.env; \
    fi

COPY . .
# Each platform gets caches of its own: two platforms built at once must not write into the
# same registry and target directories.
RUN --mount=type=cache,id=cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-${TARGETARCH},target=/src/target \
    . /target.env \
    && cargo build --release --locked -p uguisu --target "$RUST_TARGET" \
    && cp "target/$RUST_TARGET/release/uguisu" /uguisu \
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
