# Feed probe fixtures

Hand-written minimal feeds for `uguisu_feed::probe` tests (Phase 2). They are **not** real-world captures; the messy real-feed corpus arrives with Phase 3. Each file's name states what it exercises.

| File | Exercises |
|---|---|
| `rss_itunes_podcast.xml` | RSS 2.0 with iTunes + Podcasting 2.0 tags, 3 items with audio enclosures, self link, `podcast:guid`, `itunes:new-feed-url` |
| `atom_with_enclosures.xml` | Atom feed with `link rel="enclosure"` entries |
| `rss_no_enclosures.xml` | Valid RSS blog feed without enclosures → not a podcast |
| `rss_alternate_enclosure_only.xml` | Item with only `podcast:alternateEnclosure` |
| `rss_latin1.xml` | ISO-8859-1 declared encoding with umlauts |
| `rss_entities_and_cdata.xml` | Titles with `&amp;`, numeric entities and CDATA |
| `malformed.xml` | Unclosed elements |
| `entity_bomb.xml` | Billion-laughs DOCTYPE (must not expand) |
| `html_page.html` | An HTML page served where a feed was expected |
| `rss_rdf.xml` | RSS 1.0 (RDF) feed |
