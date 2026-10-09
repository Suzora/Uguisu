<!--
  The download queue.

  Speed and ETA are not on the row at all — they arrive as
  `download.progress` events, joined here from the one shared event stream.
  The server stays authoritative; every event is an overlay on a row the API
  supplied, and a reconnect re-reads the list.
-->
<script lang="ts">
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import Pager from '../lib/components/Pager.svelte';
  import Progress from '../lib/components/Progress.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { events } from '../lib/events.svelte';
  import {
    commandDownload,
    downloadStats,
    getPodcast,
    listDownloads,
    messageFor,
    pauseAllDownloads,
    resumeAllDownloads,
    retryFailedDownloads,
  } from '../lib/api';
  import type { DownloadJob, DownloadState, DownloadStats, JobPage } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { hrefFor, withQuery } from '../lib/router';
  import { dateTime, eta, rate, relative, transferred } from '../lib/format';
  import { label, m } from '../lib/i18n';

  interface Props {
    query: Record<string, string>;
  }

  const { query }: Props = $props();

  const PAGE = 50;
  const STATES: DownloadState[] = [
    'queued',
    'downloading',
    'finalizing',
    'retrying',
    'paused',
    'completed',
    'failed',
    'cancelled',
  ];

  const stats = new Resource<DownloadStats>();
  const first = new Resource<JobPage>();
  let jobs = $state<DownloadJob[]>([]);
  let cursor = $state<string | null>(null);
  let loadingMore = $state(false);
  let podcastTitle = $state<string | null>(null);
  /** Job id → the newest `download.progress` payload for it. */
  let live = $state<Record<string, { bytes: number; total: number | null; speed: number; eta: number | null }>>({});
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let busy = $state<string | null>(null);

  const selected = $derived((query.state ?? '') as DownloadState | '');
  const podcast = $derived(query.podcast ?? '');

  async function reload(): Promise<void> {
    void stats.load((signal) => downloadStats({ signal }));
    const page = await first.load((signal) =>
      listDownloads({ state: selected, podcast, limit: PAGE }, { signal }),
    );
    if (page) {
      jobs = page.jobs;
      cursor = page.next_after ?? null;
    }
  }

  async function loadMore(): Promise<void> {
    if (!cursor || loadingMore) {
      return;
    }
    loadingMore = true;
    try {
      const page = await listDownloads({ state: selected, podcast, after: cursor, limit: PAGE });
      jobs = [...jobs, ...page.jobs];
      cursor = page.next_after ?? null;
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      loadingMore = false;
    }
  }

  $effect(() => {
    void selected;
    void podcast;
    void reload();
    return () => {
      first.cancel();
      stats.cancel();
    };
  });

  // The filter's label; a filtered queue may be empty, so not from a row.
  $effect(() => {
    podcastTitle = null;
    if (!podcast) {
      return;
    }
    const controller = new AbortController();
    void getPodcast(podcast, { signal: controller.signal })
      .then((detail) => {
        podcastTitle = detail.podcast.title;
      })
      .catch(() => {
        // The id is still shown; only the friendly name is missing.
      });
    return () => controller.abort();
  });

  $effect(() =>
    events.subscribe((event) => {
      if (event.kind === 'download.progress') {
        const p = event as unknown as {
          job_id: string;
          bytes_downloaded: number;
          total_bytes: number | null;
          speed_bps: number;
          eta_secs: number | null;
        };
        live = {
          ...live,
          [p.job_id]: {
            bytes: p.bytes_downloaded,
            total: p.total_bytes,
            speed: p.speed_bps,
            eta: p.eta_secs,
          },
        };
        return;
      }
      if (event.kind.startsWith('download.')) {
        void reload();
      }
    }),
  );
  // A reconnect means events were missed, and progress is never replayed.
  $effect(() =>
    events.onResync(() => {
      live = {};
      void reload();
    }),
  );

  function setFilter(patch: Record<string, string | undefined>): void {
    navigation.go(withQuery(navigation.current, patch), { replace: true });
  }

  async function act<T>(key: string, run: () => Promise<T>, done: string | ((result: T) => string)): Promise<void> {
    busy = key;
    notice = null;
    try {
      const result = await run();
      notice = { tone: 'ok', text: typeof done === 'string' ? done : done(result) };
      await reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      busy = null;
    }
  }

  function count(s: DownloadState): number {
    return stats.data?.by_state?.[s] ?? 0;
  }

  function progressOf(job: DownloadJob) {
    const feed = live[job.id];
    return {
      done: feed?.bytes ?? job.bytes_downloaded,
      total: feed?.total ?? job.total_bytes,
      speed: feed?.speed ?? null,
      remaining: feed?.eta ?? null,
    };
  }
