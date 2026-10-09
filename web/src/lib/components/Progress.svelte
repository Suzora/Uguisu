<!-- A determinate bar when the total is known, an indeterminate one when not. -->
<script lang="ts">
  import { m } from '../i18n';

  interface Props {
    done: number;
    total: number | null | undefined;
    label: string;
  }

  const { done, total, label }: Props = $props();
  const share = $derived(total && total > 0 ? Math.min(1, done / total) : null);
</script>

{#if share === null}
  <div class="bar unknown" role="progressbar" aria-label={label} aria-valuetext={m.common.progress.sizeUnknown}></div>
{:else}
  <div
    class="bar"
    role="progressbar"
    aria-label={label}
    aria-valuenow={Math.round(share * 100)}
    aria-valuemin="0"
    aria-valuemax="100"
  >
    <div class="fill" style:width="{share * 100}%"></div>
  </div>
{/if}

<style>
  .bar {
    height: 0.4rem;
    border-radius: 999px;
    background: var(--border);
    overflow: hidden;
    min-width: 4rem;
  }

  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 200ms linear;
  }

  .unknown {
    background: repeating-linear-gradient(
      90deg,
      var(--border) 0 0.5rem,
      transparent 0.5rem 1rem
    );
  }
</style>
