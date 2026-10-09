<!-- A state word in the colour its meaning deserves. -->
<script lang="ts">
  import { label } from '../i18n';

  interface Props {
    value: string | null | undefined;
    /** Overrides the tone the value would otherwise map to. */
    tone?: 'neutral' | 'ok' | 'warn' | 'err' | 'busy';
    title?: string;
  }

  const { value, tone, title }: Props = $props();

  const TONES: Record<string, Props['tone']> = {
    active: 'ok',
    archived: 'ok',
    verified: 'ok',
    completed: 'ok',
    ready: 'ok',
    written: 'ok',
    fetched: 'ok',
    downloading: 'busy',
    finalizing: 'busy',
    queued: 'busy',
    building: 'busy',
    retrying: 'warn',
    paused: 'warn',
    stale: 'warn',
    unchecked: 'warn',
    modified: 'warn',
    skipped: 'neutral',
    expected: 'neutral',
    cancelled: 'neutral',
    untagged: 'neutral',
    error: 'err',
    failed: 'err',
    missing: 'err',
    invalid: 'err',
  };

  const resolved = $derived(tone ?? TONES[value ?? ''] ?? 'neutral');
</script>

<span class="badge" data-tone={resolved} {title}>{label(value)}</span>

<style>
  .badge {
    display: inline-block;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
    border: 1px solid currentcolor;
    font-size: 0.78rem;
    line-height: 1.5;
    white-space: nowrap;
  }

  .badge[data-tone='neutral'] {
    color: var(--muted);
  }

  .badge[data-tone='ok'] {
    color: var(--ok);
  }

  .badge[data-tone='warn'] {
    color: var(--warn);
  }

  .badge[data-tone='err'] {
    color: var(--err);
  }

  .badge[data-tone='busy'] {
    color: var(--accent);
  }
</style>
