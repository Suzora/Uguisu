<!--
  The one loading / empty / error block the whole UI uses.

  It exists so that "nothing here" and "we could not find out" never look the
  same: `empty` is a fact the server reported, `error` is a failure with a way
  to retry, and `loading` says a request is still open.
-->
<script lang="ts">
  import { ApiFailure, messageFor } from '../api';
  import { m } from '../i18n';

  interface Props {
    state: 'loading' | 'empty' | 'error';
    /** Headline; a sensible default per state when omitted. */
    title?: string;
    /** The failure, when `state` is `error`. */
    error?: unknown;
    /** What the reader can do about it. */
    hint?: string;
    onretry?: () => void;
  }

  const { state, title, error, hint, onretry }: Props = $props();

  const failure = $derived(error instanceof ApiFailure ? error : null);
  const heading = $derived(
    title ??
      (state === 'loading' ? m.common.state.loading : state === 'empty' ? m.common.state.empty : m.common.state.failed),
  );
  const explanation = $derived.by(() => {
    if (state !== 'error') {
      return hint ?? null;
    }
    if (failure?.failure === 'network') {
      return m.common.state.network;
    }
    if (failure?.failure === 'unavailable') {
      return messageFor(error);
    }
    if (failure?.failure === 'timeout') {
      return m.common.state.timeout;
    }
    return messageFor(error);
  });
</script>

<div class="block" data-state={state} role={state === 'error' ? 'alert' : 'status'}>
  <p class="heading">{heading}</p>
  {#if explanation}
    <p class="muted small">{explanation}</p>
  {/if}
  {#if failure?.kind}
    <p class="muted small"><code>{failure.kind}</code>{#if failure.status}{m.common.state.httpStatus(failure.status)}{/if}</p>
  {/if}
  {#if onretry && state === 'error'}
    <button type="button" onclick={onretry}>{m.common.state.retry}</button>
  {/if}
</div>

<style>
  .block {
    padding: 1.5rem 1rem;
    text-align: center;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
  }

  .block[data-state='error'] {
    border-style: solid;
    border-color: var(--err);
  }

  .heading {
    margin: 0 0 0.25rem;
    font-weight: 600;
  }

  p {
    margin: 0 0 0.5rem;
  }

  button {
    margin-top: 0.25rem;
  }
</style>
