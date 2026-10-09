<!--
  Repairing archived files a check found missing (ADR 0060): put the exact
  bytes back from a folder on the server, or download the episode again.

  Restoring comes first on purpose. It brings back the bytes the record
  describes; a new download may bring different ones.
-->
<script lang="ts">
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import Pager from '../lib/components/Pager.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { events } from '../lib/events.svelte';
  import {
    ApiFailure,
    listArchive,
    listPodcasts,
    messageFor,
    redownload,
    restoreArchive,
    verifyArchive,
  } from '../lib/api';
  import type { ArchiveFile, ArchivePage, RestoreBody, RestoreRequest } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { hrefFor, withQuery } from '../lib/router';
  import { bytes, dateTime, relative } from '../lib/format';
  import { m } from '../lib/i18n';

  interface Props {
    query: Record<string, string>;
  }

  const { query }: Props = $props();

  const PAGE = 200;
  const ACTIONS = ['restore', 'returned', 'source_only', 'taken', 'changed', 'not_found', 'failed'] as const;
  const TONES: Record<string, 'ok' | 'neutral' | 'warn' | 'err'> = {
    restore: 'ok',
    returned: 'ok',
    source_only: 'warn',
    taken: 'err',
    changed: 'warn',
    not_found: 'neutral',
    failed: 'err',
  };

  const podcast = $derived(query.podcast ?? '');
  const first = new Resource<ArchivePage>();
  let files = $state<ArchiveFile[]>([]);
  let cursor = $state<string | null>(null);
  let loadingMore = $state(false);
  let titles = $state<Record<string, string>>({});
  /** What is running: `check`, `look`, `apply`, `all`, or one episode's id. */
  let busy = $state<string | null>(null);
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let path = $state('');
  let plan = $state<RestoreBody | null>(null);
  /** The request `plan` answers, sent again unchanged to apply it. */
  let planned = $state<RestoreRequest | null>(null);
  /** The state of the job each repaired file's download is now in. */
  let queued = $state<Record<string, string>>({});

  const found = $derived((plan?.items ?? []).filter((item) => item.action === 'restore').length);
  const waiting = $derived(files.filter((file) => !queued[file.episode_id]));

  async function reload(): Promise<void> {
    const page = await first.load((signal) =>
      listArchive({ state: 'missing', podcast, limit: PAGE }, { signal }),
    );
    if (page) {
      files = page.files;
      cursor = page.next_after ?? null;
    }
  }

  async function loadMore(): Promise<void> {
    if (!cursor || loadingMore) {
      return;
    }
    loadingMore = true;
    try {
      const page = await listArchive({ state: 'missing', podcast, after: cursor, limit: PAGE });
      files = [...files, ...page.files];
      cursor = page.next_after ?? null;
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      loadingMore = false;
    }
  }

  $effect(() => {
    void podcast;
    void reload();
    return () => first.cancel();
  });

  $effect(() => {
    void listPodcasts()
      .then((entries) => {
        titles = Object.fromEntries(entries.map((e) => [e.podcast.id, e.podcast.title]));
      })
      .catch(() => {
        // Ids remain readable without the library.
      });
  });

  // A check or a restore reports one event per file; the list is read once
  // when it ends instead.
  $effect(() =>
    events.subscribe((event) => {
      if (busy === null && event.kind.startsWith('archive.')) {
        void reload();
      }
    }),
  );
  $effect(() => events.onResync(() => void reload()));

  async function run(key: string, work: () => Promise<void>): Promise<void> {
    if (busy !== null) {
      return;
    }
    busy = key;
    notice = null;
    try {
      await work();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      busy = null;
      void reload();
    }
  }

  function check(): Promise<void> {
    return run('check', async () => {
      const summary = await verifyArchive({ podcast, depth: 'light' });
      notice = { tone: 'ok', text: m.archiveRepair.check.report(summary.checked, summary.missing) };
    });
  }

  function look(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (path.trim() === '') {
      return Promise.resolve();
    }
    return run('look', async () => {
      plan = null;
      planned = null;
      // Sent as typed: a folder may end in a space.
      const asked: RestoreRequest = { path, podcast: podcast === '' ? null : podcast, apply: false };
      plan = await restoreArchive(asked);
      planned = asked;
    });
  }

  function apply(): Promise<void> {
    return run('apply', async () => {
      if (planned === null) {
        return;
      }
      try {
        plan = await restoreArchive({ ...planned, apply: true });
      } catch (failure) {
        // Only a lost answer may have copied anything; a refusal copied nothing.
        const text =
          failure instanceof ApiFailure && failure.retryable
            ? m.archiveRepair.folder.lost(messageFor(failure))
            : messageFor(failure);
        notice = { tone: 'err', text };
      }
    });
  }

  /** Asks for one file again; true when a download was queued for it. */
  async function ask(file: ArchiveFile): Promise<boolean> {
    const result = await redownload(file.episode_id);
    queued = { ...queued, [file.episode_id]: result.job.state };
    return result.outcome === 'created' || result.outcome === 'requeued';
  }

  function again(file: ArchiveFile): Promise<void> {
    return run(file.episode_id, async () => {
      await ask(file);
    });
  }

  function againAll(): Promise<void> {
    return run('all', async () => {
      let count = 0;
      const refused: string[] = [];
      for (const file of waiting) {
        try {
          if (await ask(file)) {
            count += 1;
          }
        } catch (failure) {
          refused.push(`${file.relative_path}: ${messageFor(failure)}`);
        }
      }
      notice =
        refused.length === 0
          ? { tone: 'ok', text: m.archiveRepair.again.queuedAll(count) }
          : { tone: 'err', text: m.archiveRepair.again.refused(count, refused) };
    });
  }
