# ADR 0045 — Uguisu, part of Suzora

**Status:** accepted · **Date:** 2026-09-30

## Context

The owner groups this project and the tools beside it under one umbrella, **Suzora**. Each tool keeps its own name, but the publisher, the copyright holder and the desktop identifier are chosen once and shared, so that a package, an app store entry or a settings directory says which family it belongs to.

## Decision

| What | Value |
|---|---|
| Product | **Uguisu**: crates `uguisu-*`, binaries `uguisu` and `uguisu-desktop`, variables `UGUISU_*` |
| Umbrella brand | **Suzora**; the product is described as "by Suzora" |
| Publisher, copyright holder | Suzora; "Suzora contributors" in `authors`, the licence and the AppStream developer entry |
| Desktop identifier | `io.github.suzora.Uguisu` (Tauri `identifier`, Flatpak app id, AppStream id) |
| Repository | `https://github.com/suzora/uguisu`: `repository` and `homepage` in the manifests, the AppStream URLs and the default `User-Agent` |

## Consequences

- Every package, the desktop settings directory and the Flatpak sandbox are named after `io.github.suzora.Uguisu`. An identifier is chosen once: changing it later strands those directories.
- The README and [`PRODUCT.md`](../PRODUCT.md) name Suzora, and every other document names only Uguisu.

## Alternatives considered

- **An identifier under the owner's personal namespace.** Rejected: the product belongs to Suzora, and the identifier outlives any one maintainer.
- **Suzora as the product name, Uguisu as an edition.** Rejected: each Suzora tool is a product in its own right, with its own releases.
