<!--
  Local search over the FTS5 index.

  The five outcomes the backend distinguishes stay distinct here: an empty
  query, an index that is still building, an index that has gone stale, a
  query that matched nothing, and results. Collapsing them into "no results"
  would hide the one case the user can actually fix.
-->
<script lang="ts">
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { messageFor, reindex, searchLibrary } from '../lib/api';
  import type { LibrarySearchResults } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { hrefFor, withQuery } from '../lib/router';
  import { date, duration } from '../lib/format';
  import { m } from '../lib/i18n';

  interface Props {
    query: Record<string, string>;
  }

  const { query }: Props = $props();

  const results = new Resource<LibrarySearchResults>();
  let text = $state('');
  let explained = $state<string | null>(null);
  let rebuilding = $state(false);
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let debounce: ReturnType<typeof setTimeout> | null = null;

  const kind = $derived(query.kind ?? 'all');

  $effect(() => {
    const q = query.q ?? '';
    text = q;
    void results.load((signal) => searchLibrary({ q, kind, limit: 25 }, { signal }));
  });

  function onInput(value: string): void {
    text = value;
    if (debounce !== null) {
      clearTimeout(debounce);
    }
    debounce = setTimeout(() => {
      navigation.go(withQuery(navigation.current, { q: value }), { replace: true });
    }, 250);
  }

  async function rebuild(): Promise<void> {
    rebuilding = true;
    notice = null;
    try {
      const report = await reindex();
      notice = {
        tone: 'ok',
        text: m.search.reindexed(report.episodes, report.podcasts, report.duration_ms),
      };
      void results.load((signal) => searchLibrary({ q: query.q ?? '', kind }, { signal }));
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      rebuilding = false;
    }
  }

  const found = $derived(results.data);
  const total = $derived((found?.podcasts.length ?? 0) + (found?.episodes.length ?? 0));
</script>

<h1>{m.search.title}</h1>
<p class="muted">{m.search.intro}</p>

<form class="row" onsubmit={(event) => event.preventDefault()}>
  <label class="grow">
    <span class="visually-hidden">{m.search.text}</span>
    <input
      type="search"
      placeholder={m.search.placeholder}
      value={text}
      oninput={(event) => onInput(event.currentTarget.value)}
    />
  </label>
  <label>
    <span class="visually-hidden">{m.search.kind}</span>
    <select
      value={kind}
      onchange={(event) =>
        navigation.go(withQuery(navigation.current, { kind: event.currentTarget.value }), {
          replace: true,
        })}
    >
      <option value="all">{m.search.kinds.all}</option>
      <option value="podcasts">{m.search.kinds.podcasts}</option>
      <option value="episodes">{m.search.kinds.episodes}</option>
    </select>
  </label>
</form>

