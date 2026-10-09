# Test fixtures

Recorded or hand-crafted inputs for unit, integration and end-to-end tests. **Tests never access the network.**

```text
tests/fixtures/
  feeds/parser/   messy real-world feeds: broken dates, duplicate GUIDs, strange
                  Unicode, missing metadata, malformed XML, inconsistent enclosure
                  MIME types, entity bombs, itunes:new-feed-url
  feeds/probe/    minimal feeds for the resolver's streaming probe
  discovery/spec/ provider responses derived from published API documentation,
                  including error shapes (400, 401, 403, malformed JSON)
  discovery/live/ real recorded exchanges, with provenance; see discovery/README.md
  websites/       HTML for autodiscovery: link tags, platform patterns, redirects
                  to private addresses (which must be blocked)
  upgrade/<tag>/  a data directory a tagged build left, and what its --json
                  commands said about it; every later build must open it
                  (ADR 0054, scripts/upgrade_fixture.py)
```

Two fixture sets live elsewhere, deliberately. Foreign-archive trees for import tests are in `crates/uguisu-archive/tests/fixtures/archives/`, because the *names* in the tree are the fixture and they belong next to the matcher. Media for tag tests is **built in code** — a hand-written MPEG frame and a FLAC `STREAMINFO` block are auditable; a committed binary blob is not. The upgrade fixtures are the exception, and only because an old build's database cannot be rebuilt by new code: `fixture.json` says which build made it and how.

Rules:

- Strip personal data; publisher names may be replaced by placeholders.
- No large media.
- Each fixture directory explains what is intentionally broken and which test consumes it.
- Provider fixtures are recorded with `tools/record-fixtures` in a network-enabled environment and committed. The spec tree is never overwritten by a recording, so schema drift shows up as a diff.
