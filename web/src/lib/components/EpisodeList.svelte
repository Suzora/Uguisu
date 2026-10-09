<!--
  A podcast's episodes, paged with the API's keyset cursor.

  An episode row shows the state the backend recorded, plus the download job
  for that episode when one exists — the API keeps those in two places, so
  the join happens here and nowhere else.

  A candidate duplicate (ADR 0051) offers its two resolutions instead of a
  download, which the queue would refuse. A merge cannot be undone, so it
  asks once more.
-->
<script lang="ts">
  import Badge from './Badge.svelte';
  import Link from './Link.svelte';
  import Notice from './Notice.svelte';
  import Pager from './Pager.svelte';
  import Progress from './Progress.svelte';
  import StateBlock from './StateBlock.svelte';
  import { Resource } from '../resource.svelte';
  import { events } from '../events.svelte';
  import {
    commandDownload,
    enqueueEpisode,
    listDownloads,
    listEpisodes,
    messageFor,
    resolveDuplicate,
  } from '../api';
  import type { DownloadJob, DuplicateResolution, Episode, EpisodePage } from '../api';
  import { player } from '../player.svelte';
  import { hrefFor } from '../router';
  import { bytes, date, duration, transferred } from '../format';
  import { label, m } from '../i18n';
  import { webLink } from '../links';

  interface Props {
    podcastId: string;
    podcastTitle: string;
    /** Shows only episodes in these archive states when non-empty. */
    stateFilter: string;
  }

  const { podcastId, podcastTitle, stateFilter }: Props = $props();

  const PAGE = 50;
  const first = new Resource<EpisodePage>();
  let episodes = $state<Episode[]>([]);
  let cursor = $state<string | null>(null);
  let loadingMore = $state(false);
  let jobs = $state<Record<string, DownloadJob>>({});
  let expanded = $state<string | null>(null);
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let acting = $state<string | null>(null);
  let confirming = $state<string | null>(null);

  async function reload(): Promise<void> {
    const page = await first.load((signal) =>
      listEpisodes(podcastId, { limit: PAGE }, { signal }),
    );
    if (page) {
      episodes = page.episodes;
      cursor = page.next_after ?? null;
    }
    await loadJobs();
  }

  async function loadMore(): Promise<void> {
    if (!cursor || loadingMore) {
      return;
    }
    loadingMore = true;
    try {
      const page = await listEpisodes(podcastId, { after: cursor, limit: PAGE });
      episodes = [...episodes, ...page.episodes];
      cursor = page.next_after ?? null;
      await loadJobs();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      loadingMore = false;
    }
  }

  /** The queue for this podcast, keyed by episode, newest job winning. */
  async function loadJobs(): Promise<void> {
    try {
      const page = await listDownloads({ podcast: podcastId, limit: 500 });
      const next: Record<string, DownloadJob> = {};
      for (const job of page.jobs) {
        if (!next[job.episode_id]) {
          next[job.episode_id] = job;
        }
      }
      jobs = next;
    } catch {
      // The episode list is still useful without the queue overlay.
    }
  }

  $effect(() => {
    void podcastId;
    void reload();
    return () => first.cancel();
  });

  $effect(() =>
    events.subscribe((event) => {
      if (event.podcast_id !== podcastId) {
        return;
      }
      if (event.kind === 'download.progress') {
        const progress = event as unknown as { job_id: string; bytes_downloaded: number };
        for (const [episodeId, job] of Object.entries(jobs)) {
          if (job.id === progress.job_id) {
            jobs = {
              ...jobs,
              [episodeId]: { ...job, bytes_downloaded: progress.bytes_downloaded },
            };
            return;
          }
        }
        return;
      }
      if (event.kind.startsWith('download.') || event.kind.startsWith('archive.')) {
        void loadJobs();
      }
      if (event.kind.startsWith('episode.')) {
        void reload();
      }
    }),
  );
  $effect(() => events.onResync(() => void reload()));

  const visible = $derived(
    stateFilter ? episodes.filter((e) => e.archive_state === stateFilter) : episodes,
  );

  function primary(episode: Episode) {
    return episode.enclosures.find((e) => e.is_primary) ?? episode.enclosures[0] ?? null;
  }

  /** The episode a candidate duplicates, by title when it is loaded. */
  function originalTitle(candidate: Episode): string {
    const original = episodes.find((e) => e.id === candidate.duplicate_of_episode_id);
    return original ? m.episodes.quoted(original.title) : m.episodes.earlierEpisode;
  }

  async function resolve(candidate: Episode, resolution: DuplicateResolution): Promise<void> {
    confirming = null;
    acting = candidate.id;
    notice = null;
    try {
      const done = await resolveDuplicate(candidate.id, resolution);
      notice = {
        tone: 'ok',
        text:
          resolution === 'same'
            ? m.episodes.merged
            : done.queued
              ? m.episodes.separatedQueued
              : m.episodes.separated,
      };
      await reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      acting = null;
    }
  }

  async function act(id: string, run: () => Promise<unknown>, done: string): Promise<void> {
    acting = id;
    notice = null;
    try {
      await run();
      notice = { tone: 'ok', text: done };
      await loadJobs();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      acting = null;
    }
  }
