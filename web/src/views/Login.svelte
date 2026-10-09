<!--
  The login form, shown by the shell instead of a view when the server needs a
  credential and this request has none.

  The password is in the form and nowhere else: no URL, no `localStorage`, no
  query parameter. What comes back is a cookie the browser holds and a CSRF
  token the client keeps in memory.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import Notice from '../lib/components/Notice.svelte';
  import { ApiFailure, login, messageFor } from '../lib/api';
  import { m } from '../lib/i18n';

  interface Props {
    /** Called once a session exists, so the shell can re-read it. */
    onauthenticated: () => void;
    /** The operator's name, when the server was willing to say it. */
    username?: string | null;
  }

  const { onauthenticated, username = null }: Props = $props();

  // Seeded once from the server's answer; after that the field belongs to
  // whoever is typing in it.
  let name = $state(untrack(() => username) ?? 'uguisu');
  let password = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  let waitSeconds = $state<number | null>(null);

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    busy = true;
    error = null;
    waitSeconds = null;
    try {
      await login(name, password);
      password = '';
      onauthenticated();
    } catch (cause) {
      error = messageFor(cause);
      if (cause instanceof ApiFailure) {
        waitSeconds = cause.retryAfterSeconds;
      }
    } finally {
      busy = false;
    }
  }
</script>

<section class="login">
  <h1>{m.login.title}</h1>
  <p class="muted small">{m.login.intro}</p>

  <form onsubmit={submit}>
    <label for="username">{m.login.user}</label>
    <input id="username" name="username" autocomplete="username" bind:value={name} required />

    <label for="password">{m.login.password}</label>
    <!-- svelte-ignore a11y_autofocus -->
    <input
      id="password"
      name="password"
      type="password"
      autocomplete="current-password"
      bind:value={password}
      required
      autofocus
    />

    <button type="submit" disabled={busy || password === ''}>
      {busy ? m.login.signingIn : m.login.submit}
    </button>
  </form>

  {#if error}
    <Notice tone="err">
      {error}{#if waitSeconds !== null}
        {m.login.retryAfter(waitSeconds)}
      {/if}
    </Notice>
  {/if}
</section>

<style>
  .login {
    max-width: 22rem;
    margin: 3rem auto;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  h1 {
    margin: 0;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  label {
    font-size: 0.9rem;
    color: var(--muted);
  }

  input {
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--fg);
  }

  button {
    margin-top: 0.5rem;
  }
</style>
