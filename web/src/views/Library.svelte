<!--
  The podcast library, one page at a time.

  The server filters, sorts and pages (ADR 0057); this view keeps the pages it
  has fetched and asks for the next one with the cursor, as the archive does.
-->
<script lang="ts">
  import Artwork from '../lib/components/Artwork.svelte';
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import Pager from '../lib/components/Pager.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { events } from '../lib/events.svelte';
  import { artworkImageUrl, messageFor, opmlExportUrl, podcastArtwork, podcastPage } from '../lib/api';
  import type { PodcastDetail, PodcastPage } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { hrefFor, withQuery } from '../lib/router';
  import { relative } from '../lib/format';
  import { m } from '../lib/i18n';

  interface Props {
    query: Record<string, string>;
  }

  const { query }: Props = $props();

  const PAGE = 48;
  /** The server refuses a longer title filter. */
  const MAX_FILTER = 200;
  const SORTS = {
    title: m.library.sort.title,
    added: m.library.sort.added,
    refreshed: m.library.sort.refreshed,
    episodes: m.library.sort.episodes,
  } as const;
  type Sort = keyof typeof SORTS;

  const first = new Resource<PodcastPage>();
  let podcasts = $state<PodcastDetail[]>([]);
  let cursor = $state<string | null>(null);
  let loadingMore = $state(false);
  let moreError = $state<string | null>(null);
  /** Podcast id → the URL of Uguisu's own copy of its artwork. */
  let covers = $state<Record<string, string>>({});
  /** Replaced on every reload: stops that list's artwork requests and marks its pages stale. */
  let covering = new AbortController();

  const filter = $derived((query.q ?? '').trim());
  const sort = $derived(((query.sort ?? '') in SORTS ? (query.sort as Sort) : 'title'));
  const status = $derived(query.status ?? '');

  async function reload(): Promise<void> {
    covering.abort();
    covering = new AbortController();
    moreError = null;
    const page = await first.load((signal) =>
      podcastPage({ q: filter, status, sort, limit: PAGE }, { signal }),
    );
    if (page) {
      podcasts = page.podcasts;
      cursor = page.next_after ?? null;
      void loadCovers(page.podcasts, covering.signal);
    }
  }

  async function loadMore(): Promise<void> {
    if (!cursor || loadingMore) {
      return;
    }
    loadingMore = true;
    moreError = null;
    const list = covering;
    try {
      const page = await podcastPage({ q: filter, status, sort, after: cursor, limit: PAGE });
      if (list !== covering) {
        return;
      }
      podcasts = [...podcasts, ...page.podcasts];
      cursor = page.next_after ?? null;
      void loadCovers(page.podcasts, covering.signal);
    } catch (error) {
      moreError = messageFor(error);
    } finally {
      loadingMore = false;
    }
  }

  // Artwork is a second request per podcast, so it is fetched after the page
  // is already on screen and a failure only costs a placeholder.
  async function loadCovers(entries: PodcastDetail[], signal: AbortSignal): Promise<void> {
    for (const entry of entries) {
      if (signal.aborted) {
        return;
      }
      try {
        const { current } = await podcastArtwork(entry.podcast.id, { signal });
        const href = artworkImageUrl(entry.podcast.id, current?.hash_value);
        if (href) {
          covers = { ...covers, [entry.podcast.id]: href };
        }
      } catch {
        // A podcast without stored artwork is the normal case, not an error.
      }
    }
  }

  $effect(() => {
    void filter;
    void status;
    void sort;
    void reload();
    return () => {
      first.cancel();
      covering.abort();
    };
  });

  $effect(() =>
    events.subscribe((event) => {
      if (event.kind.startsWith('podcast.')) {
        void reload();
      }
    }),
  );
  $effect(() => events.onResync(() => void reload()));

  function setQuery(patch: Record<string, string | undefined>): void {
    navigation.go(withQuery(navigation.current, patch), { replace: true });
  }
</script>