</script>

{#if notice}
  <Notice tone={notice.tone}>{notice.text}</Notice>
{/if}

{#if first.state === 'error'}
  <StateBlock state="error" title={m.episodes.unreadable} error={first.error} onretry={() => void reload()} />
{:else if first.data === null}
  <StateBlock state="loading" />
{:else if episodes.length === 0}
  <StateBlock state="empty" title={m.episodes.none} hint={m.episodes.noneHint} />
{:else if visible.length === 0}
  <StateBlock state="empty" title={m.episodes.noneInState} />
{:else}
  <ul class="episodes">
    {#each visible as episode (episode.id)}
      {@const job = jobs[episode.id]}
      {@const file = primary(episode)}
      <li>
        <div class="line">
          <div class="title">
            <p class="name"><Link href={hrefFor({ name: 'episode', id: episode.id })}>{episode.title}</Link></p>
            <p class="muted small">
              {date(episode.published_at)}
              {#if episode.duration_secs}· {duration(episode.duration_secs)}{/if}
              {#if episode.season}· {m.episodes.season(episode.season)}{/if}
              {#if episode.episode_number}· {m.episodes.number(episode.episode_number)}{/if}
              {#if file?.length_bytes}· {bytes(file.length_bytes)}{/if}
              {#if episode.explicit}· {m.episodes.explicit}{/if}
            </p>
            {#if episode.duplicate_of_episode_id}
              <p class="small duplicate">
                {m.episodes.possiblySame(originalTitle(episode))}
                {#if episode.duplicate_reasons.length}({episode.duplicate_reasons
                    .map(label)
                    .join(', ')}){/if}
              </p>
            {/if}
          </div>
          <div class="state">
            <Badge value={episode.archive_state} title={episode.skip_reason ?? undefined} />
            {#if job && (job.state === 'downloading' || job.state === 'finalizing')}
              <span class="bar">
                <Progress
                  done={job.bytes_downloaded}
                  total={job.total_bytes}
                  label={m.episodes.progress}
                />
              </span>
              <span class="muted small">{transferred(job.bytes_downloaded, job.total_bytes)}</span>
            {:else if job && job.state !== 'completed'}
              <Badge value={job.state} title={job.state_reason ?? undefined} />
            {/if}
          </div>
          <div class="actions row">
            {#if episode.archive_state === 'archived'}
              <button type="button" onclick={() => player.play(episode, podcastTitle)}>{m.episodes.play}</button>
            {/if}
            {#if job && (job.state === 'queued' || job.state === 'downloading' || job.state === 'retrying')}
              <button
                type="button"
                disabled={acting === episode.id}
                onclick={() => act(episode.id, () => commandDownload(job.id, 'cancel'), m.episodes.cancelled)}
              >
                {m.episodes.cancel}
              </button>
            {:else if job && job.state === 'failed'}
              <button
                type="button"
                disabled={acting === episode.id}
                onclick={() => act(episode.id, () => commandDownload(job.id, 'retry'), m.episodes.retried)}
              >
                {m.episodes.retry}
              </button>
            {:else if episode.duplicate_of_episode_id && confirming === episode.id}
              <span class="small">{m.episodes.confirmMerge(originalTitle(episode))}</span>
              <button
                type="button"
                disabled={acting === episode.id}
                onclick={() => void resolve(episode, 'same')}
              >
                {m.episodes.merge}
              </button>
              <button type="button" onclick={() => (confirming = null)}>{m.episodes.cancel}</button>
            {:else if episode.duplicate_of_episode_id}
              <button
                type="button"
                disabled={acting === episode.id}
                onclick={() => (confirming = episode.id)}
              >
                {m.episodes.same}
              </button>
              <button
                type="button"
                disabled={acting === episode.id}
                onclick={() => void resolve(episode, 'separate')}
              >
                {m.episodes.separate}
              </button>
            {:else if episode.archive_state !== 'archived' && file}
              <button
                type="button"
                disabled={acting === episode.id}
                onclick={() => act(episode.id, () => enqueueEpisode(episode.id), m.episodes.queued)}
              >
                {m.episodes.download}
              </button>
            {/if}
            <button
              type="button"
              aria-expanded={expanded === episode.id}
              onclick={() => (expanded = expanded === episode.id ? null : episode.id)}
            >
              {expanded === episode.id ? m.episodes.less : m.episodes.more}
            </button>
          </div>
        </div>
        {#if expanded === episode.id}
          {@const page = webLink(episode.link)}
          <div class="detail small">
            {#if episode.description_text}
              <p class="notes">{episode.description_text}</p>
            {:else}
              <p class="muted">{m.episodes.noNotes}</p>
            {/if}
            <dl>
              {#if job}
                <div><dt>{m.episodes.job}</dt><dd>{job.state}{job.state_reason ? ` (${job.state_reason})` : ''}</dd></div>
                {#if job.last_error_detail}
                  <div><dt>{m.episodes.lastError}</dt><dd class="err">{job.last_error_kind}: {job.last_error_detail}</dd></div>
                {/if}
                <div><dt>{m.episodes.attemptsLabel}</dt><dd>{m.episodes.attempts(job.attempt_count, job.max_attempts)}</dd></div>
              {/if}
              {#if episode.skip_reason}
                <div><dt>{m.episodes.skipped}</dt><dd>{episode.skip_reason}</dd></div>
              {/if}
              {#if page}
                <div><dt>{m.episodes.page}</dt><dd><a href={page} rel="noreferrer noopener external">{page}</a></dd></div>
              {/if}
            </dl>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
  <Pager
    shown={visible.length}
    more={cursor !== null}
    busy={loadingMore}
    onmore={() => void loadMore()}
  />
{/if}

<style>
  .episodes {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .episodes li {
    border-bottom: 1px solid var(--border);
    padding: 0.6rem 0;
  }

  .line {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 0.75rem;
    align-items: center;
  }

  .title {
    flex: 1 1 18rem;
    min-width: 0;
  }

  .name {
    margin: 0;
    font-weight: 600;
  }

  p {
    margin: 0;
  }

  .state {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex: 0 1 12rem;
  }

  .bar {
    flex: 1 1 4rem;
  }

  .actions {
    flex: none;
  }

  .detail {
    padding: 0.5rem 0 0.25rem;
  }

  .notes {
    white-space: pre-wrap;
    max-height: 14rem;
    overflow-y: auto;
  }

  dl {
    display: grid;
    gap: 0.15rem;
    margin: 0.5rem 0 0;
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

  .duplicate {
    color: var(--warn);
  }
</style>
