# ADR 0065 — Fuzzing: its own workspace, run by hand

**Status:** accepted, amends [0012](0012-metadata-tagging.md) · **Date:** 2026-10-10

## Context

Three parsers read what a stranger wrote: Uguisu's own feed parser ([ADR 0008](0008-feed-parser.md)), the OPML reader ([ADR 0049](0049-opml-import-and-export.md)), and `lofty`, through `uguisu-metadata`, for the tags and the length of audio files. Property tests feed them random bytes and XML-like text. Coverage-guided fuzzing goes further, because it keeps every input that reaches new code and mutates from there.

[`SECURITY.md`](../SECURITY.md) and the ROADMAP planned fuzzing "run locally or by hand, never nightly". `cargo fuzz` needs a nightly compiler, and libFuzzer instruments every crate it builds.

The first runs found three ways a hostile audio file stops `lofty` 0.25.4, the newest release:
- a 30-byte ADTS file sends its reader seeking for ever;
- an Opus last-page granule position overflows when it is turned into milliseconds;
- a Musepack SV8 sample count does the same.

None of them is fixed or reported upstream. An import that read such a file never finished, and the overflows panic wherever overflow checks are on.

They also found `lofty` writing files that do not read back as written: as another container, without the values, or not at all. The engine renamed such a copy into place as long as it parsed, and a sync that wrote values `lofty` does not read back wrote them again on every run. A review of the fixes found one more: `lofty`'s MP4 writer recurses once per nested container atom, so a file of nested `moov` headers overflows the stack.

## Decision

**`fuzz/` is a workspace of its own,** like `desktop/`:
- `fuzz/rust-toolchain.toml` pins a dated nightly, so a finding replays on the compiler that found it.
- Its lockfile carries the root's versions. Only `libfuzzer-sys` and `arbitrary` are added.
- Its lints are the root's, restated, with two differences. `unsafe_code` is allowed, because `fuzz_target!` expands to the entry point libFuzzer calls. `expect` and `unwrap` are allowed, because a panic is how a target reports a finding.
- `.dockerignore` keeps it out of the image's build context.

**Six targets,** each the way Uguisu calls the code:

| Target | Calls | Asserts beyond "no panic, no hang" |
|---|---|---|
| `feed_parse` | `parse`, `normalize_channel`, `normalize_item`, `signals`, `resolve_identities`, `comparable_hash` | — |
| `feed_probe` | `probe` | — |
| `opml_parse` | `opml::parse`, then `write` of the URLs an import could store, then `parse` | the round trip keeps every feed |
| `tags_read` | `read_tags` | — |
| `identity_read` | `read_identity` | — |
| `tags_write` | `write_tags` | after a write that counted, a second `sync` write changes nothing |

The three metadata targets put Rust's default panic hook back. libFuzzer's own hook would abort on a panic that `uguisu-metadata` catches; a panic nothing catches still ends in libFuzzer's abort.

**The corpus is generated, never checked in.** The `seeds` package writes the containers that `uguisu-metadata`'s tests build, bare and tagged with every managed field, plus a feed and an OPML file. The feed fixtures are a second corpus directory. `corpus/` and `artifacts/` are ignored by git.

**`python3 scripts/check.py fuzz` runs every target for 60 seconds.**
- It is part of `all`, never part of CI, and it is skipped where `cargo-fuzz` is missing.
- The per-input timeout is 60 seconds, above `uguisu-metadata`'s deadline for a small file, so a file Uguisu gives up on is an error and only a real hang is a timeout.

**Fuzzing the metadata targets is done on Linux,** in WSL or the `rust` image, where libFuzzer saves each finding's input to `fuzz/artifacts/`. On Windows:
- Only AddressSanitizer builds link: without a sanitizer, the MSVC linker cannot resolve the coverage section symbols. `check.py` finds ASan's runtime in the MSVC installation with `vswhere`.
- That runtime cannot unwind a Rust panic, so the panics `uguisu-metadata` catches would end the process. `check.py fuzz` runs only the feed and OPML targets there.
- A panic ends the process through `__fastfail`, which libFuzzer cannot catch, so a finding comes without its input.

**A finding becomes a test in the crate that owns the code.** The input is minimized with `cargo fuzz tmin` and embedded in the test, or built there when its structure is plain.

**A finding in a dependency gets a guard in the crate that owns the dependency.** It is also reported upstream. For `lofty` that crate is `uguisu-metadata` ([ADR 0012](0012-metadata-tagging.md)):
- Every read and write goes through a file handle that fails once its deadline has passed: `DEADLINE`, 30 seconds, plus one second for every 4 MiB of the file. Every loop fuzzing found reads or seeks, so the handle ends it.
- A parse that ran past the deadline is refused, whatever `lofty` returned. `lofty` swallows some failed reads and returns what it had by then.
- A panic inside `lofty` is caught and becomes that file's `Unreadable` or `NotWritten` error.
- A write counts only when the file reads back as the same container, with every value written. Otherwise it is `NotWritten`, and the engine discards the copy.
- An MP4 whose container atoms nest deeper than 16 levels is not written. A stack overflow is no panic, so it is refused before `lofty`'s writer can recurse. Real files nest five deep.

## Consequences

- Nothing runs the fuzzers by itself. A parser change is fuzzed when someone runs `check.py fuzz` or `all`; [`SECURITY.md`](../SECURITY.md) §5 lists it with the release gate.
- The nightly pin is moved by hand. A new nightly that breaks `libfuzzer-sys` fails `check.py fuzz`, not the build.
- A file that takes `lofty` longer than its deadline is refused as unreadable. The slowest kind measured, a synthetic three-hour AAC file that `lofty` reads frame by frame, took 2.5 seconds of its 71.
- A hostile file is small, so it is given up on after about 30 seconds; a large one can hold a worker for as long as its size allows.
- A caught panic still prints `lofty`'s message to standard error through Rust's panic hook. Uguisu's own report of the file is the error.
- A tag write on a file `lofty` cannot write back faithfully fails, and the file keeps its bytes. Before, it was tagged in a way nothing reads ([ADR 0026](0026-tagging-and-artwork.md)).
- `check.py fuzz` on Windows does not fuzz the tag code. Fuzzing it takes Linux.

## Alternatives considered

- **Fuzzing in CI, nightly or per pull request.** The ROADMAP ruled out nightly runs. A minute per target per push would take a third of ADR 0063's budget, and a fuzzer that finds something at random would turn a pull request red for a reason unrelated to it.
- **The targets inside the root workspace.** That would put nightly and libFuzzer's instrumentation into `--workspace` checks.
- **A thread with a timeout around `lofty`.** The thread would go on spinning after Uguisu gave up on it. A handle that fails ends the loop itself.
- **A budget of bytes or operations instead of time.** `lofty` reads AAC frame by frame through a buffered reader that it seeks after every frame, so a legitimate long file costs many times its size. No fixed ratio separates such a file from a spinning one.
- **A flat deadline.** `lofty` reads and rewrites a whole MP4 to tag it, so a large video on slow storage could never be tagged.
- **A patched `lofty`.** Carrying a fork costs every update. A guard plus upstream reports keeps the fix where it belongs.
- **Running the metadata targets in a container from `check.py` on Windows.** It would make Docker a requirement of the check for a run that is by hand anyway.
