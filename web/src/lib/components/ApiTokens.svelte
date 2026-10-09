<!--
  The API tokens: list, create, revoke (ADR 0035).

  A new token's secret is shown once, in this component's state and nowhere
  else: not in the URL, not in storage. Leaving the page forgets it.
-->
<script lang="ts">
  import Badge from './Badge.svelte';
  import Notice from './Notice.svelte';
  import StateBlock from './StateBlock.svelte';
  import { Resource } from '../resource.svelte';
  import { createToken, listTokens, messageFor, revokeToken } from '../api';
  import type { ApiTokenRecord, TokenScope } from '../api';
  import { dateTime, relative } from '../format';
  import { m } from '../i18n';

  const tokens = new Resource<ApiTokenRecord[]>();
  let notice = $state<{ tone: 'ok' | 'err'; text: string } | null>(null);
  let name = $state('');
  let scope = $state<TokenScope>('read');
  let creating = $state(false);
  let secret = $state<string | null>(null);
  let copied = $state(false);
  let confirming = $state<string | null>(null);
  let revoking = $state<string | null>(null);

  function reload(): void {
    void tokens.load((signal) => listTokens({ signal }));
  }

  $effect(() => {
    reload();
    return () => tokens.cancel();
  });

  function standing(token: ApiTokenRecord): 'active' | 'revoked' | 'expired' {
    if (token.revoked_at) {
      return 'revoked';
    }
    return token.expires_at && new Date(token.expires_at).getTime() <= Date.now() ? 'expired' : 'active';
  }

  async function create(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    creating = true;
    notice = null;
    secret = null;
    copied = false;
    try {
      const made = await createToken({ name: name.trim(), scope });
      secret = made.secret;
      notice = { tone: 'ok', text: m.settings.tokens.createdNotice(made.token.name) };
      name = '';
      reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      creating = false;
    }
  }

  async function revoke(token: ApiTokenRecord): Promise<void> {
    confirming = null;
    revoking = token.id;
    notice = null;
    try {
      await revokeToken(token.id);
      notice = { tone: 'ok', text: m.settings.tokens.revokedNotice(token.name) };
      reload();
    } catch (error) {
      notice = { tone: 'err', text: messageFor(error) };
    } finally {
      revoking = null;
    }
  }

  async function copy(): Promise<void> {
    if (secret) {
      await navigator.clipboard.writeText(secret);
      copied = true;
    }
  }
</script>

<section class="card">
  <h2>{m.settings.tokens.title}</h2>
  <p class="small muted">{m.settings.tokens.intro}</p>

  {#if notice}
    <Notice tone={notice.tone}>{notice.text}</Notice>
  {/if}
  {#if secret}
    <div class="row secret">
      <label for="new-token">{m.settings.tokens.secret}</label>
      <input id="new-token" type="text" readonly value={secret} onfocus={(e) => e.currentTarget.select()} />
      {#if navigator.clipboard}
        <button type="button" onclick={() => void copy()}>
          {copied ? m.settings.tokens.copied : m.settings.tokens.copy}
        </button>
      {/if}
    </div>
  {/if}

  {#if tokens.state === 'error'}
    <StateBlock state="error" title={m.settings.tokens.unreadable} error={tokens.error} onretry={reload} />
  {:else if tokens.data === null}
    <StateBlock state="loading" />
  {:else if tokens.data.length === 0}
    <p class="small muted">{m.settings.tokens.none}</p>
  {:else}
    <table>
      <caption class="visually-hidden">{m.settings.tokens.caption}</caption>
      <thead>
        <tr>
          <th scope="col">{m.settings.tokens.columns.name}</th>
          <th scope="col">{m.settings.tokens.columns.scope}</th>
          <th scope="col">{m.settings.tokens.columns.created}</th>
          <th scope="col">{m.settings.tokens.columns.lastUsed}</th>
          <th scope="col">{m.settings.tokens.columns.state}</th>
          <th scope="col"><span class="visually-hidden">{m.settings.tokens.columns.actions}</span></th>
        </tr>
      </thead>
      <tbody>
        {#each tokens.data as token (token.id)}
          {@const now = standing(token)}
          <tr>
            <td>{token.name}</td>
            <td>{token.scope}</td>
            <td title={dateTime(token.created_at)}>{relative(token.created_at)}</td>
            <td title={dateTime(token.last_used_at)}>
              {token.last_used_at ? relative(token.last_used_at) : m.settings.tokens.never}
            </td>
            <td>
              <Badge
                value={m.settings.tokens[now]}
                tone={now === 'active' ? 'ok' : 'neutral'}
              />
              {#if now === 'active' && token.expires_at}
                <span class="small muted">{m.settings.tokens.until(dateTime(token.expires_at))}</span>
              {/if}
            </td>
            <td class="actions">
              {#if now === 'active' && confirming === token.id}
                <span class="small">{m.settings.tokens.confirmRevoke(token.name)}</span>
                <button type="button" disabled={revoking !== null} onclick={() => void revoke(token)}>
                  {m.settings.tokens.revoke}
                </button>
                <button type="button" onclick={() => (confirming = null)}>{m.settings.tokens.cancel}</button>
              {:else if now === 'active'}
                <button type="button" disabled={revoking !== null} onclick={() => (confirming = token.id)}>
                  {m.settings.tokens.revoke}
                </button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  <form class="row create" onsubmit={create}>
    <label for="token-name">{m.settings.tokens.name}</label>
    <input id="token-name" type="text" bind:value={name} required maxlength="64" />
    <label for="token-scope">{m.settings.tokens.scope}</label>
    <select id="token-scope" bind:value={scope}>
      <option value="read">{m.settings.tokens.scopes.read}</option>
      <option value="write">{m.settings.tokens.scopes.write}</option>
    </select>
    <button type="submit" disabled={creating || name.trim() === ''}>
      {creating ? m.settings.tokens.creating : m.settings.tokens.create}
    </button>
  </form>
</section>

<style>
  table {
    width: 100%;
    margin-bottom: 0.75rem;
  }

  .secret,
  .create {
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 0.75rem;
  }

  .secret input {
    flex: 1;
    min-width: 16rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }

  .actions {
    white-space: nowrap;
  }
</style>
