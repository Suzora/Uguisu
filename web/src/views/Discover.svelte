<!--
  Finding a podcast that is not in the library yet.

  Three things stay distinct here, because the API keeps them distinct: a
  provider's search result, a feed that was actually resolved and verified,
  and a podcast that now exists locally. A result becomes a podcast only
  after `POST /api/v1/podcasts` says so.
-->
<script lang="ts">
  import Artwork from '../lib/components/Artwork.svelte';
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import { webLink } from '../lib/links';
  import Notice from '../lib/components/Notice.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import {
    addPodcast,
    importOpml,
    listPodcasts,
    messageFor,
    providers,
    resolveFeed,
    searchProviders,
  } from '../lib/api';
  import type {
    DiscoverySearchResponse,
    OpmlAction,
    OpmlImport,
    PolicyMode,
    ProviderStatus,
    RankedCandidate,
    ResolvedFeed,
  } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { hrefFor, withQuery } from '../lib/router';
  import { m } from '../lib/i18n';

  interface Props {
    query: Record<string, string>;
  }

  const { query }: Props = $props();

  const search = new Resource<DiscoverySearchResponse>();
  const status = new Resource<ProviderStatus[]>();
  let text = $state('');
  let resolved = $state<ResolvedFeed | null>(null);
  let resolving = $state<string | null>(null);
  let adding = $state<string | null>(null);
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let explained = $state<number | null>(null);
  /** Feed URLs already in the library, so a duplicate is visible before adding. */
  let known = $state<Set<string>>(new Set());
  let debounce: ReturnType<typeof setTimeout> | null = null;
  /** The chosen OPML file's text, kept so applying sends what was planned. */
  let opml = $state<string | null>(null);
  let plan = $state<OpmlImport | null>(null);
  let opmlMode = $state<PolicyMode | ''>('');
  let importing = $state(false);
  let opmlError = $state<string | null>(null);

  $effect(() => {
    void status.load((signal) => providers({ signal }));
    void listPodcasts()
      .then((entries) => {
        known = new Set(
          entries.map((entry) => entry.source?.feed_url).filter((url): url is string => !!url),
        );
      })
      .catch(() => {
        // Without the library the add button simply offers no duplicate hint.
      });
  });

  // The query lives in the URL, so a search survives a refresh and can be
  // shared; typing only replaces history entries.
  $effect(() => {
    const q = (query.q ?? '').trim();
    text = query.q ?? '';
    if (q.length === 0) {
      search.data = null;
      search.error = null;
      return;
    }
    void search.load((signal) => searchProviders(q, 15, { signal }));
  });

  function onInput(value: string): void {
    text = value;
    if (debounce !== null) {
      clearTimeout(debounce);
    }
    debounce = setTimeout(() => {
      navigation.go(withQuery(navigation.current, { q: value.trim() }), { replace: true });
    }, 350);
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    if (debounce !== null) {
      clearTimeout(debounce);
    }
    navigation.go(withQuery(navigation.current, { q: text.trim() }), { replace: true });
  }

  async function resolve(input: string): Promise<void> {
    resolving = input;
    resolved = null;
    notice = null;
    try {
      resolved = await resolveFeed(input);
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      resolving = null;
    }
  }

  async function add(input: string): Promise<void> {
    adding = input;
    notice = null;
    try {
      const outcome = await addPodcast(input);
      const where = hrefFor({ name: 'podcast', id: outcome.podcast.id });
      notice = outcome.created
        ? { tone: 'ok', text: m.discover.added(outcome.podcast.title) }
        : { tone: 'ok', text: m.discover.alreadyAdded(outcome.podcast.title) };
      known = new Set([...known, outcome.source.feed_url]);
      navigation.go(where);
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      adding = null;
    }
  }

  async function planOpml(file: File | null): Promise<void> {
    opml = null;
    plan = null;
    opmlError = null;
    if (file === null) {
      return;
    }
    importing = true;
    try {
      const text = await file.text();
      plan = await importOpml(text, false, null);
      opml = text;
    } catch (error) {
      opmlError = messageFor(error);
    } finally {
      importing = false;
    }
  }

  async function applyOpml(): Promise<void> {
    if (opml === null) {
      return;
    }
    importing = true;
    opmlError = null;
    try {
      plan = await importOpml(opml, true, opmlMode === '' ? null : opmlMode);
    } catch (error) {
      opmlError = messageFor(error);
    } finally {
      importing = false;
    }
  }

  const OUTCOME: Record<OpmlAction, { label: string; tone: 'ok' | 'warn' | 'err' | 'neutral' }> = {
    add: { label: m.discover.opml.outcomes.add, tone: 'ok' },
    already_present: { label: m.discover.opml.outcomes.alreadyPresent, tone: 'neutral' },
    duplicate: { label: m.discover.opml.outcomes.duplicate, tone: 'neutral' },
    invalid: { label: m.discover.opml.outcomes.invalid, tone: 'warn' },
    needs_review: { label: m.discover.opml.outcomes.needsReview, tone: 'warn' },
    conflict: { label: m.discover.opml.outcomes.conflict, tone: 'warn' },
    failed: { label: m.discover.opml.outcomes.failed, tone: 'err' },
  };

  function feedOf(result: RankedCandidate): string | null {
    return result.candidate.feed_url ?? null;
  }

  const partial = $derived(
    (search.data?.providers ?? []).filter((p) => p.status !== 'ok' && p.status !== 'skipped'),
  );
