# Parser corpus (Phase 3)

Hand-made feeds that exercise the parser, the normalizer and the refresh
pipeline. Each file targets one behaviour named in the Phase 3 brief (§37).

| File | Exercises |
|---|---|
| `minimal_rss.xml` | smallest valid RSS 2.0 podcast feed |
| `minimal_atom.xml` | smallest valid Atom feed with `link rel="enclosure"` |
| `missing_guid.xml` | no GUIDs → enclosure-URL identity; one item without media |
| `duplicate_guid.xml` | two items share a GUID → cascade falls back to the enclosure URL |
| `dates.xml` | missing, invalid, future, ancient, zoneless, offset, `dc:date`, seconds-less dates |
| `enclosures.xml` | no enclosure, primary + two `alternateEnclosure`s with sources/integrity, unknown MIME, bad length, `media:content` only, enclosure without URL, upper-case MIME |
| `durations.xml` | `H:MM:SS`, `MM:SS`, seconds, fractional, invalid, missing, padded, absurd, `1h 2m` |
| `unicode_heavy.xml` | NFC/NFD, fullwidth, RTL, emoji in titles and authors |
| `very_long_title.xml` | ~6 KB title → capped at `max_field_bytes` |
| `html_cdata_empty.xml` | HTML in description, CDATA titles, escaped markup, `content:encoded` with `<script>`, empty elements |
| `duplicate_episodes.xml` | the same item twice → one episode |
| `new_feed_url.xml` | `itunes:new-feed-url` differing from `atom:link rel="self"` |
| `podcasting20.xml` | the full iTunes + Podcasting 2.0 surface incl. `value`, nested categories, unknown namespace kept raw |
| `malformed_item.xml` | one item with a mismatched end tag between two good ones → isolated |
| `truncated.xml` | document cut mid-item → completed items kept, `truncated = true` |
| `odd_prefixes.xml` | namespaces declared under unusual prefixes → resolved by URI |
| `episodes_v1.xml` / `episodes_v2.xml` | refresh series: v2 adds one episode, updates one (title, description, enclosure URL/length, chapters), removes one |
| `guid_changed.xml` | v1 items where one GUID changed (same enclosure) and one GUID vanished → ADR 0014 branches |