<div class="row head">
  <h1>{m.library.title}</h1>
  <span class="row">
    <a href={opmlExportUrl()} download="uguisu.opml">{m.library.exportOpml}</a>
    <Link href="/discover" class="add">{m.library.add}</Link>
  </span>
</div>

<form class="row filters" onsubmit={(event) => event.preventDefault()}>
  <label>
    <span class="visually-hidden">{m.library.filter}</span>
    <input
      type="search"
      placeholder={m.library.filter}
      maxlength={MAX_FILTER}
      value={query.q ?? ''}
      oninput={(event) => setQuery({ q: event.currentTarget.value })}
    />
  </label>
  <label>
    <span class="visually-hidden">{m.library.status.label}</span>
    <select value={status} onchange={(event) => setQuery({ status: event.currentTarget.value })}>
      <option value="">{m.library.status.any}</option>
      <option value="active">{m.library.status.active}</option>
      <option value="paused">{m.library.status.paused}</option>
      <option value="error">{m.library.status.error}</option>
      <option value="archived">{m.library.status.archived}</option>
    </select>
  </label>
  <label>
    <span class="visually-hidden">{m.library.sort.label}</span>
    <select value={sort} onchange={(event) => setQuery({ sort: event.currentTarget.value })}>
      {#each Object.entries(SORTS) as [value, label] (value)}
        <option {value}>{label}</option>
      {/each}
    </select>
  </label>
</form>

{#if first.state === 'error'}
  <StateBlock state="error" title={m.library.unreadable} error={first.error} onretry={() => void reload()} />
{:else if first.data === null}
  <StateBlock state="loading" />
{:else if podcasts.length === 0 && (filter || status)}
  <StateBlock state="empty" title={m.library.noMatch} />
{:else if podcasts.length === 0}
  <StateBlock
    state="empty"
    title={m.library.emptyTitle}
    hint={m.library.emptyHint}
  />
{:else}
  <ul class="podcasts">
    {#each podcasts as entry (entry.podcast.id)}
      {@const p = entry.podcast}
      <li class="card">
        <Link href={hrefFor({ name: 'podcast', id: p.id })} class="cover">
          <Artwork src={covers[p.id] ?? p.artwork_url} title={p.title} size={64} />
        </Link>
        <div class="meta">
          <h2>
            <Link href={hrefFor({ name: 'podcast', id: p.id })}>{p.title}</Link>
          </h2>
          {#if p.author}<p class="muted small">{p.author}</p>{/if}
          <p class="row small counts">
            <Badge value={p.status} />
            <span>{m.library.episodes(entry.episodes_total)}</span>
            {#if entry.episodes_present !== entry.episodes_total}
              <span class="muted">{m.library.inFeed(entry.episodes_present)}</span>
            {/if}
            <span class="muted">{m.library.refreshed(relative(p.last_refresh_at))}</span>
            {#if entry.announced}
              <Badge value={m.library.announced} tone="warn" />
            {/if}
          </p>
          {#if p.last_error}
            <p class="small err">{p.last_error}</p>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
  {#if moreError}
    <Notice tone="err">{moreError}</Notice>
  {/if}
  <Pager shown={podcasts.length} more={cursor !== null} busy={loadingMore} onmore={() => void loadMore()} />
{/if}

<style>
  .head {
    justify-content: space-between;
  }

  .filters {
    margin-bottom: var(--gap);
  }

  .filters input {
    min-width: 14rem;
  }

  .podcasts {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.75rem;
    grid-template-columns: repeat(auto-fill, minmax(20rem, 1fr));
  }

  .podcasts li {
    display: flex;
    gap: 0.75rem;
    align-items: start;
  }

  .meta {
    min-width: 0;
  }

  h2 {
    font-size: 1rem;
    margin: 0 0 0.15rem;
  }

  h2 :global(a) {
    color: inherit;
    text-decoration: none;
  }

  h2 :global(a:hover) {
    text-decoration: underline;
  }

  p {
    margin: 0 0 0.25rem;
  }

  .counts {
    gap: 0.5rem;
  }

  .err {
    color: var(--err);
  }
</style>
