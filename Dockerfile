# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
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
