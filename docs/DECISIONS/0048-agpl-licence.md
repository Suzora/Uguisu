# ADR 0048 — Licence: AGPL-3.0-or-later

**Status:** accepted · **Date:** 2026-10-02

## Context

Uguisu was licensed `MIT OR Apache-2.0`. It is a server that people run for others to use over a network, and a permissive licence lets anyone run a modified copy as a service without sharing the changes. The owner wants every modified Uguisu, whether it is distributed or only offered over a network, to stay free software.

A licence change needs the consent of every copyright holder. Before this decision, every commit was authored by the owner or by an assistant working in the owner's sessions. No tag or release had been published, and the repository was private.

Every dependency was checked for compatibility. That covers 446 crates in the root workspace, 658 in the desktop workspace and 176 npm packages, all of them development-only. Each one is under a licence that AGPL-3.0 can include, or offers such a licence as an option: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0 (none marked "Incompatible With Secondary Licenses"), CC0-1.0, 0BSD or CDLA-Permissive-2.0. GTK and WebKitGTK are LGPL-2.1-or-later system libraries, linked dynamically.

## Decision

| What | Value |
|---|---|
| Licence | GNU Affero General Public License, version 3 or any later version: `AGPL-3.0-or-later` |
| Text | `LICENSE`, the unmodified text published by the FSF |
| Copyright holder | Suzora contributors (ADR 0045) |
| Declared in | `license` in both workspaces' `Cargo.toml` and in `web/package.json`; `project_license` in the AppStream metainfo; `licenseFile` in `tauri.conf.json`; the OpenAPI `info.license`, which is generated from `Cargo.toml` |
| Contributions | Inbound equals outbound: a contribution is licensed under `AGPL-3.0-or-later`. There is no CLA and no sign-off requirement |
| Dependency policy | Unchanged. `deny.toml`'s allow list covers third-party crates only. The project's own crates, all `publish = false`, are skipped through `[licenses.private]` |

## Consequences

- Anyone who distributes Uguisu, or a modified version of it, must offer its source under the same licence. Anyone who runs a modified version for users over a network owes those users its source (section 13).
- Copies obtained before this change are still MIT or Apache-2.0 for whoever holds them. A licence that has been granted cannot be revoked.
- The desktop bundles that take a licence file get `LICENSE` through `licenseFile`. Every package declares the licence identifier.
- A new dependency under GPL-3.0, LGPL-3.0 or AGPL-3.0 would now be compatible. It still needs an explicit entry in the allow list.
- A future relicensing needs the consent of every contributor, because there is no CLA.

## Alternatives considered

- **Stay with `MIT OR Apache-2.0`.** Rejected: it lets anyone run a modified service and keep the changes closed.
- **GPL-3.0-or-later.** Rejected: Uguisu is used over a network, and GPL's obligations start only when a copy is distributed.
- **AGPL-3.0-only.** Rejected: "or later" lets the project take a future FSF revision without asking every contributor again.
- **A built-in section 13 source offer**, such as a source link in the web UI, in `--version` or in `/api/v1/health`. Rejected for now. The unmodified program is published with its source, and the obligation falls on whoever modifies Uguisu and runs it. A link pointing at this repository would also be wrong for every fork that forgot to change it.
- **A CLA or the DCO.** Rejected: extra friction for contributors with no present need. A CLA would only matter for relicensing again.
