# Benchmarks

Performance is a core requirement and is measured, not claimed (`docs/PRODUCT.md`). Benchmarks use `criterion`; all ten targets live under `crates/<crate>/benches/` and this directory holds only this note. `python3 scripts/check.py bench` proves they compile; `cargo bench --workspace` runs them.

| Area | Target |
|---|---|
| discovery | ranking and dedup over 1k candidates, SSRF and URL classification — `uguisu-discovery`, `uguisu-http` |
| feed | parsing a 10k-item feed — `uguisu-feed` |
| refresh | a refresh against a local server — `uguisu-engine` |
| download | concurrent throughput on loopback, resume and finalization overhead, enqueue and first-claim latency — `uguisu-download` |
| archive | template parse and render, sanitization per profile, collision resolution, path containment, verification at all three depths, policy evaluation — `uguisu-archive` |
| tags | writing and reading 1k files per format — `uguisu-metadata` |
| schedule | slot derivation and due-time planning — `uguisu-core` |
| service | the scheduler's per-wake queries, FTS search and reindex, settings — `uguisu-storage` |
| argon2 | one password verification at the shipped parameters and around them, which sets what a login costs — `uguisu-engine` |

Results go in `docs/benchmarks/<date>-<subject>.md` with the machine, the toolchain and the command, so a regression is visible and a number can be reproduced.
