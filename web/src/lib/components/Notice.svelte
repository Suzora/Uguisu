<!--
  The result of a mutation, announced to assistive technology.

  Every action that changes server state ends here: the UI never leaves a
  button looking successful when the server refused it.
-->
<script lang="ts">
  interface Props {
    tone: 'ok' | 'err' | 'info';
    children: import('svelte').Snippet;
  }

  const { tone, children }: Props = $props();
</script>

<p class="notice" data-tone={tone} role={tone === 'err' ? 'alert' : 'status'}>
  {@render children()}
</p>

<style>
  .notice {
    margin: 0;
    padding: 0.5rem 0.75rem;
    border-radius: var(--radius);
    border-left: 3px solid currentcolor;
    background: var(--surface);
    font-size: 0.9rem;
  }

  .notice[data-tone='ok'] {
    color: var(--ok);
  }

  .notice[data-tone='err'] {
    color: var(--err);
  }

  .notice[data-tone='info'] {
    color: var(--muted);
  }
</style>
