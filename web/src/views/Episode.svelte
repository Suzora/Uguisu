<!--
  One episode: what the feed says about it, its archive record and its
  download job — one request, because the API composes the three.
-->
<script lang="ts">
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { events } from '../lib/events.svelte';
  import { getEpisode } from '../lib/api';
  import type { EpisodeDetail } from '../lib/api';
  import { player } from '../lib/player.svelte';
  import { hrefFor } from '../lib/router';
  import { bytes, date, dateTime, duration, relative } from '../lib/format';
  import { webLink } from '../lib/links';
  import { label, m } from '../lib/i18n';

  interface Props {
    id: string;
  }

  const { id }: Props = $props();

  const detail = new Resource<EpisodeDetail>();

  function reload(): void {
    void detail.load((signal) => getEpisode(id, { signal }));
  }

  $effect(() => {
    void id;
    reload();
    return () => detail.cancel();
  });

  $effect(() =>
    events.subscribe((event) => {
      if (event.episode_id === id && event.kind !== 'download.progress') {
        reload();
      }
    }),
  );
  $effect(() => events.onResync(reload));

  const episode = $derived(detail.data?.episode ?? null);
  const page = $derived(webLink(episode?.link));
  const podcastTitle = $derived(detail.data?.podcast_title ?? '');
  const archive = $derived(detail.data?.archive ?? null);
  const job = $derived(detail.data?.job ?? null);
</script>

{#if detail.state === 'error'}
  <StateBlock state="error" title={m.episode.unreadable} error={detail.error} onretry={reload} />
{:else if episode === null}
  <StateBlock state="loading" />
{:else}
  <p class="small">
    <Link href={hrefFor({ name: 'podcast', id: episode.podcast_id })}>{m.episode.back(podcastTitle)}</Link>
  </p>

  <h1>{episode.title}</h1>
  <p class="row small meta">
    <Badge value={episode.archive_state} title={episode.skip_reason ?? undefined} />
    <span title={dateTime(episode.published_at)}>{date(episode.published_at)}</span>
    {#if episode.duration_secs}<span class="muted">{duration(episode.duration_secs)}</span>{/if}
    {#if episode.season}<span class="muted">{m.episodes.season(episode.season)}</span>{/if}
    {#if episode.episode_number}<span class="muted">{m.episodes.number(episode.episode_number)}</span>{/if}
    {#if episode.explicit}<span class="muted">{m.episodes.explicit}</span>{/if}
  </p>
  {#if episode.archive_state === 'archived' && archive}
    <p>
      <button type="button" class="primary" onclick={() => episode && player.play(episode, podcastTitle)}>
        {m.episode.play}
      </button>
    </p>
  {/if}

  <section class="card">
    <h2>{m.episode.notes.title}</h2>
    {#if episode.description_text}
      <p class="notes">{episode.description_text}</p>
    {:else}
      <p class="muted">{m.episode.notes.none}</p>
    {/if}
    {#if page}
      <p class="small">
        <a href={page} rel="noreferrer noopener external">{m.episode.notes.page}</a>
      </p>
    {/if}
  </section>

  <div class="grid">
    <section class="card">
      <h2>{m.episode.dates.title}</h2>
      <dl>
        <div><dt>{m.episode.dates.published}</dt><dd>{dateTime(episode.published_at)}</dd></div>
        <div><dt>{m.episode.dates.firstSeen}</dt><dd>{dateTime(episode.first_seen_at)}</dd></div>
        {#if episode.removed_from_feed_at}
          <div><dt>{m.episode.dates.removed}</dt><dd>{dateTime(episode.removed_from_feed_at)}</dd></div>
        {/if}
      </dl>
    </section>

    <section class="card">
      <h2>{m.episode.archive.title}</h2>
      {#if archive}
        <dl>
          <div><dt>{m.episode.archive.file}</dt><dd><code>{archive.relative_path}</code></dd></div>
          <div><dt>{m.episode.archive.size}</dt><dd>{bytes(archive.size_bytes)}</dd></div>
          <div>
            <dt>{m.episode.archive.state}</dt>
            <dd>
              <Badge value={archive.verification_state} />
              {#if archive.verification_reason}<span class="small muted">{label(archive.verification_reason)}</span>{/if}
            </dd>
          </div>
          <div>
            <dt>{m.episode.archive.checked}</dt>
            <dd title={dateTime(archive.verified_at)}>{relative(archive.verified_at)}</dd>
          </div>
          <div><dt>{m.episode.archive.tags}</dt><dd><Badge value={archive.tag_state} /></dd></div>
        </dl>
        {#if archive.verification_state === 'missing'}
          <p><Link href={hrefFor({ name: 'archiveRepair' }, { podcast: archive.podcast_id })}>{m.archiveRepair.repairOne}</Link></p>
        {/if}
      {:else}
        <p class="muted">{m.episode.archive.none}</p>
      {/if}
    </section>

    <section class="card">
      <h2>{m.episode.job.title}</h2>
      {#if job}
        <dl>
          <div>
            <dt>{m.episode.job.state}</dt>
            <dd>
              <Badge value={job.state} />
              {#if job.state_reason}<span class="small muted">{label(job.state_reason)}</span>{/if}
            </dd>
          </div>
          <div>
            <dt>{m.episode.job.attempts}</dt>
            <dd>{m.episode.job.attemptsOf(job.attempt_count, job.max_attempts)}</dd>
          </div>
          {#if job.last_error_detail}
            <div>
              <dt>{m.episode.job.lastError}</dt>
              <dd class="err">{label(job.last_error_kind)}: {job.last_error_detail}</dd>
            </div>
          {/if}
          <div>
            <dt>{m.episode.job.updated}</dt>
            <dd title={dateTime(job.updated_at)}>{relative(job.updated_at)}</dd>
          </div>
        </dl>
      {:else}
        <p class="muted">{m.episode.job.none}</p>
      {/if}
    </section>
  </div>
{/if}

<style>
  h1 {
    margin-bottom: 0.25rem;
  }

  .meta {
    gap: 0.5rem;
    margin: 0 0 var(--gap);
  }

  .notes {
    white-space: pre-wrap;
  }

  .grid {
    margin-top: var(--gap);
  }

  dl {
    display: grid;
    gap: 0.25rem;
    margin: 0;
  }

  dt {
    color: var(--muted);
    font-weight: 600;
  }

  dd,
  dt {
    display: inline;
    margin: 0;
  }

  dt::after {
    content: ': ';
  }

  .err {
    color: var(--err);
  }
</style>