</script>

<h1>{m.downloads.title}</h1>

{#if stats.data}
  <p class="row small counters">
    {#each STATES as s (s)}
      <button
        type="button"
        class="counter"
        class:selected={selected === s}
        onclick={() => setFilter({ state: selected === s ? undefined : s })}
      >
        <Badge value={s} />
        <span>{count(s)}</span>
      </button>
    {/each}
    {#if selected}
      <button type="button" onclick={() => setFilter({ state: undefined })}>{m.downloads.everyState}</button>
    {/if}
  </p>
  {#if !stats.data.workers_started}
    <Notice tone="info">
      {m.downloads.noWorkers.before}<code>{m.downloads.noWorkers.command}</code
      >{m.downloads.noWorkers.after}
    </Notice>
  {/if}
  {#if stats.data.paused_all}
    <Notice tone="info">
      {m.downloads.queuePaused(label(stats.data.paused_all))}
    </Notice>
  {/if}
{/if}

<div class="row actions">
  <button
    type="button"
    disabled={busy !== null}
    onclick={() => act('pause-all', pauseAllDownloads, m.downloads.pausedAll)}
  >
    {m.downloads.pauseAll}
  </button>
  <button
    type="button"
    disabled={busy !== null}
    onclick={() => act('resume-all', resumeAllDownloads, m.downloads.resumedAll)}
  >
    {m.downloads.resumeAll}
  </button>
  <button
    type="button"
    disabled={busy !== null || count('failed') === 0}
    onclick={() => act('retry-failed', retryFailedDownloads, ({ requeued }) => m.downloads.retriedFailed(requeued))}
  >
    {m.downloads.retryFailed}
  </button>
  {#if podcast}
    <span class="small muted">
      {m.downloads.onlyPodcast(podcastTitle ?? podcast)}
      <button type="button" class="linky" onclick={() => setFilter({ podcast: undefined })}>
        {m.downloads.everyPodcast}
      </button>
    </span>
  {/if}
</div>

{#if notice}
  <Notice tone={notice.tone}>{notice.text}</Notice>
{/if}

{#if first.state === 'error'}
  <StateBlock state="error" title={m.downloads.unreadable} error={first.error} onretry={() => void reload()} />
{:else if first.data === null}
  <StateBlock state="loading" />
{:else if jobs.length === 0}
  <StateBlock
    state="empty"
    title={selected ? m.downloads.noneInState(selected) : m.downloads.emptyTitle}
    hint={selected ? undefined : m.downloads.emptyHint}
  />
{:else}
  <div class="scroller">
    <table>
      <caption class="visually-hidden">{m.downloads.caption}</caption>
      <thead>
        <tr>
          <th scope="col">{m.downloads.columns.episode}</th>
          <th scope="col">{m.downloads.columns.state}</th>
          <th scope="col">{m.downloads.columns.progress}</th>
          <th scope="col">{m.downloads.columns.attempts}</th>
          <th scope="col">{m.downloads.columns.updated}</th>
          <th scope="col"><span class="visually-hidden">{m.downloads.columns.actions}</span></th>
        </tr>
      </thead>
      <tbody>
        {#each jobs as job (job.id)}
          {@const p = progressOf(job)}
          <tr>
            <td data-label={m.downloads.columns.episode}>
              <Link href={hrefFor({ name: 'podcast', id: job.podcast_id })}>{job.podcast_title}</Link>
              <br />
              <!-- A title of markup only was stored empty until its feed is parsed again; the id still names the row. -->
              <Link href={hrefFor({ name: 'episode', id: job.episode_id })}>{job.episode_title || job.episode_id}</Link>
            </td>
            <td data-label={m.downloads.columns.state}>
              <Badge value={job.state} title={job.state_reason ?? undefined} />
              {#if job.last_error_detail}
                <p class="small err">{label(job.last_error_kind)}: {job.last_error_detail}</p>
              {:else if job.state_reason}
                <p class="small muted">{label(job.state_reason)}</p>
              {/if}
              {#if job.next_attempt_at}
                <p class="small muted">{m.downloads.nextTry(relative(job.next_attempt_at))}</p>
              {/if}
            </td>
            <td class="progress" data-label={m.downloads.columns.progress}>
              <Progress done={p.done} total={p.total} label={m.downloads.progressOf(job.episode_title || job.episode_id)} />
              <p class="small muted">{transferred(p.done, p.total)}</p>
              {#if p.speed !== null}
                <p class="small muted">{rate(p.speed)} · {eta(p.remaining)}</p>
              {/if}
            </td>
            <td data-label={m.downloads.columns.attempts}>{job.attempt_count} / {job.max_attempts}</td>
            <td data-label={m.downloads.columns.updated} title={dateTime(job.updated_at)}>{relative(job.updated_at)}</td>
            <td class="row" data-label={m.downloads.columns.actions}>
              {#if job.state === 'queued' || job.state === 'downloading' || job.state === 'retrying'}
                <button
                  type="button"
                  disabled={busy !== null}
                  onclick={() => act(job.id, () => commandDownload(job.id, 'pause'), m.downloads.paused)}
                >
                  {m.downloads.pause}
                </button>
                <button
                  type="button"
                  class="danger"
                  disabled={busy !== null}
                  onclick={() => act(job.id, () => commandDownload(job.id, 'cancel'), m.downloads.cancelled)}
                >
                  {m.downloads.cancel}
                </button>
              {:else if job.state === 'paused'}
                <button
                  type="button"
                  disabled={busy !== null}
                  onclick={() => act(job.id, () => commandDownload(job.id, 'resume'), m.downloads.resumed)}
                >
                  {m.downloads.resume}
                </button>
              {:else if job.state === 'failed' || job.state === 'cancelled'}
                <button
                  type="button"
                  disabled={busy !== null}
                  onclick={() => act(job.id, () => commandDownload(job.id, 'retry'), m.downloads.retried)}
                >
                  {m.downloads.retry}
                </button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <Pager shown={jobs.length} more={cursor !== null} busy={loadingMore} onmore={() => void loadMore()} />
{/if}

<style>
  .counters {
    gap: 0.35rem;
    margin-bottom: 0.5rem;
  }

  .counter {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.3rem 0.6rem;
  }

  .counter.selected {
    border-color: var(--accent);
    background: var(--bg);
  }

  .actions {
    margin-bottom: 0.75rem;
  }

  .scroller {
    overflow-x: auto;
  }

  table {
    min-width: 46rem;
  }

  .progress {
    min-width: 12rem;
  }

  p {
    margin: 0.2rem 0 0;
  }

  .err {
    color: var(--err);
  }

  .linky {
    border: none;
    background: none;
    padding: 0;
    min-height: 0;
    color: var(--accent);
    text-decoration: underline;
  }

  /* Below this width a row is taller than the viewport is wide, so each row
     becomes a card and the column headers move into the cells. */
  @media (width <= 48rem) {
    .scroller {
      overflow-x: visible;
    }

    table {
      min-width: 0;
    }

    thead {
      display: none;
    }

    tr {
      display: block;
      margin-bottom: 0.6rem;
      padding: 0.5rem 0.6rem;
      border: 1px solid var(--border);
      border-radius: var(--radius);
    }

    td {
      display: flex;
      gap: 0.5rem;
      padding: 0.2rem 0;
      border: none;
    }

    td::before {
      content: attr(data-label);
      flex: 0 0 6.5rem;
      color: var(--muted);
      font-size: 0.85rem;
    }

    td:empty {
      display: none;
    }
  }
</style>