</script>

<h1>{m.discover.title}</h1>
<p class="muted">{m.discover.intro}</p>

<form class="row" onsubmit={submit}>
  <label class="grow">
    <span class="visually-hidden">{m.discover.field}</span>
    <input
      type="search"
      placeholder={m.discover.field}
      value={text}
      oninput={(event) => onInput(event.currentTarget.value)}
    />
  </label>
  <button type="submit" class="primary">{m.discover.search}</button>
  {#if /^https?:\/\//i.test(text.trim())}
    <button type="button" disabled={resolving !== null} onclick={() => void resolve(text.trim())}>
      {resolving ? m.discover.resolving : m.discover.resolve}
    </button>
  {/if}
</form>

{#if notice}
  <Notice tone={notice.tone}>{notice.text}</Notice>
{/if}

{#if resolved}
  <section class="card resolved">
    <h2>{m.discover.resolved.title}</h2>
    <p class="row">
      <strong>{resolved.title ?? resolved.feed_url}</strong>
      {#if resolved.author}<span class="muted">{resolved.author}</span>{/if}
    </p>
    <p class="small muted">
      {m.discover.resolved.items(resolved.item_count)} · {m.discover.resolved.withMedia(resolved.items_with_media)}
      {#if resolved.newest_item}· {m.discover.resolved.newest(resolved.newest_item)}{/if}
    </p>
    <p class="small"><code>{resolved.feed_url}</code></p>
    {#each resolved.warnings as warning (warning)}
      <p class="small warn">{warning}</p>
    {/each}
    <details class="small">
      <summary>{m.discover.resolved.steps(resolved.provenance.length)}</summary>
      <ol>
        {#each resolved.provenance as step, index (index)}
          <li>
            <Badge value={step.kind} tone={step.ok ? 'ok' : 'err'} />
            {step.detail}
            <span class="muted">{m.discover.resolved.elapsed(step.elapsed_ms)}</span>
          </li>
        {/each}
      </ol>
    </details>
    <div class="row">
      {#if known.has(resolved.feed_url)}
        <p class="small muted">{m.discover.resolved.known}</p>
      {:else}
        {@const feedUrl = resolved.feed_url}
        <button
          type="button"
          class="primary"
          disabled={adding !== null}
          onclick={() => void add(feedUrl)}
        >
          {adding ? m.discover.adding : m.discover.resolved.add}
        </button>
      {/if}
      <button type="button" onclick={() => (resolved = null)}>{m.discover.resolved.dismiss}</button>
    </div>
  </section>
{/if}

{#if status.data}
  <p class="row small providers">
    {#each status.data as provider (provider.id)}
      <span title={provider.disabled_reason ?? provider.attribution ?? undefined}>
        <Badge
          value={provider.name}
          tone={provider.enabled ? (provider.health.circuit.state === 'closed' ? 'ok' : 'warn') : 'neutral'}
        />
      </span>
    {/each}
  </p>
{/if}

{#if search.loading && search.data === null}
  <StateBlock state="loading" title={m.discover.searching} />
{:else if search.state === 'error'}
  <StateBlock
    state="error"
    title={m.discover.failed}
    error={search.error}
    onretry={() => void search.load((signal) => searchProviders(text.trim(), 15, { signal }))}
  />
{:else if search.data}
  {#if partial.length > 0}
    <Notice tone="info">
      {m.discover.incomplete(
        partial.map((p) => `${p.provider} ${p.status}${p.error ? `: ${p.error}` : ''}`).join(' · '),
      )}
    </Notice>
  {/if}

  {#if search.data.outcome === 'no_providers_enabled'}
    <StateBlock
      state="empty"
      title={m.discover.noProviders}
      hint={m.discover.noProvidersHint}
    />
  {:else if search.data.outcome === 'all_providers_failed'}
    <StateBlock
      state="error"
      title={m.discover.allFailed}
      error={new Error(m.discover.allFailedDetail)}
    />
  {:else if search.data.results.length === 0}
    <StateBlock state="empty" title={m.discover.noMatch(search.data.query.raw)} />
  {:else}
    <ul class="results">
      {#each search.data.results as result (result.rank)}
        {@const feed = feedOf(result)}
        {@const c = result.candidate}
        {@const website = webLink(c.website)}
        <li class="card">
          <Artwork src={c.artwork} title={c.title} size={64} />
          <div class="meta">
            <h2>{c.title}</h2>
            <p class="muted small">
              {c.author ?? c.publisher ?? m.discover.result.unknownAuthor}
              {#if c.episode_count}· {m.discover.result.episodes(c.episode_count)}{/if}
              · {m.discover.result.foundBy(c.identities.map((i) => i.provider).join(', '))}
            </p>
            {#if c.description}
              <p class="small snippet">{c.description}</p>
            {/if}
            <p class="row small">
              <span class="muted">{m.discover.result.score(result.score.toFixed(2))}</span>
              <button
                type="button"
                class="linky"
                aria-expanded={explained === result.rank}
                onclick={() => (explained = explained === result.rank ? null : result.rank)}
              >
                {explained === result.rank ? m.discover.result.hideExplanation : m.discover.result.explain}
              </button>
            </p>
            {#if explained === result.rank}
              <table class="small">
                <caption class="visually-hidden">{m.discover.result.signalsCaption(c.title)}</caption>
                <thead>
                  <tr><th scope="col">{m.discover.result.signal}</th><th scope="col">{m.discover.result.value}</th><th scope="col">{m.discover.result.contribution}</th><th scope="col">{m.discover.result.note}</th></tr>
                </thead>
                <tbody>
                  {#each result.explanation.signals as signal (signal.name)}
                    <tr>
                      <td>{signal.name}</td>
                      <td>{signal.value.toFixed(2)}</td>
                      <td>{signal.contribution.toFixed(2)}</td>
                      <td class="muted">{signal.note}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
              {#each result.ambiguities as ambiguity (ambiguity.other_title)}
                <p class="small warn">{m.discover.result.closeTo(ambiguity.other_title, ambiguity.reason)}</p>
              {/each}
            {/if}
            <div class="row">
              {#if feed && known.has(feed)}
                <span class="small muted">{m.discover.result.inLibrary}</span>
              {:else if feed}
                <button
                  type="button"
                  class="primary"
                  disabled={adding !== null}
                  onclick={() => void add(feed)}
                >
                  {adding === feed ? m.discover.adding : m.discover.result.add}
                </button>
                <button type="button" disabled={resolving !== null} onclick={() => void resolve(feed)}>
                  {m.discover.result.inspect}
                </button>
              {:else}
                <span class="small muted">{m.discover.result.noFeed}</span>
              {/if}
              {#if website}
                <a class="small" href={website} rel="noreferrer noopener external">{m.discover.result.website}</a>
              {/if}
            </div>
          </div>
        </li>
      {/each}
    </ul>
    {#if search.data.attribution.length > 0}
      <p class="muted small attribution">{search.data.attribution.join(' · ')}</p>
    {/if}
  {/if}
{:else}
  <StateBlock
    state="empty"
    title={m.discover.start}
    hint={m.discover.startHint}
  />
{/if}

<section class="card opml" aria-labelledby="opml-heading">
  <h2 id="opml-heading">{m.discover.opml.title}</h2>
  <p class="small muted">{m.discover.opml.intro}</p>
  <label class="row">
    <span>{m.discover.opml.file}</span>
    <input
      type="file"
      accept=".opml,.xml,text/x-opml,text/xml,application/xml"
      disabled={importing}
      onchange={(event) => void planOpml(event.currentTarget.files?.[0] ?? null)}
    />
  </label>
  {#if opmlError}
    <Notice tone="err">{opmlError}</Notice>
  {/if}
  {#if plan}
    <p>
      {#if plan.applied}
        {m.discover.opml.applied(plan.counts.add, plan.items.length)}
      {:else}
        {m.discover.opml.planned(plan.counts.add, plan.items.length)}
      {/if}
    </p>
    {#if plan.items.length > 0}
      <table class="small">
        <caption class="visually-hidden">{m.discover.opml.caption}</caption>
        <thead>
          <tr><th scope="col">{m.discover.opml.feed}</th><th scope="col">{m.discover.opml.outcome}</th><th scope="col">{m.discover.opml.detail}</th></tr>
        </thead>
        <tbody>
          {#each plan.items as item, index (index)}
            <tr>
              <td>{item.title ?? item.xml_url}<br /><code>{item.xml_url}</code></td>
              <td>
                <Badge
                  value={item.action === 'add' && plan.applied ? m.discover.opml.outcomes.added : OUTCOME[item.action].label}
                  tone={OUTCOME[item.action].tone}
                />
              </td>
              <td class="muted">{item.detail ?? ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    {#if !plan.applied && plan.counts.add > 0}
      <div class="row">
        <label>
          <span>{m.discover.opml.mode}</span>
          <select bind:value={opmlMode} disabled={importing}>
            <option value="">{m.discover.opml.modeGlobal}</option>
            <option value="auto">{m.discover.opml.modeAuto}</option>
            <option value="manual">{m.discover.opml.modeManual}</option>
          </select>
        </label>
        <button type="button" class="primary" disabled={importing} onclick={() => void applyOpml()}>
          {importing ? m.discover.adding : m.discover.opml.add(plan.counts.add)}
        </button>
      </div>
    {/if}
  {/if}
  <p class="small"><Link href={hrefFor({ name: 'archiveImport' })}>{m.archiveImport.link}</Link></p>
</section>

<p class="small"><Link href="/podcasts">{m.discover.back}</Link></p>

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

  .providers {
    gap: 0.35rem;
    margin: 0.5rem 0;
  }

  .results {
    list-style: none;
    margin: var(--gap) 0;
    padding: 0;
    display: grid;
    gap: 0.75rem;
  }

  .results li,
  .resolved {
    display: flex;
    gap: 0.75rem;
    align-items: start;
  }

  .resolved {
    display: block;
    margin: var(--gap) 0;
  }

  .meta {
    min-width: 0;
    flex: 1;
  }

  h2 {
    font-size: 1.05rem;
    margin: 0 0 0.2rem;
  }

  p {
    margin: 0 0 0.35rem;
  }

  .snippet {
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .warn {
    color: var(--warn);
  }

  .linky {
    border: none;
    background: none;
    padding: 0;
    min-height: 0;
    color: var(--accent);
    text-decoration: underline;
  }

  ol {
    margin: 0.35rem 0 0;
    padding-left: 1.25rem;
  }

  .attribution {
    margin-top: var(--gap);
  }

  .opml {
    margin: var(--gap) 0;
  }

  .opml table {
    width: 100%;
  }

  .opml code {
    word-break: break-all;
  }
</style>
