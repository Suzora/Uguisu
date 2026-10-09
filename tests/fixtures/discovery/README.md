# Discovery fixtures

One JSON file per HTTP exchange, loaded by `uguisu_discovery::testing::Fixture` and replayed through wiremock. Format and sanitization rules are documented in `crates/uguisu-discovery/src/testing.rs`.

Two trees, never mixed:

| Tree | `origin` | Loader | Written by |
|---|---|---|---|
| `spec/<provider>/<case>.json` | `spec-example` — assembled from the provider's published documentation or OpenAPI examples. Documents the published shape, not live behaviour. | `Fixture::load(provider, case)` | by hand (Phase 2 Part A) |
| `live/<provider>/<case>.json`, `live/web/<host>/<case>.json` | `recorded` — captured from the live service with `tools/record-fixtures` on the date in `recorded_at`, credentials stripped, long arrays truncated with a note. Proves live behaviour on that date. | `Fixture::load_live(provider, case)` | `record-fixtures` (default output root) |

Keeping both lets a re-recording show schema drift as a diff (`record-fixtures diff spec/apple/search_darknet.json live/apple/search_darknet.json`) and keeps the "implemented against docs" and "verified live" evidence apart.

## The release checklist's scenarios

The eight discovery scenarios Phase 11 names, and the test that plays each. The four in `scenarios.rs` build their provider answers inline: they are about Uguisu's merging and ranking, not about a provider's shape.

| Scenario | Test | Shows |
|---|---|---|
| common | `e2e.rs` `misspelled_query_merges_three_providers_and_resolves` | three providers merged into one ranked show, under the soft deadline, resolved to a feed |
| obscure | `scenarios.rs` `obscure_show_ranks_and_resolves` | a show only one directory knows ranks above a popular show that shares a word, and resolves |
| similarly named | `scenarios.rs` `similar_names_stay_apart` | two shows with one title stay two results, each naming the other as possibly the same |
| misspelled | `e2e.rs` `misspelling_recalled_from_recordings` | a typo no directory answers is recalled by a looser query (recorded answers) |
| multi-feed | `live_fixtures.rs` `resolver_replays_chaosradio_multiple_feed_links`, `gpoddernet_live_lists_feed_variants` | a site announcing three feeds, a directory listing feed variants |
| provider disagreement | `scenarios.rs` `disagreeing_providers_keep_provenance` | one show with two feed addresses: Podcast Index's wins, the artwork stays Apple's, and the provenance says so |
| website only | `live_fixtures.rs` `resolver_replays_darknetdiaries_autodiscovery_and_redirect_chain`, `resolver_replays_relative_and_absolute_link_tags`, `resolver_replays_a_site_without_feeds` | a website URL resolved to its feed through link tags and redirects, or refused when it has none |
| dead provider | `e2e.rs` `outages_yield_partial_then_all_failed`, `scenarios.rs` `unreachable_provider_is_survivable` | a provider that errors, stalls or cannot be reached costs its own results only |

## `spec/` (2026-09-17, Part A)

| Provider | Case | Source |
|---|---|---|
| apple | `search_darknet`, `search_empty`, `search_missing_feedurl`, `lookup_id`, `error_403`, `malformed` | developer.apple.com iTunes Search API docs (field names) + observed podcast result keys; synthetic error/malformed bodies |
| podcastindex | `search_byterm`, `search_darknet`, `search_empty`, `byfeedurl`, `byitunesid`, `error_401`, `error_400`, `malformed` | `docs/pi_api.yaml` per-field examples and error responses |
| gpoddernet | `search_floss`, `search_darknet`, `search_empty`, `podcast_by_url`, `podcast_by_url_404`, `malformed` | `mygpo/doc/api/reference/directory.rst` examples |

## `live/` (recorded 2026-09-17, Part B)

| Provider | Case | What it shows |
|---|---|---|
| apple | `search_darknet` | 3 results for "Darknet Diaries": the show, its Spanish and German editions; real feed URL `podcast.darknetdiaries.com`; `genres` includes "Podcasts"; `max-age=86400` |
| apple | `search_misspelled` | "darknet diariez" → `resultCount: 0` (Apple search is not fuzzy) |
| apple | `search_author` | "Jack Rhysider" → 5 results (author match works) |
| apple | `search_partial` | "darknet" → 25 results |
| apple | `search_empty` | nonsense term → empty |
| apple | `search_german` | "Gemischtes Hack", storefront `DE`; `country: "DEU"` |
| apple | `lookup_id` | `lookup?id=1296350485` |
| apple | `lookup_unknown` | `lookup?id=1` → `200`, `resultCount: 0` |
| gpoddernet | `search_darknet` | 4 feed variants of one show, all with `subscribers: 0`, `http://` logo URLs, `max-age=3600` |
| gpoddernet | `search_misspelled`, `search_author`, `search_partial`, `search_empty`, `search_german` | same queries as Apple |
| gpoddernet | `podcast_by_url` | `api/2/data/podcast.json?url=https://feeds.megaphone.fm/darknetdiaries` |
| gpoddernet | `podcast_by_url_404` | unknown feed → `404` with an HTML body |
| podcastindex | `error_401_invalid_key`, `byfeedurl_401` | invalid key → `401`, plain-text body despite `application/json`, `no-cache` |
| fyyd | `search_darknet` | reference only (captured with curl): live `0.2/search/podcast` schema; no fyyd provider exists |
| web/darknetdiaries_com | `darknetdiaries_home` | website with one absolute `<link rel="alternate">` to a redirecting feed host |
| web/chaosradio_de | `chaosradio_home` | three announced feeds (mp3/m4a/opus) |
| web/atp_fm | `atp_home` | relative `href="/rss"` |
| web/twit_tv | `twit_security_now` | absolute link to `feeds.twit.tv/sn.xml` |
| web/www_buzzsprout_com | `buzzsprout_show_page` | oembed link plus RSS link |
| web/www_gemischteshack_de | `gemischteshack_home` | SPA without feed links (catch-all HTML) |
| web/anchor_fm, feeds_feedburner_com, rss_buzzsprout_com, rss_libsyn_com, feeds_twit_tv | `*_feed` | podcast feeds from five hosts; the Anchor and TWiT bodies are placeholder feeds in the recorded shape, and the TWiT one keeps its self-referencing `itunes:new-feed-url` |
| web/feeds_buzzsprout_com | `buzzsprout_feed_redirect` | recorded after a `301` to `rss.buzzsprout.com` (see `notes`) |

What was changed after recording, and why, is in each file's `notes`: the five website pages that announce a feed keep only their `<title>` and autodiscovery links, the Anchor feed (a private person's show) and the TWiT feed (CC BY-NC-ND) are placeholders, and the Apple captures replace the names of hosts who are not public figures.

Not recorded on purpose: the 1 MB `podcast.darknetdiaries.com` feed and the 0.5–3 MB Simplecast/Transistor/ATP/Lex Fridman feeds and pages (too large for the repository; validated live only).

Podcast Index authenticated responses are missing because no key was available; record them with `UGUISU_PODCASTINDEX_KEY`/`SECRET` set.