{#if notice}
  <Notice tone={notice.tone}>{notice.text}</Notice>
{/if}

{#if results.state === 'error'}
  <StateBlock
    state="error"
    title={m.search.failed}
    error={results.error}
    onretry={() => void results.load((signal) => searchLibrary({ q: text, kind }, { signal }))}
  />
{:else if found === null}
  <StateBlock state="loading" />
{:else if found.outcome === 'empty_query'}
  <StateBlock
    state="empty"
    title={m.search.empty.title}
    hint={m.search.empty.hint(found.index.episodes, found.index.podcasts)}
  />
{:else if found.outcome === 'index_building'}
  <StateBlock
    state="loading"
    title={m.search.building.title}
    hint={m.search.building.hint(found.index.episodes, found.index.detail)}
  />
{:else if found.outcome === 'index_stale' && total === 0}
  <div class="stale">
    <StateBlock
      state="empty"
      title={m.search.stale.title}
      hint={m.search.stale.hint(found.index.built_at ? date(found.index.built_at) : null)}
    />
    <button type="button" class="primary" disabled={rebuilding} onclick={() => void rebuild()}>
      {rebuilding ? m.search.rebuilding : m.search.stale.rebuild}
    </button>
  </div>
{:else if found.outcome === 'no_results'}
  <StateBlock
    state="empty"
    title={m.search.noResults.title(found.terms)}
    hint={found.truncated ? m.search.noResults.truncated : undefined}
  />
{:else}
  {#if found.outcome === 'index_stale'}
    <Notice tone="info">
      {m.search.stale.notice}
      <button type="button" class="linky" disabled={rebuilding} onclick={() => void rebuild()}>
        {rebuilding ? m.search.rebuilding : m.search.stale.rebuildIt}
      </button>
    </Notice>
  {/if}
  <p class="muted small">{m.search.summary(total, found.duration_ms)}</p>

  {#if found.podcasts.length > 0}
    <h2>{m.search.results.podcasts}</h2>
    <ul class="hits">
      {#each found.podcasts as hit (hit.podcast_id)}
        <li>
          <p class="name">
            <Link href={hrefFor({ name: 'podcast', id: hit.podcast_id })}>{hit.title}</Link>
            {#if hit.author}<span class="muted small">{hit.author}</span>{/if}
          </p>
          <p class="small snippet">{hit.snippet}</p>
          <p class="small">
            <button
              type="button"
              class="linky"
              aria-expanded={explained === hit.podcast_id}
              onclick={() => (explained = explained === hit.podcast_id ? null : hit.podcast_id)}
            >
              {m.search.results.score(hit.score.toFixed(2))}
            </button>
          </p>
          {#if explained === hit.podcast_id}
            <ul class="signals small muted">
              {#each hit.signals as signal (signal.name)}
                <li>{signal.name}: {signal.contribution.toFixed(2)} — {signal.note}</li>
              {/each}
            </ul>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if found.episodes.length > 0}
    <h2>{m.search.results.episodes}</h2>
    <ul class="hits">
      {#each found.episodes as hit (hit.episode_id)}
        <li>
          <p class="name">
            <Link href={hrefFor({ name: 'episode', id: hit.episode_id })}>{hit.title}</Link>
            <Badge value={hit.archive_state} />
          </p>
          <p class="muted small">
            <Link href={hrefFor({ name: 'podcast', id: hit.podcast_id })}>{hit.podcast_title}</Link> · {date(hit.published_at)}
            {#if hit.duration_secs}· {duration(hit.duration_secs)}{/if}
          </p>
          <p class="small snippet">{hit.snippet}</p>
          <p class="small">
            <button
              type="button"
              class="linky"
              aria-expanded={explained === hit.episode_id}
              onclick={() => (explained = explained === hit.episode_id ? null : hit.episode_id)}
            >
              {m.search.results.score(hit.score.toFixed(2))}
            </button>
          </p>
          {#if explained === hit.episode_id}
            <ul class="signals small muted">
              {#each hit.signals as signal (signal.name)}
                <li>{signal.name}: {signal.contribution.toFixed(2)} — {signal.note}</li>
              {/each}
            </ul>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
{/if}

<style>
  form {
    margin-bottom: var(--gap);
  }

  .grow {
    flex: 1 1 18rem;
  }

  .grow input {
    width: 100%;
  }

  .hits {
    list-style: none;
    margin: 0 0 var(--gap);
    padding: 0;
  }

  .hits li {
    padding: 0.6rem 0;
    border-bottom: 1px solid var(--border);
  }

  p {
    margin: 0 0 0.2rem;
  }

  .name {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: baseline;
    font-weight: 600;
  }

  .snippet {
    color: var(--muted);
  }

  .signals {
    margin: 0.25rem 0 0 1rem;
    padding: 0;
  }

  .linky {
    border: none;
    background: none;
    padding: 0;
    min-height: 0;
    color: var(--accent);
    text-decoration: underline;
  }

  .stale {
    display: grid;
    gap: 0.75rem;
    justify-items: center;
  }
</style>