</script>

<h1>{m.archiveRepair.title}</h1>
<p class="muted">{m.archiveRepair.intro}</p>

{#if podcast}
  <p class="small muted">
    {m.archiveRepair.onlyPodcast(titles[podcast] ?? podcast)}
    <button
      type="button"
      class="linky"
      onclick={() => navigation.go(withQuery(navigation.current, { podcast: undefined }), { replace: true })}
    >
      {m.archiveRepair.everyPodcast}
    </button>
  </p>
{/if}

{#if notice}
  <Notice tone={notice.tone}>{notice.text}</Notice>
{/if}

<section class="card">
  <h2>{m.archiveRepair.check.title}</h2>
  <div class="row">
    <button type="button" disabled={busy !== null} onclick={() => void check()}>
      {busy === 'check' ? m.archiveRepair.check.checking : m.archiveRepair.check.button}
    </button>
    <span class="small muted">{m.archiveRepair.check.hint}</span>
  </div>

  {#if first.state === 'error'}
    <StateBlock state="error" title={m.archiveRepair.check.unreadable} error={first.error} onretry={() => void reload()} />
  {:else if first.data === null}
    <StateBlock state="loading" />
  {:else if files.length === 0}
    <p>{m.archiveRepair.check.none}</p>
  {:else}
    <p>{m.archiveRepair.check.count(files.length, cursor !== null)}</p>
    <div class="scroller">
      <table class="small">
        <caption class="visually-hidden">{m.archiveRepair.check.caption}</caption>
        <thead>
          <tr>
            <th scope="col">{m.archiveRepair.check.columns.file}</th>
            <th scope="col">{m.archiveRepair.check.columns.size}</th>
            <th scope="col">{m.archiveRepair.check.columns.checked}</th>
            <th scope="col"><span class="visually-hidden">{m.archiveRepair.check.columns.actions}</span></th>
          </tr>
        </thead>
        <tbody>
          {#each files as file (file.id)}
            <tr>
              <td data-label={m.archiveRepair.check.columns.file}>
                <Link href={hrefFor({ name: 'podcast', id: file.podcast_id })}>
                  {titles[file.podcast_id] ?? file.podcast_id}
                </Link>
                <br />
                <Link href={hrefFor({ name: 'episode', id: file.episode_id })}><code>{file.relative_path}</code></Link>
              </td>
              <td data-label={m.archiveRepair.check.columns.size}>{bytes(file.size_bytes)}</td>
              <td data-label={m.archiveRepair.check.columns.checked} title={dateTime(file.verified_at)}>
                {relative(file.verified_at)}
              </td>
              <td data-label={m.archiveRepair.check.columns.actions}>
                {#if queued[file.episode_id]}
                  <Badge value={queued[file.episode_id]} />
                {:else}
                  <button type="button" disabled={busy !== null} onclick={() => void again(file)}>
                    {m.archiveRepair.again.one}
                  </button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <Pager shown={files.length} more={cursor !== null} busy={loadingMore} onmore={() => void loadMore()} />
  {/if}
</section>

<section class="card">
  <h2>{m.archiveRepair.folder.title}</h2>
  <form onsubmit={(event) => void look(event)}>
    <div class="field">
      <label>
        <span>{m.archiveRepair.folder.path}</span>
        <input
          type="text"
          bind:value={path}
          required
          disabled={busy !== null}
          autocomplete="off"
          spellcheck="false"
          aria-describedby="repair-path-hint"
        />
      </label>
      <span id="repair-path-hint" class="small muted">{m.archiveRepair.folder.pathHint}</span>
    </div>
    <button type="submit" disabled={busy !== null || path.trim() === '' || files.length === 0}>
      {busy === 'look' ? m.archiveRepair.folder.looking : m.archiveRepair.folder.look}
    </button>
  </form>

  {#if plan}
    <p>
      {plan.applied ? m.archiveRepair.folder.applied(found) : m.archiveRepair.folder.planned(plan.scanned, found)}
    </p>
    {#if !plan.applied && found > 0}
      <button type="button" class="primary" disabled={busy !== null} onclick={() => void apply()}>
        {m.archiveRepair.folder.restore(found)}
      </button>
    {/if}
    <p class="row small counters">
      {#each ACTIONS as key (key)}
        {@const count = plan.items.filter((item) => item.action === key).length}
        {#if count > 0}
          <span class="counter"><Badge value={key} tone={TONES[key]} /> {count}</span>
        {/if}
      {/each}
    </p>
    {#if plan.items.length > 0}
      <div class="scroller">
        <table class="small">
          <caption class="visually-hidden">{m.archiveRepair.folder.caption}</caption>
          <thead>
            <tr>
              <th scope="col">{m.archiveRepair.folder.columns.file}</th>
              <th scope="col">{m.archiveRepair.folder.columns.outcome}</th>
              <th scope="col">{m.archiveRepair.folder.columns.source}</th>
              <th scope="col">{m.archiveRepair.folder.columns.note}</th>
            </tr>
          </thead>
          <tbody>
            {#each plan.items as item (item.episode_id)}
              <tr>
                <td data-label={m.archiveRepair.folder.columns.file}>
                  <Link href={hrefFor({ name: 'episode', id: item.episode_id })}><code>{item.target_path}</code></Link>
                </td>
                <td data-label={m.archiveRepair.folder.columns.outcome}><Badge value={item.action} tone={TONES[item.action]} /></td>
                <td data-label={m.archiveRepair.folder.columns.source}>
                  {#if item.source_path}<code>{item.source_path}</code>{/if}
                </td>
                <td class="muted" data-label={m.archiveRepair.folder.columns.note}>{item.detail ?? ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</section>

<section class="card">
  <h2>{m.archiveRepair.again.title}</h2>
  <p class="small muted">{m.archiveRepair.again.hint}</p>
  <div class="row">
    <button type="button" disabled={busy !== null || waiting.length === 0} onclick={() => void againAll()}>
      {m.archiveRepair.again.all(waiting.length)}
    </button>
    {#if Object.keys(queued).length > 0}
      <Link href={hrefFor({ name: 'downloads' })}>{m.archiveRepair.again.downloads}</Link>
    {/if}
  </div>
</section>

<style>
  section {
    margin-bottom: var(--gap);
  }

  form {
    display: grid;
    gap: 0.75rem;
    margin-bottom: 0.75rem;
  }

  label,
  .field {
    display: grid;
    gap: 0.25rem;
  }

  .counters {
    gap: 0.5rem;
    margin: 0.75rem 0 0.5rem;
  }

  .counter {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
  }

  .scroller {
    overflow-x: auto;
  }

  code {
    word-break: break-all;
  }

  .linky {
    border: none;
    background: none;
    padding: 0;
    min-height: 0;
    color: var(--accent);
    text-decoration: underline;
  }
</style>
