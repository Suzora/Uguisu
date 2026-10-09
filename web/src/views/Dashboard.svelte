<!--
  What Uguisu is doing right now, from the endpoints that already answer it.

  Every number here comes from one API field. Nothing is derived, summed or
  estimated in the browser, so a count the UI shows is a count the backend
  stands behind — and a section that could not load says so instead of
  showing a zero.
-->
<script lang="ts">
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { events } from '../lib/events.svelte';
  import { archiveStats, recentEvents, status } from '../lib/api';
  import type { ArchiveStats, UguisuEvent, ServiceStatus } from '../lib/api';
  import { hrefFor } from '../lib/router';
  import { dateTime, eventLabel, relative } from '../lib/format';
  import { label, m } from '../lib/i18n';

  const service = new Resource<ServiceStatus>();
  const archive = new Resource<ArchiveStats>();
  const activity = new Resource<UguisuEvent[]>();

  function reload(): void {
    void service.load((signal) => status({ signal }));
    void archive.load((signal) => archiveStats({ signal }));
    void activity.load((signal) => recentEvents(12, { signal }));
  }

  $effect(() => {
    reload();
    return () => {
      service.cancel();
      archive.cancel();
      activity.cancel();
    };
  });

  // The stream is the refresh trigger: a quiet library costs no requests, and
  // a busy one never polls faster than things actually happen.
  $effect(() =>
    events.subscribe((event) => {
      if (event.kind !== 'download.progress') {
        void activity.load((signal) => recentEvents(12, { signal }));
      }
    }),
  );
  $effect(() => events.onResync(reload));

  const downloads = $derived(service.data?.downloads ?? null);
  const scheduler = $derived(service.data?.scheduler ?? null);
  const search = $derived(service.data?.search ?? null);

  /** The episode's title when the event carries one. */
  function episodeName(event: UguisuEvent): string {
    return 'title' in event && typeof event.title === 'string' ? event.title : m.dashboard.activity.episode;
  }

  function count(state: string): number {
    return downloads?.by_state?.[state as keyof typeof downloads.by_state] ?? 0;
  }
</script>

<h1>{m.dashboard.title}</h1>

