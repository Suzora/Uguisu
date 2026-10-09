<!--
  One podcast: what it is, what the scheduler and the archive policy do with
  it, and its episodes.

  Every action here maps to one existing endpoint and is followed by an
  authoritative re-read, so the page never shows a state the server did not
  confirm.
-->
<script lang="ts">
  import Artwork from '../lib/components/Artwork.svelte';
  import Badge from '../lib/components/Badge.svelte';
  import EpisodeList from '../lib/components/EpisodeList.svelte';
  import Link from '../lib/components/Link.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import {
    archivePodcast,
    artworkImageUrl,
    clearPolicy,
    enqueuePodcast,
    getPodcast,
    getPolicy,
    messageFor,
    moveFeed,
    pausePodcast,
    podcastArtwork,
    refreshPodcast,
    removePodcast,
    resumePodcast,
    schedulePodcast,
    setPolicy,
  } from '../lib/api';
  import type { PodcastArtwork, PodcastDetail, PolicyBody } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { withQuery } from '../lib/router';
  import { dateTime, relative } from '../lib/format';
  import { webLink } from '../lib/links';
  import { label, m } from '../lib/i18n';

  interface Props {
    id: string;
    query: Record<string, string>;
  }

  const { id, query }: Props = $props();

  const detail = new Resource<PodcastDetail>();
  const policy = new Resource<PolicyBody>();
  const artwork = new Resource<{ current: PodcastArtwork | null }>();
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let busy = $state<string | null>(null);

  function reload(): void {
    void detail.load((signal) => getPodcast(id, { signal }));
    void policy.load((signal) => getPolicy(id, { signal }));
    void artwork.load((signal) => podcastArtwork(id, { signal }));
  }

  $effect(() => {
    void id;
    reload();
    return () => {
      detail.cancel();
      policy.cancel();
      artwork.cancel();
    };
  });

  const podcast = $derived(detail.data?.podcast ?? null);
  const website = $derived(webLink(podcast?.website));
  const source = $derived(detail.data?.source ?? null);
  const announced = $derived(detail.data?.announced ?? null);
  /** Why the announced feed failed the check, once a move was asked for. */
  let unverified = $state<string | null>(null);
  const cover = $derived(
    artworkImageUrl(id, artwork.data?.current?.hash_value) ?? podcast?.artwork_url ?? null,
  );
  const stateFilter = $derived(query.state ?? '');

  let confirmingRemove = $state(false);

  /** Removes the podcast's records and leaves for the library (ADR 0055). */
  async function remove(): Promise<void> {
    busy = 'remove';
    notice = null;
    try {
      await removePodcast(id);
      navigation.go('/podcasts');
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
      confirmingRemove = false;
    } finally {
      busy = null;
    }
  }

  async function act<T>(name: string, run: () => Promise<T>, done: string | ((result: T) => string)): Promise<void> {
    busy = name;
    notice = null;
    try {
      const result = await run();
      notice = { tone: 'ok', text: typeof done === 'string' ? done : done(result) };
      reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      busy = null;
    }
  }

  /** Moves to the announced feed; one that fails the check asks first. */
  async function move(force: boolean): Promise<void> {
    if (!announced) {
      return;
    }
    busy = 'move';
    notice = null;
    try {
      const result = await moveFeed(id, announced.feed_url, force);
      if (result.moved) {
        unverified = null;
        notice = {
          tone: 'ok',
          text: m.podcast.announced.moved(result.report ? describeRefresh(result.report) : null),
        };
        reload();
      } else if (!result.verified) {
        unverified = result.check;
      } else {
        reload();
      }
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      busy = null;
    }
  }

  function describeRefresh(report: { outcome: string; episodes?: { added: number; updated: number } }): string {
    if (report.outcome === 'not_modified') {
      return m.podcast.refresh.notModified;
    }
    if (report.outcome === 'failed') {
      return m.podcast.refresh.failed;
    }
    const added = report.episodes?.added ?? 0;
    const updated = report.episodes?.updated ?? 0;
    return m.podcast.refresh.counts(added, updated);
  }
</script>

