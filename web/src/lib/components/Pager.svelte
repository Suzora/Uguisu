<!-- Cursor paging: the API hands out a `next_after`, never a page number. -->
<script lang="ts">
  import { m } from '../i18n';

  interface Props {
    /** How many rows are on screen. */
    shown: number;
    /** Whether the API offered another page. */
    more: boolean;
    busy?: boolean;
    onmore: () => void;
  }

  const { shown, more, busy = false, onmore }: Props = $props();
</script>

<div class="row pager">
  <span class="muted small">{m.common.pager.shown(shown)}</span>
  {#if more}
    <button type="button" onclick={onmore} disabled={busy}>
      {busy ? m.common.state.loading : m.common.pager.more}
    </button>
  {/if}
</div>

<style>
  .pager {
    justify-content: space-between;
    padding-top: 0.5rem;
  }
</style>
