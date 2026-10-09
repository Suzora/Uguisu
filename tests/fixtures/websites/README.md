# Website fixtures

Hand-written HTML pages for feed autodiscovery tests (`uguisu_discovery::resolve`). Served through wiremock in tests; not real captures. Real pages are recorded in Phase 2 Part B with `record-fixtures url`.

| File | Exercises |
|---|---|
| `link_absolute.html` | one `<link rel="alternate" type="application/rss+xml">` with an absolute href |
| `link_relative_base.html` | relative href resolved against `<base href>` |
| `link_multiple.html` | comments feed, Atom feed and a podcast-titled RSS feed; the podcast one must win |
| `no_links.html` | no feed links at all → well-known paths |
| `links_all_html.html` | many feed links that all point to HTML pages → budget / no feed found |