{#if service.state === 'error'}
  <StateBlock state="error" title={m.dashboard.unreadable} error={service.error} onretry={reload} />
{:else if service.data === null}
  <StateBlock state="loading" />
{:else}
  <div class="grid">
    <section class="card">
      <h2>{m.dashboard.library.title}</h2>
      <p class="figure">{service.data.podcasts}</p>
      <p class="muted small">{m.dashboard.library.subscribed(service.data.podcasts)}</p>
      {#if archive.error}
        <p class="small err">{m.dashboard.library.totalsUnavailable}</p>
      {:else if archive.data}
        <p class="small">
          {m.dashboard.library.archivedFiles(archive.data.total)}
          {#if archive.data.by_state.missing}· <Link href={hrefFor({ name: 'archiveRepair' })}><span class="err">{m.dashboard.library.missing(archive.data.by_state.missing)}</span></Link>{/if}
          {#if archive.data.by_state.invalid}· <span class="err">{m.dashboard.library.invalid(archive.data.by_state.invalid)}</span>{/if}
        </p>
      {/if}
      <p><Link href="/podcasts">{m.dashboard.library.open}</Link></p>
    </section>

    <section class="card">
      <h2>{m.dashboard.downloads.title}</h2>
      <p class="figure">{count('downloading') + count('finalizing')}</p>
      <p class="muted small">{m.dashboard.downloads.running}</p>
      <ul class="facts small">
        <li>{m.dashboard.downloads.queued(count('queued'))}</li>
        <li>{m.dashboard.downloads.retrying(count('retrying'))}</li>
        <li>{m.dashboard.downloads.paused(count('paused'))}</li>
        <li class:err={count('failed') > 0}>{m.dashboard.downloads.failed(count('failed'))}</li>
        <li>{m.dashboard.downloads.completed(count('completed'))}</li>
      </ul>
      {#if !downloads?.workers_started}
        <p class="small muted">
          {m.dashboard.downloads.noWorkers.before}<code>{m.dashboard.downloads.noWorkers.command}</code
          >{m.dashboard.downloads.noWorkers.after}
        </p>
      {/if}
      {#if downloads?.paused_all}
        <p class="small"><Badge value={m.dashboard.downloads.pausedAll(label(downloads.paused_all))} tone="warn" /></p>
      {/if}
      <p><Link href="/downloads">{m.dashboard.downloads.open}</Link></p>
    </section>

    <section class="card">
      <h2>{m.dashboard.scheduler.title}</h2>
      <p class="figure">{scheduler?.due_now ?? 0}</p>
      <p class="muted small">{m.dashboard.scheduler.due(scheduler?.due_now ?? 0)}</p>
      <ul class="facts small">
        <li>
          <Badge
            value={scheduler?.paused
              ? m.dashboard.scheduler.paused
              : scheduler?.running
                ? m.dashboard.scheduler.running
                : m.dashboard.scheduler.stopped}
            tone={scheduler?.paused ? 'warn' : scheduler?.running ? 'ok' : 'neutral'}
          />
        </li>
        <li>{m.dashboard.scheduler.next(relative(scheduler?.next_due_at))}</li>
        <li>{m.dashboard.scheduler.interval(Math.round((scheduler?.interval_secs ?? 0) / 60))}</li>
      </ul>
      <p><Link href="/service">{m.dashboard.scheduler.open}</Link></p>
    </section>

    <section class="card">
      <h2>{m.dashboard.search.title}</h2>
      <p class="figure"><Badge value={search?.state} /></p>
      <p class="muted small">
        {m.dashboard.search.indexed(search?.episodes ?? 0, search?.podcasts ?? 0)}
      </p>
      {#if search?.detail}
        <p class="small muted">{search.detail}</p>
      {/if}
      <p><Link href="/search">{m.dashboard.search.open}</Link></p>
    </section>
  </div>

  {#if service.data.settings_problems > 0}
    <p class="card warn-card">
      {m.dashboard.ignoredSettings.before(service.data.settings_problems)}<Link href="/settings"
        >{m.dashboard.ignoredSettings.review}</Link
      >{m.dashboard.ignoredSettings.after}
    </p>
  {/if}

  <section class="card activity">
    <h2>{m.dashboard.activity.title}</h2>
    {#if activity.error}
      <StateBlock state="error" title={m.dashboard.activity.unreadable} error={activity.error} />
    {:else if activity.data === null}
      <StateBlock state="loading" />
    {:else if activity.data.length === 0}
      <StateBlock state="empty" title={m.dashboard.activity.none} hint={m.dashboard.activity.noneHint} />
    {:else}
      <ul class="events">
        {#each activity.data.slice().reverse() as event (event.id)}
          <li>
            <span class="when muted small" title={dateTime(event.occurred_at)}>
              {relative(event.occurred_at)}
            </span>
            <span>{eventLabel(event.kind)}</span>
            {#if event.episode_id}
              <Link href={hrefFor({ name: 'episode', id: event.episode_id })}>{episodeName(event)}</Link>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .figure {
    font-size: 2rem;
    font-weight: 600;
    margin: 0;
    line-height: 1.1;
  }

  p {
    margin: 0 0 0.35rem;
  }

  .facts {
    list-style: none;
    margin: 0.5rem 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem 0.75rem;
  }

  .err {
    color: var(--err);
  }

  .warn-card {
    margin-top: var(--gap);
    border-left: 3px solid var(--warn);
  }

  .activity {
    margin-top: var(--gap);
  }

  .events {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .events li {
    display: flex;
    gap: 0.75rem;
    padding: 0.3rem 0;
    border-bottom: 1px solid var(--border);
  }

  .events li:last-child {
    border-bottom: none;
  }

  .when {
    flex: none;
    width: 9rem;
  }
</style>
