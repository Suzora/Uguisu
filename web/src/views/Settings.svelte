<!--
  Persisted settings, with the Phase-7 precedence made visible.

  A value set in the environment or on the command line wins over anything
  stored, so those keys get no control at all: an input the server will refuse
  is worse than no input. Validation stays on the server — there is no second
  parser here.
-->
<script lang="ts">
  import ApiTokens from '../lib/components/ApiTokens.svelte';
  import Badge from '../lib/components/Badge.svelte';
  import Notice from '../lib/components/Notice.svelte';
  import StateBlock from '../lib/components/StateBlock.svelte';
  import { Resource } from '../lib/resource.svelte';
  import { events } from '../lib/events.svelte';
  import { clearSetting, messageFor, session, setPassword, setSetting, settings } from '../lib/api';
  import type { KeyDescription, Session, SettingsReport } from '../lib/api';
  import { navigation } from '../lib/navigation.svelte';
  import { withQuery } from '../lib/router';
  import {
    chooseMediaRoot,
    desktopReport,
    inDesktopShell,
    setAutostart,
    setNotifications,
  } from '../lib/api/desktop';
  import type { DesktopReport } from '../lib/api/desktop';
  import { label, m } from '../lib/i18n';

  interface Props {
    query: Record<string, string>;
  }

  const { query }: Props = $props();

  const report = new Resource<SettingsReport>();
  let drafts = $state<Record<string, string>>({});
  let busy = $state<string | null>(null);
  let notice = $state<{ tone: 'ok' | 'err' | 'info'; text: string } | null>(null);

  // The desktop panel: present only inside the shell, and driven entirely by
  // what the shell reports about the machine it is on.
  let desktop = $state<DesktopReport | null>(null);
  let desktopBusy = $state(false);
  let desktopError = $state<string | null>(null);

  $effect(() => {
    if (!inDesktopShell()) {
      return;
    }
    void desktopReport()
      .then((report) => {
        desktop = report;
      })
      .catch(() => {
        // Present but not answering: the panel simply stays away.
      });
  });

  async function desktopChange(act: () => Promise<DesktopReport>): Promise<void> {
    desktopBusy = true;
    desktopError = null;
    try {
      desktop = await act();
    } catch (error) {
      desktopError = messageFor(error);
    } finally {
      desktopBusy = false;
    }
  }

  // The two authentication knobs are deployment settings, not `SETTINGS` rows,
  // so they never appear above. The password is the one credential this view
  // can act on, and it lives here rather than in a forced wizard because
  // authentication is optional on loopback.
  let auth = $state<Session | null>(null);
  let current = $state('');
  let next = $state('');
  let confirm = $state('');
  let savingPassword = $state(false);

  async function readSession(): Promise<void> {
    try {
      auth = await session();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    }
  }

  async function savePassword(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (next !== confirm) {
      notice = { tone: 'err', text: m.settings.password.mismatch };
      return;
    }
    savingPassword = true;
    notice = null;
    try {
      await setPassword({ current: current || undefined, next });
      current = '';
      next = '';
      confirm = '';
      notice = {
        tone: 'ok',
        text: m.settings.password.saved,
      };
      await readSession();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      savingPassword = false;
    }
  }

  const filter = $derived((query.q ?? '').trim().toUpperCase());

  function reload(): void {
    void report.load(async (signal) => {
      const body = await settings({ signal });
      drafts = {};
      return body;
    });
  }

  $effect(() => {
    reload();
    void readSession();
    return () => report.cancel();
  });

  $effect(() =>
    events.subscribe((event) => {
      if (event.kind.startsWith('settings.')) {
        reload();
      }
    }),
  );

  const keys = $derived(
    (report.data?.keys ?? []).filter((key) => !filter || key.key.includes(filter)),
  );
  const rejected = $derived(report.data?.rejected ?? []);
  const unused = $derived(report.data?.unused ?? []);

  /** The group a key belongs to, taken from its `UGUISU_<GROUP>_…` name. */
  function group(key: string): string {
    const parts = key.split('_');
    return parts.length > 2 ? (parts[1] ?? 'general') : 'general';
  }

  const grouped = $derived.by(() => {
    const map = new Map<string, KeyDescription[]>();
    for (const key of keys) {
      const name = group(key.key);
      map.set(name, [...(map.get(name) ?? []), key]);
    }
    return [...map.entries()].sort(([a], [b]) => a.localeCompare(b));
  });

  function draftOf(key: KeyDescription): string {
    return drafts[key.key] ?? key.stored ?? key.value;
  }

  /**
   * Whether this key can be written here at all.
   *
   * `pinned` is not the test: the API sets it only when a stored value exists
   * *and* something above it wins, so a key simply exported in the
   * environment arrives with `pinned: false` and a write would still be
   * refused with a conflict. The origin is what decides.
   */
  function editable(key: KeyDescription): boolean {
    return key.persistable && key.origin !== 'env' && key.origin !== 'cli';
  }

  function heldBy(key: KeyDescription): string {
    return key.origin === 'cli' ? m.settings.key.source.cli : m.settings.key.source.env;
  }

  async function save(key: KeyDescription): Promise<void> {
    busy = key.key;
    notice = null;
    try {
      await setSetting(key.key, draftOf(key));
      notice = key.live
        ? { tone: 'ok', text: m.settings.key.saved(key.key) }
        : { tone: 'info', text: m.settings.key.savedOnRestart(key.key) };
      reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
      reload();
    } finally {
      busy = null;
    }
  }

  async function clear(key: KeyDescription): Promise<void> {
    busy = key.key;
    notice = null;
    try {
      const { cleared } = await clearSetting(key.key);
      notice = {
        tone: 'ok',
        text: cleared ? m.settings.key.cleared(key.key) : m.settings.key.nothingStored(key.key),
      };
      reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      busy = null;
    }
  }
