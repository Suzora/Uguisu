<!--
  Importing an archive another tool wrote (ADR 0059): a folder on the server,
  read and matched first, copied only when asked.

  The whole run is one request. While a copy runs the page counts the
  `archive.imported` events; an interrupted handler sends no completion event,
  so a lost answer is not waited for, and running the same request again
  continues with what was not copied.
-->
<script lang="ts">
  import Badge from '../lib/components/Badge.svelte';
  import Link from '../lib/components/Link.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import { ApiFailure, importArchive, listPodcasts, messageFor } from '../lib/api';
  import type { ImportBody, ImportRequest } from '../lib/api';
  import { events } from '../lib/events.svelte';
  import { hrefFor } from '../lib/router';
  import { m } from '../lib/i18n';

  const PAGE = 200;
  const OUTCOMES = ['import', 'already_present', 'conflict', 'ambiguous', 'unmatched', 'invalid'] as const;
  const TONES: Record<string, 'ok' | 'neutral' | 'warn' | 'err'> = {
    import: 'ok',
    already_present: 'neutral',
    conflict: 'err',
    ambiguous: 'warn',
    unmatched: 'neutral',
    invalid: 'warn',
  };

  let path = $state('');
  let format = $state<'' | 'podgrab' | 'generic'>('');
  let podgrabDb = $state('');
  let podcast = $state('');
  let podcasts = $state<{ id: string; title: string }[]>([]);
  let plan = $state<ImportBody | null>(null);
  /** The request `plan` answers, sent again unchanged to apply it. */
  let planned = $state<ImportRequest | null>(null);
  let busy = $state<'plan' | 'apply' | null>(null);
  let copied = $state(0);
  /** The episodes the plan copies: another import's events are not this run's. */
  let copying = new Set<string>();
  let error = $state<string | null>(null);
  let outcome = $state('');
  let shown = $state(PAGE);

  const rows = $derived((plan?.items ?? []).filter((item) => outcome === '' || item.action === outcome));

  $effect(() => {
    void listPodcasts()
      .then((entries) => {
        podcasts = entries.map((e) => ({ id: e.podcast.id, title: e.podcast.title }));
      })
      .catch(() => {
        // The import still works for every podcast; only the narrowing is missing.
      });
  });

  $effect(() =>
    events.subscribe((event) => {
      if (busy === 'apply' && event.kind === 'archive.imported' && copying.has(event.episode_id ?? '')) {
        copied += 1;
      }
    }),
  );

  function request(apply: boolean): ImportRequest {
    // Sent as typed: a folder may end in a space, and the server says
    // which path it could not read.
    return {
      path,
      format: format === '' ? null : format,
      podgrab_db: podgrabDb.trim() === '' ? null : podgrabDb,
      podcast: podcast === '' ? null : podcast,
      apply,
    };
  }

  async function read(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy !== null || path.trim() === '') {
      return;
    }
    busy = 'plan';
    error = null;
    plan = null;
    planned = null;
    try {
      const asked = request(false);
      plan = await importArchive(asked);
      planned = asked;
      outcome = '';
      shown = PAGE;
    } catch (failure) {
      error = messageFor(failure);
    } finally {
      busy = null;
    }
  }

  async function apply(): Promise<void> {
    if (busy !== null || planned === null) {
      return;
    }
    busy = 'apply';
    copied = 0;
    copying = new Set((plan?.items ?? []).filter((i) => i.action === 'import').map((i) => i.episode_id ?? ''));
    error = null;
    try {
      plan = await importArchive({ ...planned, apply: true });
      outcome = '';
      shown = PAGE;
    } catch (failure) {
      // Only a lost answer may have copied anything; a refusal copied nothing.
      error =
        failure instanceof ApiFailure && failure.retryable
          ? m.archiveImport.lost(messageFor(failure))
          : messageFor(failure);
    } finally {
      busy = null;
    }
  }
</script>

<h1>{m.archiveImport.title}</h1>
<p class="muted">{m.archiveImport.intro}</p>

