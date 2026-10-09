# ADR 0007 — SHA-256 as canonical integrity hash

**Status:** accepted · **Date:** 2026-09-17

## Context

Archive integrity requires a cryptographic hash per file, verification runs over potentially terabytes, and manifests that remain useful without Uguisu.

## Decision

- SHA-256 is the canonical hash in v1, computed while streaming during download (no second read) and stored with an explicit `hash_algo` column.
- Per-podcast `manifest.sha256` files are written in `sha256sum -c` format.
- The schema and sidecars are algorithm-tagged so BLAKE3 (or others) can be added by a later ADR once benchmarks on real archives show verification time matters.
- `podcast:integrity` values from feeds (SRI/PGP) are stored when present (nothing compares them yet), and never replace Uguisu's own hash.

## Consequences

- Interoperable with every standard tool; users can verify without Uguisu.
- SHA-256 throughput (~1–2 GB/s with SHA-NI, ~300–500 MB/s without) is well above typical disk read speeds for HDD-backed archives; on fast NVMe with huge archives BLAKE3 could halve verification time — measured, not assumed.

## Alternatives considered

- **BLAKE3 only:** faster, but less interoperable for manifests and unfamiliar to many users.
- **Both always:** doubles CPU during download for little gain; add on demand instead.
