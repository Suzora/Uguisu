# Phase 11 verification, 2026-10-04

What the release checklist asks to be "scripted and green", and the test or script that plays each part. Local runs on Windows 11 (NTFS) and on WSL Ubuntu 26.04 (ext4); CI evidence waits for the November minutes ([ADR 0046](../DECISIONS/0046-ci-within-a-minutes-budget.md)). Commit hashes and CI run numbers that predate publication refer to the pre-publication history, which this repository does not contain.

## Reliability scenarios

"Real" means an actual process, file system or signal; "simulated" means a fail point or a fake the test injects at the same boundary.

| Scenario | How | Test |
|---|---|---|
| Power loss | simulated: a fail point stops the worker at each finalization boundary without touching the database, and the next start repairs it; every state write follows `sync_all` of the file (and, on Unix, of its directory) | `uguisu-download` `crash.rs` `a_restart_repairs_every_crash_boundary`, `two_deaths_end_in_one_file`; `uguisu-engine` `archive_crash.rs` `a_crash_near_finalization_loses_nothing` |
| Process crash | real: `serve` killed with SIGKILL or `TerminateProcess` mid-download at two offsets, an archive import killed mid-copy, a `download --wait` killed mid-transfer | `uguisu-cli` `recovery.rs` `killed_serve_resumes_its_downloads`, `killed_import_finishes_on_rerun`; `download.rs` `killed_download_resumes_with_a_range_request_after_restart` |
| Container restart | real: `docker stop` (SIGTERM) mid-download exits 0, parks the job as `queued(shutdown)`, and a restart resumes it with a Range request; on Unix a SIGTERM to an idle `serve` exits 0 and releases the lock; simulated in-process: a shutdown parks a running job and a new service resumes it | `scripts/docker_smoke.py` (on request); `uguisu-cli` `download.rs` `serve_runs_workers_answers_remote_wait_and_stops_on_sigterm` (`cfg(unix)`); `uguisu-download` `service.rs` `shutdown_parks_jobs_restart_resumes` |
| Incomplete download | real: disconnected, stalled and wrongly ranged bodies from the scenario media server; a resume only with a strong validator | `uguisu-download` `worker.rs` `resume_when_db_and_file_agree`, `resume_reconciles_any_part_length`, `resume_needs_a_strong_validator`, `slow_bodies_finish_but_stalls_time_out`, `bad_range_answers_fail_without_retry` |
| Unavailable feed or enclosure | real: every fetch failure kind against a mock host, enclosures answering each error status, an unreachable feed in an OPML import | `uguisu-engine` `refresh.rs` `every_failure_kind_leaves_state_intact`, `opml.rs` `unreachable_feed_fails_alone`; `uguisu-download` `worker.rs` `status_matrix_and_attempt_budget`, `policy_and_source_problems_fail_for_good`; `uguisu-cli` `library.rs` `exit_codes_reflect_feed_and_network_failures` |
| Disk full | simulated: a space probe reports too little room before an attempt, and a write fails with `StorageFull` at a chosen offset | `uguisu-download` `worker.rs` `a_full_disk_spends_no_attempt`; `crash.rs` `a_disk_full_pause_survives_restart` |
| Database restart | real: the database is closed and opened again after a clean stop and after each kill above; schema upgrades from every earlier phase; a newer database is refused unchanged | `uguisu-download` `crash.rs` `enqueue_and_reconcile_are_idempotent_across_restarts`; `uguisu-storage` `migration_upgrade.rs` (every phase, `newer_database_is_refused`); `uguisu-cli` `upgrade.rs` `every_fixture_upgrades` |

In `recovery.rs`, once the killed work has been finished (after both kills of `serve`, and after a rerun of the import), the archive verifies in full, `db check` is clean, and `archive orphans` finds nothing but at most one scratch copy, which an interrupted copy leaves and nothing deletes.

## Security review

Every control `docs/SECURITY.md` §2–§5 promises was read against the code and the tests, one reader per group of sections, each answering with the code that implements the control, the test that would fail if it broke, and a status. Nothing was taken from the document's word.

| Sections | Verified | Code, no test | Stale wording | Missing |
|---|---|---|---|---|
| §2, §3.1 SSRF | 20 | 7 | 6 | 0 |
| §3.2 Paths | 29 | 7 | 11 | 0 |
| §3.3 Feeds, §3.4 Media, §3.4a Web UI | 37 | 12 | 8 | 1 |
| §3.5 Auth, §3.6 Exposure, §3.6a Desktop | 29 | 7 | 3 | 0 |
| §3.7 Providers, §3.8 Exhaustion | 16 | 8 | 3 | 1 |
| §3.9 Data loss, §3.10 Supply chain, §4, §5 | 30 | 6 | 8 | 3 |
| **All** | **161** | **47** | **39** | **5** |

**Fixed in code**, each with a test that fails without the fix, except `b3a7c4c`, which clippy's `disallowed-methods` enforces for the whole workspace:

| Gap | Commit |
|---|---|
| A proxy from `HTTP_PROXY`/`ALL_PROXY` resolved names itself, bypassing the resolver hook that returns only approved addresses | `aa9350c` |
| `lofty`'s `AudioFile::save_to_path` and `TagType::remove_from_path` edit a file in place and were not forbidden | `b3a7c4c` |
| A `javascript:` episode link or website reached an `href`; only the CSP stopped it | `250af6f` |
| A cross-host redirect forwarded Podcast Index's `X-Auth-Key`, and the provider's base URL could be stored through the API | `4fe6bb3` |
| The download size cap applied per response, so a resumed transfer could exceed it | `bbb217e` |
| A sidecar was read whole before its size was checked, a manifest with no cap at all | `73382ad` |
| "A test asserts no token in a URL" named a test that did not exist | `01a81d8` |

Found while writing the gap tests, also fixed: artwork refused by the network policy answered as a network error (`435c5d5`).

**Corrected in the document:** the 39 stale statements, from "ports … unless configured otherwise" (nothing configures them) to the cookie's `Secure` flag (only ever the explicit setting), the OPTIONS answer (401 once a password is set, not 405) and §5's list of suites.

**Residual, accepted or documented:**
- Concurrent full verification runs are not limited; a `write` credential can start several at once (§3.8).
- A dry-run import names every media file under any folder the server can read, so a `write` credential can enumerate media files outside the archive (§3.2). The import route is a mutation and needs that credential.
- Fuzzing the feed and tag parsers is not built yet (§5); it is Phase 11's next step.
- 47 controls are implemented with no test of their own. The ones a regression would hurt most: a provider-supplied feed URL at a private address, a private `itunes:new-feed-url` announcement, `podcast move-feed --force` to a private address, and the two resolver layers each without the other. Each is enforced by the same `uguisu-http` policy the tested paths go through.
