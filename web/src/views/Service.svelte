<!--
  The daemon: what it is doing, and the few controls the API already exposes.

  The Phase-7 page was read-only. The mutations added here — pause, resume,
  one pass now, maintenance, refresh everything, rebuild the index — are all
  endpoints that already existed for the CLI, and each is followed by an
  authoritative re-read.
-->
<script lang="ts">
  import Badge from '../lib/components/Badge.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { events } from '../lib/events.svelte';
  import {
    messageFor,
    pauseScheduler,
    refreshAllPodcasts,
    reindex,
    resumeScheduler,
    runMaintenance,
    runSchedulerPass,
    resumeAllDownloads,
    pauseAllDownloads,
    status,
  } from '../lib/api';
  import type { ServiceStatus } from '../lib/api';
  import { dateTime, relative } from '../lib/format';
  import { label, m } from '../lib/i18n';

  const service = new Resource<ServiceStatus>();
  let notice = $state<{ tone: 'ok' | 'err' | 'info'; text: string } | null>(null);
  let busy = $state<string | null>(null);
  let reason = $state('');

  function reload(): void {
    void service.load((signal) => status({ signal }));
  }

  $effect(() => {
    reload();
    return () => service.cancel();
  });

  $effect(() =>
    events.subscribe((event) => {
      if (event.kind.startsWith('scheduler.') || event.kind.startsWith('search.')) {
        reload();
      }
    }),
  );
  $effect(() => events.onResync(reload));

  async function act(key: string, run: () => Promise<unknown>, done: string): Promise<void> {
    busy = key;
    notice = null;
    try {
      await run();
      notice = { tone: 'ok', text: done };
      reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      busy = null;
    }
  }

  const scheduler = $derived(service.data?.scheduler ?? null);
  const search = $derived(service.data?.search ?? null);
  const downloads = $derived(service.data?.downloads ?? null);
</script>

<h1>{m.service.title}</h1>

