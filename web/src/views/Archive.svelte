<!--
  What is on disk and whether it is still what Uguisu wrote.

  Read-oriented on purpose: verification and manifests are the only writes
  offered, because nothing in Uguisu deletes a media file and this view is
  not where that would change.
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
    archiveStats,
    listArchive,
    listManifests,
    listPodcasts,
    messageFor,
    verifyArchive,
    verifyOne,
  } from '../lib/api';
  import type { ArchiveFile, ArchiveManifest, ArchivePage, ArchiveStats } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { inDesktopShell, revealFile } from '../lib/api/desktop';
  import { hrefFor, withQuery } from '../lib/router';
  import { bytes, dateTime, relative } from '../lib/format';
  import { label, m } from '../lib/i18n';

  interface Props {
    query: Record<string, string>;
  }

  const { query }: Props = $props();

  const PAGE = 200;
  const stats = new Resource<ArchiveStats>();
  const first = new Resource<ArchivePage>();
  const manifests = new Resource<ArchiveManifest[]>();
  let files = $state<ArchiveFile[]>([]);
  let cursor = $state<string | null>(null);
  let loadingMore = $state(false);
  let titles = $state<Record<string, string>>({});
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let busy = $state<string | null>(null);
  // Constant for the life of the page: the shell is there or it is not.
  const onDesktop = inDesktopShell();

  const selected = $derived(query.state ?? '');
  const podcast = $derived(query.podcast ?? '');

  async function reload(): Promise<void> {
    void stats.load((signal) => archiveStats({ signal }));
    void manifests.load((signal) => listManifests({ signal }));
    const page = await first.load((signal) =>
      listArchive({ state: selected, podcast, limit: PAGE }, { signal }),
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
      const page = await listArchive({ state: selected, podcast, after: cursor, limit: PAGE });
      files = [...files, ...page.files];
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
      stats.cancel();
      first.cancel();
      manifests.cancel();
    };
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

  $effect(() =>
    events.subscribe((event) => {
      if (event.kind.startsWith('archive.')) {
        void reload();
      }
    }),
  );
  $effect(() => events.onResync(() => void reload()));

  async function act(key: string, run: () => Promise<unknown>, done: string): Promise<void> {
    busy = key;
    notice = null;
    try {
      await run();
      // An action that reported what it found keeps its report.
      notice ??= { tone: 'ok', text: done };
      void reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      busy = null;
    }
  }

  const staleManifests = $derived((manifests.data ?? []).filter((manifest) => manifest.stale));
</script>

<h1>{m.archive.title}</h1>
<p class="small"><Link href={hrefFor({ name: 'archiveImport' })}>{m.archiveImport.link}</Link></p>

{#if stats.data}
  <p class="row small counters">
    {#each ['verified', 'unchecked', 'missing', 'invalid'] as key (key)}
      <button
        type="button"
        class="counter"
        class:selected={selected === key}
        onclick={() =>
          navigation.go(withQuery(navigation.current, { state: selected === key ? undefined : key }), {
            replace: true,
          })}
      >
        <Badge value={key} />
        <span>{stats.data.by_state[key as keyof typeof stats.data.by_state] ?? 0}</span>
      </button>
    {/each}
    <span class="muted">{m.archive.total(stats.data.total)}</span>
    {#if stats.data.by_state.missing}
      <Link href={hrefFor({ name: 'archiveRepair' })}>{m.archiveRepair.link}</Link>
    {/if}
  </p>
{:else if stats.error}
  <Notice tone="err">{m.archive.totalsUnreadable(messageFor(stats.error))}</Notice>
{/if}

<div class="row actions">
  <button
    type="button"
    disabled={busy !== null}
    onclick={() =>
      act('verify-light', async () => {
        const summary = await verifyArchive({ podcast, depth: 'light' });
        notice = {
          tone: 'ok',
          text: m.archive.checkedReport(summary.checked, summary.verified, summary.missing, summary.invalid),
        };
      }, m.archive.checkedNotice)}
  >
    {busy === 'verify-light' ? m.archive.checking : m.archive.checkButton}
  </button>
  <button
    type="button"
    disabled={busy !== null}
    onclick={() =>
      act('verify-full', async () => {
        const summary = await verifyArchive({ podcast, depth: 'full' });
        notice = {
          tone: 'ok',
          text: m.archive.hashedReport(summary.checked, summary.verified, summary.missing, summary.invalid),
        };
      }, m.archive.verifiedNotice)}
  >
    {busy === 'verify-full' ? m.archive.hashing : m.archive.verifyAllButton}
  </button>
  <span class="small muted">{m.archive.hashingHint}</span>
</div>

{#if notice}
  <Notice tone={notice.tone}>{notice.text}</Notice>
{/if}

{#if staleManifests.length > 0}
  <Notice tone="info">
    {m.archive.staleManifests.count(staleManifests.length)}
    {m.archive.staleManifests.before(staleManifests.length)}<code>{m.archive.staleManifests.command}</code>{m.archive.staleManifests.end}
  </Notice>
{/if}

{#if podcast}
  <p class="small muted">
    {m.archive.onlyPodcast(titles[podcast] ?? podcast)}
    <button
      type="button"
      class="linky"
      onclick={() => navigation.go(withQuery(navigation.current, { podcast: undefined }), { replace: true })}
    >
      {m.archive.everyPodcast}
    </button>
  </p>
{/if}

{#if first.state === 'error'}
  <StateBlock state="error" title={m.archive.unreadable} error={first.error} onretry={() => void reload()} />
{:else if first.data === null}
  <StateBlock state="loading" />
{:else if files.length === 0}
  <StateBlock
    state="empty"
    title={selected ? m.archive.noneInState(selected) : m.archive.none}
    hint={selected ? undefined : m.archive.noneHint}
  />
{:else}
  <div class="scroller">
    <table>
      <caption class="visually-hidden">{m.archive.caption}</caption>
      <thead>
        <tr>
          <th scope="col">{m.archive.columns.file}</th>
          <th scope="col">{m.archive.columns.state}</th>
          <th scope="col">{m.archive.columns.size}</th>
          <th scope="col">{m.archive.columns.sidecar}</th>
          <th scope="col">{m.archive.columns.tags}</th>
          <th scope="col">{m.archive.columns.checked}</th>
          <th scope="col"><span class="visually-hidden">{m.archive.columns.actions}</span></th>
        </tr>
      </thead>
      <tbody>
        {#each files as file (file.id)}
          <tr>
            <td data-label={m.archive.columns.file}>
              <Link href={hrefFor({ name: 'podcast', id: file.podcast_id })}>
                {titles[file.podcast_id] ?? file.podcast_id}
              </Link>
              <br />
              <Link href={hrefFor({ name: 'episode', id: file.episode_id })}>
                <code class="small">{file.relative_path}</code>
              </Link>
            </td>
            <td data-label={m.archive.columns.state}>
              <Badge value={file.verification_state} />
              {#if file.verification_reason}
                <p class="small muted">{label(file.verification_reason)}</p>
              {/if}
            </td>
            <td data-label={m.archive.columns.size}>{bytes(file.size_bytes)}</td>
            <td class="small" data-label={m.archive.columns.sidecar}>
              {file.sidecar_written_at ? relative(file.sidecar_written_at) : m.archive.noSidecar}
            </td>
            <td data-label={m.archive.columns.tags}><Badge value={file.tag_state} /></td>
            <td class="small" data-label={m.archive.columns.checked} title={dateTime(file.verified_at)}>{relative(file.verified_at)}</td>
            <td data-label={m.archive.columns.actions}>
              <button
                type="button"
                disabled={busy !== null}
                onclick={() =>
                  act(file.id, async () => {
                    const checked = await verifyOne(file.episode_id, 'full');
                    notice = {
                      tone: checked.state === 'verified' ? 'ok' : 'err',
                      text: `${checked.state}: ${label(checked.reason)}${checked.detail ? ` — ${checked.detail}` : ''}`,
                    };
                  }, m.archive.verifiedNotice)}
              >
                {m.archive.verifyButton}
              </button>
              {#if onDesktop}
                <button
                  type="button"
                  disabled={busy !== null}
                  onclick={() =>
                    act(file.id, () => revealFile(file.relative_path), m.archive.revealedNotice)}
                >
                  {m.archive.revealButton}
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
    min-width: 48rem;
  }

  p {
    margin: 0.2rem 0;
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