</script>

<h1>{m.settings.title}</h1>
<p class="muted">{m.settings.intro}</p>

<form class="row" onsubmit={(event) => event.preventDefault()}>
  <label class="grow">
    <span class="visually-hidden">{m.settings.filter}</span>
    <input
      type="search"
      placeholder={m.settings.filter}
      value={query.q ?? ''}
      oninput={(event) =>
        navigation.go(withQuery(navigation.current, { q: event.currentTarget.value }), {
          replace: true,
        })}
    />
  </label>
</form>

{#if notice}
  <Notice tone={notice.tone}>{notice.text}</Notice>
{/if}

{#if desktop}
  <section class="card">
    <h2>{m.settings.desktop.title}</h2>
    <p class="small muted">{m.settings.desktop.intro}</p>

    <div class="row setting">
      <div>
        <strong>{m.settings.desktop.mediaRoot}</strong>
        <p class="small muted mono">{desktop.media_root}</p>
        {#if desktop.media_pinned}
          <p class="small muted">
            {m.settings.desktop.mediaRootPinned.before}<code>{'UGUISU_MEDIA_DIR'}</code>{m.settings.desktop.mediaRootPinned.after}
          </p>
        {:else}
          <p class="small muted">{m.settings.desktop.mediaRootHint}</p>
        {/if}
      </div>
      <button
        type="button"
        disabled={desktop.media_pinned || desktopBusy}
        onclick={() => void desktopChange(chooseMediaRoot)}
      >
        {m.settings.desktop.chooseFolder}
      </button>
    </div>

    <label class="row setting">
      <span>
        <strong>{m.settings.desktop.notifications}</strong>
        <span class="small muted">{m.settings.desktop.notificationsHint}</span>
      </span>
      <input
        type="checkbox"
        checked={desktop.notifications}
        disabled={desktopBusy}
        onchange={(event) =>
          void desktopChange(() => setNotifications(event.currentTarget.checked))}
      />
    </label>

    <label class="row setting">
      <span>
        <strong>{m.settings.desktop.autostart}</strong>
        <span class="small muted">
          {desktop.autostart_supported
            ? m.settings.desktop.autostartHint
            : m.settings.desktop.autostartUnsupported}
        </span>
      </span>
      <input
        type="checkbox"
        checked={desktop.autostart}
        disabled={desktopBusy || !desktop.autostart_supported}
        onchange={(event) => void desktopChange(() => setAutostart(event.currentTarget.checked))}
      />
    </label>

    {#if desktopError}
      <Notice tone="err">{desktopError}</Notice>
    {/if}
  </section>
{/if}

{#if auth}
  <section class="card">
    <h2>{auth.credential_set ? m.settings.password.title : m.settings.password.titleUnset}</h2>
    <p class="small muted">
      {#if auth.credential_set}
        {m.settings.password.changeHint.before}<code>{auth.username}</code>{m.settings.password.changeHint.after}
      {:else}
        {m.settings.password.unsetHint}
      {/if}
    </p>
    <form class="password" onsubmit={savePassword}>
      {#if auth.credential_set}
        <label for="current-password">{m.settings.password.current}</label>
        <input
          id="current-password"
          type="password"
          autocomplete="current-password"
          bind:value={current}
          required
        />
      {/if}
      <label for="new-password">{m.settings.password.next}</label>
      <input
        id="new-password"
        type="password"
        autocomplete="new-password"
        bind:value={next}
        required
      />
      <label for="confirm-password">{m.settings.password.confirm}</label>
      <input
        id="confirm-password"
        type="password"
        autocomplete="new-password"
        bind:value={confirm}
        required
      />
      <button type="submit" disabled={savingPassword || next === ''}>
        {savingPassword
          ? m.settings.saving
          : auth.credential_set
            ? m.settings.password.change
            : m.settings.password.set}
      </button>
    </form>
  </section>
  {#if auth.credential_set}
    <ApiTokens />
  {/if}
{/if}

{#if report.state === 'error'}
  <StateBlock state="error" title={m.settings.unreadable} error={report.error} onretry={reload} />
{:else if report.data === null}
  <StateBlock state="loading" />
{:else}
  {#if rejected.length > 0}
    <section class="card problem">
      <h2>{m.settings.rejected.title}</h2>
      <p class="small muted">{m.settings.rejected.hint}</p>
      <ul>
        {#each rejected as bad (bad.key)}
          <li>
            <code>{bad.key}</code> = <code>{bad.value}</code>
            <p class="small err">{bad.message}</p>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if unused.length > 0}
    <section class="card problem">
      <h2>{m.settings.unused.title}</h2>
      <ul>
        {#each unused as entry (entry.key)}
          <li>
            <code>{entry.key}</code> = <code>{entry.value}</code>
            <span class="small muted">— {entry.reason}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if keys.length === 0}
    <StateBlock state="empty" title={m.settings.noMatch} />
  {/if}

  {#each grouped as [name, entries] (name)}
    <section class="card group">
      <h2>{label(name.toLowerCase())}</h2>
      <ul class="keys">
        {#each entries as key (key.key)}
          <li>
            <div class="key">
              <code>{key.key}</code>
              <span class="row small tags">
                <Badge value={key.origin} tone={key.origin === 'default' ? 'neutral' : 'ok'} />
                {#if !editable(key) && key.persistable}
                  <Badge value={m.settings.key.pinnedBy(heldBy(key))} tone="warn" />
                {/if}
                {#if !key.persistable}
                  <Badge value={m.settings.key.environmentOnly} tone="neutral" />
                {/if}
                {#if !key.live}
                  <Badge value={m.settings.key.restartRequired} tone="warn" />
                {/if}
              </span>
            </div>
            {#if key.origin === 'env' || key.origin === 'cli'}
              <p class="small muted">{m.settings.key.inForce(heldBy(key))}</p>
              <p class="value"><code>{key.value}</code></p>
              {#if key.stored !== null}
                <p class="small warn">
                  {m.settings.key.storedIgnored.before}<code>{key.stored}</code>{m.settings.key.storedIgnored.after}
                </p>
              {/if}
            {:else if !key.persistable}
              <p class="small muted">{m.settings.key.environmentOnlyHint}</p>
              <p class="value"><code>{key.value}</code></p>
            {:else}
              <div class="row edit">
                <label>
                  <span class="visually-hidden">{m.settings.key.value(key.key)}</span>
                  <input
                    value={draftOf(key)}
                    oninput={(event) =>
                      (drafts = { ...drafts, [key.key]: event.currentTarget.value })}
                  />
                </label>
                <button
                  type="button"
                  class="primary"
                  disabled={busy !== null || draftOf(key) === (key.stored ?? key.value)}
                  onclick={() => void save(key)}
                >
                  {busy === key.key ? m.settings.saving : m.settings.key.save}
                </button>
                {#if key.stored !== null}
                  <button type="button" disabled={busy !== null} onclick={() => void clear(key)}>
                    {m.settings.key.useDefault}
                  </button>
                {/if}
              </div>
              {#if key.stored !== null && key.stored !== key.value}
                <p class="small warn">
                  {m.settings.key.storedOverridden.before}<code>{key.stored}</code>{m.settings.key.storedOverridden.between}<code>{key.value}</code>{m.settings.key.storedOverridden.after}
                </p>
              {/if}
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/each}
{/if}

<style>
  form {
    margin-bottom: var(--gap);
  }

  .grow {
    flex: 1 1 16rem;
  }

  .grow input {
    width: 100%;
  }

  .group,
  .problem {
    margin-bottom: var(--gap);
  }

  .password {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    max-width: 20rem;
  }

  .password button {
    margin-top: 0.5rem;
    align-self: flex-start;
  }

  .problem {
    border-left: 3px solid var(--warn);
  }

  h2 {
    margin-bottom: 0.5rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .keys li {
    padding: 0.6rem 0;
    border-bottom: 1px solid var(--border);
  }

  .keys li:last-child {
    border-bottom: none;
  }

  .key {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 0.3rem;
  }

  .tags {
    gap: 0.3rem;
  }

  p {
    margin: 0 0 0.3rem;
  }

  .edit input {
    min-width: 14rem;
  }

  .err {
    color: var(--err);
  }

  .warn {
    color: var(--warn);
  }
</style>