{#if service.state === 'error'}
  <StateBlock state="error" title={m.service.unreadable} error={service.error} onretry={reload} />
{:else if service.data === null}
  <StateBlock state="loading" />
{:else}
  {#if notice}
    <Notice tone={notice.tone}>{notice.text}</Notice>
  {/if}

  <div class="grid">
    <section class="card">
      <h2>{m.service.scheduler.title}</h2>
      <dl>
        <div>
          <dt>{m.service.scheduler.state}</dt>
          <dd>
            <Badge
              value={scheduler?.paused
                ? m.service.scheduler.paused
                : scheduler?.running
                  ? m.service.scheduler.running
                  : m.service.scheduler.notRunning}
              tone={scheduler?.paused ? 'warn' : scheduler?.running ? 'ok' : 'neutral'}
            />
          </dd>
        </div>
        {#if scheduler?.paused_reason}
          <div><dt>{m.service.scheduler.pausedReason}</dt><dd>{scheduler.paused_reason}</dd></div>
        {/if}
        {#if scheduler?.paused_at}
          <div><dt>{m.service.scheduler.pausedAt}</dt><dd title={dateTime(scheduler.paused_at)}>{relative(scheduler.paused_at)}</dd></div>
        {/if}
        <div><dt>{m.service.scheduler.enabled}</dt><dd>{scheduler?.enabled ? m.service.yes : m.service.no}</dd></div>
        <div><dt>{m.service.scheduler.dueNow}</dt><dd>{scheduler?.due_now ?? 0}</dd></div>
        <div><dt>{m.service.scheduler.nextDue}</dt><dd title={dateTime(scheduler?.next_due_at)}>{relative(scheduler?.next_due_at)}</dd></div>
        <div><dt>{m.service.scheduler.inFlight}</dt><dd>{m.service.scheduler.inFlightOf(scheduler?.inflight ?? 0, scheduler?.concurrency ?? 0)}</dd></div>
        <div><dt>{m.service.scheduler.interval}</dt><dd>{m.service.scheduler.intervalMinutes(Math.round((scheduler?.interval_secs ?? 0) / 60))}</dd></div>
        <div>
          <dt>{m.service.scheduler.lastMaintenance}</dt>
          <dd title={dateTime(scheduler?.last_maintenance_at)}>{relative(scheduler?.last_maintenance_at)}</dd>
        </div>
      </dl>

      {#if scheduler?.paused}
        <button
          type="button"
          class="primary"
          disabled={busy !== null}
          onclick={() => act('resume', resumeScheduler, m.service.scheduler.resumedNotice)}
        >
          {m.service.scheduler.resumeButton}
        </button>
      {:else}
        <div class="row">
          <label>
            <span class="visually-hidden">{m.service.scheduler.reasonLabel}</span>
            <input placeholder={m.service.scheduler.reasonPlaceholder} bind:value={reason} />
          </label>
          <button
            type="button"
            disabled={busy !== null}
            onclick={() =>
              act('pause', () => pauseScheduler(reason.trim() || null), m.service.scheduler.pausedNotice)}
          >
            {m.service.scheduler.pauseButton}
          </button>
        </div>
      {/if}

      <div class="row">
        <button
          type="button"
          disabled={busy !== null}
          onclick={() =>
            act('pass', async () => {
              const pass = await runSchedulerPass();
              notice = {
                tone: pass.paused ? 'info' : 'ok',
                text: pass.paused
                  ? m.service.scheduler.passSuppressed
                  : m.service.scheduler.passStarted(pass.started, pass.due),
              };
            }, m.service.scheduler.passNotice)}
        >
          {m.service.scheduler.passButton}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onclick={() =>
            act('maintenance', async () => {
              const report = await runMaintenance();
              notice = {
                tone: 'ok',
                text: m.service.scheduler.housekeepingReport(report.events_pruned, report.cache_expired),
              };
            }, m.service.scheduler.housekeepingNotice)}
        >
          {m.service.scheduler.housekeepingButton}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onclick={() => act('refresh-all', refreshAllPodcasts, m.service.scheduler.refreshedNotice)}
        >
          {busy === 'refresh-all' ? m.service.scheduler.refreshing : m.service.scheduler.refreshAllButton}
        </button>
      </div>
      <p class="small muted">{m.service.scheduler.refreshAllHint}</p>
    </section>

    <section class="card">
      <h2>{m.service.downloads.title}</h2>
      <dl>
        <div><dt>{m.service.downloads.workers}</dt><dd>{downloads?.workers_started ? m.service.downloads.workersRunning : m.service.downloads.workersNotStarted}</dd></div>
        <div><dt>{m.service.downloads.active}</dt><dd>{downloads?.running ?? 0}</dd></div>
        <div>
          <dt>{m.service.downloads.queue}</dt>
          <dd>{downloads?.paused_all ? m.service.downloads.queuePaused(label(downloads.paused_all)) : m.service.downloads.queueRunning}</dd>
        </div>
        <div><dt>{m.service.downloads.nextRetry}</dt><dd title={dateTime(downloads?.next_retry_at)}>{relative(downloads?.next_retry_at)}</dd></div>
        <div><dt>{m.service.downloads.orphanParts}</dt><dd>{downloads?.orphan_parts ?? 0}</dd></div>
      </dl>
      <div class="row">
        {#if downloads?.paused_all}
          <button
            type="button"
            disabled={busy !== null}
            onclick={() => act('dl-resume', resumeAllDownloads, m.service.downloads.resumedNotice)}
          >
            {m.service.downloads.resumeButton}
          </button>
        {:else}
          <button
            type="button"
            disabled={busy !== null}
            onclick={() => act('dl-pause', pauseAllDownloads, m.service.downloads.pausedNotice)}
          >
            {m.service.downloads.pauseButton}
          </button>
        {/if}
      </div>
    </section>

    <section class="card">
      <h2>{m.service.search.title}</h2>
      <dl>
        <div><dt>{m.service.search.state}</dt><dd><Badge value={search?.state} /></dd></div>
        <div><dt>{m.service.search.podcasts}</dt><dd>{search?.podcasts ?? 0}</dd></div>
        <div><dt>{m.service.search.episodes}</dt><dd>{search?.episodes ?? 0}</dd></div>
        <div><dt>{m.service.search.built}</dt><dd title={dateTime(search?.built_at)}>{relative(search?.built_at)}</dd></div>
        {#if search?.detail}
          <div><dt>{m.service.search.detail}</dt><dd>{search.detail}</dd></div>
        {/if}
      </dl>
      <button
        type="button"
        disabled={busy !== null}
        onclick={() =>
          act('reindex', async () => {
            const report = await reindex();
            notice = {
              tone: 'ok',
              text: m.service.search.rebuiltReport(report.episodes, report.podcasts, report.duration_ms),
            };
          }, m.service.search.rebuiltNotice)}
      >
        {busy === 'reindex' ? m.service.search.rebuilding : m.service.search.rebuildButton}
      </button>
      <p class="small muted">{m.service.search.rebuildHint}</p>
    </section>

    <section class="card">
      <h2>{m.service.build.title}</h2>
      <dl>
        <div><dt>{m.service.build.version}</dt><dd>{service.data.version}</dd></div>
        <div><dt>{m.service.build.podcasts}</dt><dd>{service.data.podcasts}</dd></div>
        <div><dt>{m.service.build.settingsProblems}</dt><dd>{service.data.settings_problems}</dd></div>
      </dl>
      <p class="small muted">
        {m.service.build.noAuthentication}
      </p>
    </section>
  </div>
{/if}

<style>
  .grid {
    grid-template-columns: repeat(auto-fit, minmax(19rem, 1fr));
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

  p {
    margin: 0.4rem 0 0;
  }

  .row {
    margin-bottom: 0.5rem;
  }
</style>