{#if detail.state === 'error'}
  <StateBlock state="error" title={m.podcast.unreadable} error={detail.error} onretry={reload} />
{:else if podcast === null}
  <StateBlock state="loading" />
{:else}
  <p class="small"><Link href="/podcasts">{m.podcast.back}</Link></p>

  <header class="head">
    <Artwork src={cover} title={podcast.title} size={112} />
    <div class="about">
      <h1>{podcast.title}</h1>
      <p class="row small meta">
        <Badge value={podcast.status} />
        {#if podcast.author}<span>{podcast.author}</span>{/if}
        {#if podcast.language}<span class="muted">{podcast.language}</span>{/if}
        {#if podcast.explicit}<span class="muted">{m.podcast.explicit}</span>{/if}
        <span class="muted">{m.podcast.episodeCount(detail.data?.episodes_total ?? 0)}</span>
      </p>
      {#if podcast.categories.length > 0}
        <p class="muted small">{podcast.categories.join(' · ')}</p>
      {/if}
      {#if podcast.description_text}
        <details>
          <summary>{m.podcast.description}</summary>
          <p class="description">{podcast.description_text}</p>
        </details>
      {/if}
      <p class="small links">
        {#if website}
          <a href={website} rel="noreferrer noopener external">{m.podcast.website}</a>
        {/if}
        {#if source}
          <a href={source.feed_url} rel="noreferrer noopener external">{m.podcast.feed}</a>
        {/if}
      </p>
    </div>
  </header>

  {#if notice}
    <Notice tone={notice.tone}>{notice.text}</Notice>
  {/if}

  {#if announced}
    <section class="card announced">
      <h2>{m.podcast.announced.title}</h2>
      <p class="small"><code>{announced.feed_url}</code></p>
      <p class="small muted">
        {m.podcast.announced.notMoved(relative(announced.discovered_at))}
        {label(announced.fetch.last_error_kind)}: {announced.fetch.last_error_detail}
      </p>
      {#if unverified}
        <p class="small">{m.podcast.announced.unverified(unverified)}</p>
        <div class="row">
          <button type="button" class="primary" disabled={busy !== null} onclick={() => void move(true)}>
            {busy === 'move' ? m.podcast.announced.moving : m.podcast.announced.moveAnyway}
          </button>
          <button type="button" disabled={busy !== null} onclick={() => (unverified = null)}>{m.podcast.announced.cancel}</button>
        </div>
      {:else}
        <button type="button" disabled={busy !== null} onclick={() => void move(false)}>
          {busy === 'move' ? m.podcast.announced.checking : m.podcast.announced.move}
        </button>
      {/if}
    </section>
  {/if}

  <section class="card actions">
    <h2>{m.podcast.actions.title}</h2>
    <div class="row">
      {#if podcast.status !== 'archived'}
        <button
          type="button"
          class="primary"
          disabled={busy !== null}
          onclick={() => act('refresh', () => refreshPodcast(id), describeRefresh)}
        >
          {busy === 'refresh' ? m.podcast.actions.refreshing : m.podcast.actions.refresh}
        </button>
      {/if}
      {#if podcast.status === 'paused' || podcast.status === 'archived'}
        <button
          type="button"
          disabled={busy !== null}
          onclick={() => act('resume', () => resumePodcast(id), m.podcast.actions.resumed)}
        >
          {m.podcast.actions.resume}
        </button>
      {:else if podcast.status === 'active'}
        <button
          type="button"
          disabled={busy !== null}
          onclick={() => act('pause', () => pausePodcast(id), m.podcast.actions.paused)}
        >
          {m.podcast.actions.pause}
        </button>
      {/if}
      {#if podcast.status !== 'archived'}
        <button
          type="button"
          disabled={busy !== null}
          onclick={() => act('schedule', () => schedulePodcast(id, null), m.podcast.actions.scheduled)}
        >
          {m.podcast.actions.schedule}
        </button>
      {/if}
      <button
        type="button"
        disabled={busy !== null}
        onclick={() =>
          act('download', () => enqueuePodcast(id), (summary) =>
            m.podcast.actions.enqueued(summary.created, summary.existing, summary.completed, summary.skipped.length),
          )}
      >
        {m.podcast.actions.downloadAll}
      </button>
      {#if podcast.status !== 'archived'}
        <button
          type="button"
          disabled={busy !== null}
          onclick={() => act('archive', () => archivePodcast(id), m.podcast.actions.archived)}
        >
          {m.podcast.actions.archive}
        </button>
      {/if}
      {#if !confirmingRemove}
        <button type="button" disabled={busy !== null} onclick={() => (confirmingRemove = true)}>
          {m.podcast.actions.remove}
        </button>
      {/if}
    </div>
    {#if confirmingRemove}
      <p class="small">
        {m.podcast.actions.confirmRemove(podcast.title, detail.data?.episodes_total ?? 0)}
      </p>
      <div class="row">
        <button type="button" class="danger" disabled={busy !== null} onclick={() => void remove()}>
          {busy === 'remove' ? m.podcast.actions.removing : m.podcast.actions.remove}
        </button>
        <button type="button" disabled={busy !== null} onclick={() => (confirmingRemove = false)}>
          {m.podcast.actions.cancel}
        </button>
      </div>
    {/if}
  </section>

  <div class="grid">
    <section class="card">
      <h2>{m.podcast.schedule.title}</h2>
      <dl>
        <div><dt>{m.podcast.schedule.lastRefresh}</dt><dd title={dateTime(podcast.last_refresh_at)}>{relative(podcast.last_refresh_at)}</dd></div>
        <div><dt>{m.podcast.schedule.nextRefresh}</dt><dd title={dateTime(podcast.next_refresh_at)}>{relative(podcast.next_refresh_at)}</dd></div>
        <div>
          <dt>{m.podcast.schedule.interval}</dt>
          <dd>
            {podcast.refresh_interval_secs
              ? m.podcast.schedule.minutes(Math.round(podcast.refresh_interval_secs / 60))
              : m.podcast.schedule.defaultInterval}
          </dd>
        </div>
        {#if source}
          <div><dt>{m.podcast.schedule.feedState}</dt><dd><Badge value={source.fetch.state} /></dd></div>
          {#if source.fetch.consecutive_failures > 0}
            <div><dt>{m.podcast.schedule.failures}</dt><dd class="err">{source.fetch.consecutive_failures}</dd></div>
          {/if}
          {#if source.fetch.last_error_detail}
            <div><dt>{m.podcast.schedule.lastError}</dt><dd class="err">{label(source.fetch.last_error_kind)}: {source.fetch.last_error_detail}</dd></div>
          {/if}
        {/if}
      </dl>
      {#if podcast.last_error}
        <p class="small err">{podcast.last_error}</p>
      {/if}
    </section>

    <section class="card">
      <h2>{m.podcast.policy.title}</h2>
      {#if policy.state === 'error'}
        <StateBlock state="error" title={m.podcast.policy.unreadable} error={policy.error} />
      {:else if policy.data === null}
        <StateBlock state="loading" />
      {:else}
        {@const effective = policy.data.effective}
        <dl>
          <div><dt>{m.podcast.policy.mode}</dt><dd><Badge value={effective.mode} tone={effective.mode === 'auto' ? 'ok' : 'neutral'} /></dd></div>
          <div><dt>{m.podcast.policy.backlog}</dt><dd>{m.podcast.policy.backlogEpisodes(effective.max_backlog)}</dd></div>
          <div><dt>{m.podcast.policy.maxAge}</dt><dd>{m.podcast.policy.maxAgeDays(effective.max_age_days)}</dd></div>
          <div><dt>{m.podcast.policy.priority}</dt><dd>{effective.priority}</dd></div>
          <div>
            <dt>{m.podcast.policy.source}</dt>
            <dd>{policy.data.stored ? m.podcast.policy.stored : m.podcast.policy.global}</dd>
          </div>
        </dl>
        <div class="row">
          <button
            type="button"
            disabled={busy !== null}
            onclick={() =>
              act(
                'policy',
                () => setPolicy(id, { mode: effective.mode === 'auto' ? 'manual' : 'auto' }),
                effective.mode === 'auto'
                  ? m.podcast.policy.manualNotice
                  : m.podcast.policy.autoNotice,
              )}
          >
            {effective.mode === 'auto' ? m.podcast.policy.stopAuto : m.podcast.policy.startAuto}
          </button>
          {#if policy.data.stored}
            <button
              type="button"
              disabled={busy !== null}
              onclick={() => act('policy-clear', () => clearPolicy(id), m.podcast.policy.cleared)}
            >
              {m.podcast.policy.clear}
            </button>
          {/if}
        </div>
      {/if}
    </section>
  </div>

  <section class="episodes">
    <div class="row head-row">
      <h2>{m.podcast.episodes.title}</h2>
      <label>
        <span class="visually-hidden">{m.podcast.episodes.filter}</span>
        <select
          value={stateFilter}
          onchange={(event) =>
            navigation.go(withQuery(navigation.current, { state: event.currentTarget.value }), {
              replace: true,
            })}
        >
          <option value="">{m.podcast.episodes.states.any}</option>
          <option value="archived">{m.podcast.episodes.states.archived}</option>
          <option value="expected">{m.podcast.episodes.states.expected}</option>
          <option value="queued">{m.podcast.episodes.states.queued}</option>
          <option value="downloading">{m.podcast.episodes.states.downloading}</option>
          <option value="missing">{m.podcast.episodes.states.missing}</option>
          <option value="failed">{m.podcast.episodes.states.failed}</option>
          <option value="skipped">{m.podcast.episodes.states.skipped}</option>
        </select>
      </label>
    </div>
    <EpisodeList podcastId={id} podcastTitle={podcast.title} {stateFilter} />
  </section>
{/if}

<style>
  .head {
    display: flex;
    gap: var(--gap);
    align-items: start;
    margin-bottom: var(--gap);
  }

  .about {
    min-width: 0;
  }

  h1 {
    margin-bottom: 0.25rem;
  }

  p {
    margin: 0 0 0.35rem;
  }

  .meta {
    gap: 0.5rem;
  }

  .description {
    white-space: pre-wrap;
    max-height: 16rem;
    overflow-y: auto;
    margin-top: 0.35rem;
  }

  .links {
    display: flex;
    gap: 0.75rem;
  }

  .actions,
  .announced {
    margin-bottom: var(--gap);
  }

  .announced code {
    overflow-wrap: anywhere;
  }

  .grid {
    margin-bottom: var(--gap);
    grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr));
  }

  dl {
    display: grid;
    gap: 0.25rem;
    margin: 0 0 0.75rem;
  }

  dt {
    color: var(--muted);
  }

  dt,
  dd {
    display: inline;
    margin: 0;
  }

  dt::after {
    content: ': ';
  }

  .err {
    color: var(--err);
  }

  .head-row {
    justify-content: space-between;
  }

  @media (width <= 34rem) {
    .head {
      flex-direction: column;
    }
  }
</style>