<form class="card" onsubmit={(event) => void read(event)}>
  <div class="field">
    <label>
      <span>{m.archiveImport.path}</span>
      <input
        type="text"
        bind:value={path}
        required
        disabled={busy !== null}
        autocomplete="off"
        spellcheck="false"
        aria-describedby="import-path-hint"
      />
    </label>
    <span id="import-path-hint" class="small muted">{m.archiveImport.pathHint}</span>
  </div>
  <div class="row">
    <label>
      <span>{m.archiveImport.format}</span>
      <select bind:value={format} disabled={busy !== null}>
        <option value="">{m.archiveImport.formats.detect}</option>
        <option value="podgrab">{m.archiveImport.formats.podgrab}</option>
        <option value="generic" disabled={podgrabDb.trim() !== ''}>{m.archiveImport.formats.generic}</option>
      </select>
    </label>
    <label>
      <span>{m.archiveImport.podcast}</span>
      <select bind:value={podcast} disabled={busy !== null}>
        <option value="">{m.archiveImport.everyPodcast}</option>
        {#each podcasts as entry (entry.id)}
          <option value={entry.id}>{entry.title}</option>
        {/each}
      </select>
    </label>
  </div>
  <div class="field">
    <label>
      <span>{m.archiveImport.podgrabDb}</span>
      <input
        type="text"
        bind:value={podgrabDb}
        disabled={busy !== null}
        autocomplete="off"
        spellcheck="false"
        aria-describedby="import-podgrab-db-hint"
      />
    </label>
    <span id="import-podgrab-db-hint" class="small muted">{m.archiveImport.podgrabDbHint}</span>
  </div>
  <button type="submit" class="primary" disabled={busy !== null || path.trim() === ''}>
    {busy === 'plan' ? m.archiveImport.reading : m.archiveImport.read}
  </button>
</form>

{#if error}
  <Notice tone="err">{error}</Notice>
{/if}

{#if busy === 'apply' && plan}
  <p role="status">{m.archiveImport.copying(copied, plan.imported)}</p>
{/if}

{#if plan}
  <p>
    {plan.applied ? m.archiveImport.applied(plan.imported) : m.archiveImport.planned(plan.scanned, plan.imported)}
  </p>
  {#if plan.unreadable > 0}
    <Notice tone="info">{m.archiveImport.unreadable(plan.unreadable)}</Notice>
  {/if}
  {#if !plan.applied && plan.imported > 0}
    <button type="button" class="primary" disabled={busy !== null} onclick={() => void apply()}>
      {m.archiveImport.copy(plan.imported)}
    </button>
  {/if}
  {#if plan.applied}
    <p><Link href={hrefFor({ name: 'archive' }, { state: 'unchecked' })}>{m.archiveImport.afterwards.verify}</Link></p>
    <p class="small muted">{m.archiveImport.afterwards.downloads}</p>
  {/if}

  <p class="row small counters">
    {#each OUTCOMES as key (key)}
      <button
        type="button"
        class="counter"
        class:selected={outcome === key}
        onclick={() => {
          outcome = outcome === key ? '' : key;
          shown = PAGE;
        }}
      >
        <Badge value={key} tone={TONES[key]} />
        <span>{plan.items.filter((item) => item.action === key).length}</span>
      </button>
    {/each}
    {#if outcome}
      <button type="button" onclick={() => (outcome = '')}>{m.archiveImport.everyOutcome}</button>
    {/if}
  </p>

  {#if rows.length > 0}
    <div class="scroller">
      <table class="small">
        <caption class="visually-hidden">{m.archiveImport.caption}</caption>
        <thead>
          <tr>
            <th scope="col">{m.archiveImport.columns.file}</th>
            <th scope="col">{m.archiveImport.columns.outcome}</th>
            <th scope="col">{m.archiveImport.columns.episode}</th>
            <th scope="col">{m.archiveImport.columns.confidence}</th>
            <th scope="col">{m.archiveImport.columns.target}</th>
            <th scope="col">{m.archiveImport.columns.note}</th>
          </tr>
        </thead>
        <tbody>
          {#each rows.slice(0, shown) as item, index (index)}
            <tr>
              <td data-label={m.archiveImport.columns.file}><code>{item.source_path}</code></td>
              <td data-label={m.archiveImport.columns.outcome}><Badge value={item.action} tone={TONES[item.action]} /></td>
              <td data-label={m.archiveImport.columns.episode}>
                {#if item.episode_id}
                  <Link href={hrefFor({ name: 'episode', id: item.episode_id })}><code>{item.episode_id}</code></Link>
                {/if}
              </td>
              <td data-label={m.archiveImport.columns.confidence}>
                {item.episode_id ? m.archiveImport.confidence(item.confidence) : ''}
              </td>
              <td data-label={m.archiveImport.columns.target}>
                {#if item.target_path}<code>{item.target_path}</code>{/if}
              </td>
              <td class="muted" data-label={m.archiveImport.columns.note}>{item.detail ?? ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if rows.length > shown}
      <button type="button" onclick={() => (shown += PAGE)}>{m.archiveImport.more(rows.length - shown)}</button>
    {/if}
  {/if}
{/if}

<style>
  form {
    display: grid;
    gap: 0.75rem;
    margin-bottom: var(--gap);
  }

  label,
  .field {
    display: grid;
    gap: 0.25rem;
  }

  .counters {
    gap: 0.35rem;
    margin: 0.75rem 0 0.5rem;
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

  .scroller {
    overflow-x: auto;
  }

  code {
    word-break: break-all;
  }
</style>
