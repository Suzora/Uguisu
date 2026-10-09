<!--
  The playback bar.

  Native `<audio controls>` supplies play/pause, seeking, volume, duration
  and keyboard handling, and a browser's own controls are already accessible
  and usable on a phone. What is added here is the episode it belongs to and
  an honest message when the media cannot be served.
-->
<script lang="ts">
  import { m } from '../i18n';
  import { player } from '../player.svelte';

  let failed = $state(false);
  const now = $derived(player.current);

  $effect(() => {
    void now?.episodeId;
    failed = false;
  });
</script>

{#if now}
  <div class="player" role="region" aria-label={m.common.player.region}>
    <div class="meta">
      <p class="title">{now.title}</p>
      <p class="muted small">{now.podcastTitle}</p>
    </div>
    {#if failed}
      <p class="error small" role="alert">{m.common.player.failed}</p>
    {:else}
      <!-- svelte-ignore a11y_media_has_caption -->
      <audio src={now.src} controls autoplay preload="metadata" onerror={() => (failed = true)}
      ></audio>
    {/if}
    <button type="button" onclick={() => player.close()} aria-label={m.common.player.close}>✕</button>
  </div>
{/if}

<style>
  .player {
    position: sticky;
    bottom: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
    align-items: center;
    padding: 0.6rem 1rem;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }

  .meta {
    flex: 1 1 12rem;
    min-width: 0;
  }

  .title {
    margin: 0;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  p {
    margin: 0;
  }

  audio {
    flex: 2 1 18rem;
    min-width: 0;
    max-width: 100%;
  }

  .error {
    flex: 2 1 18rem;
    color: var(--err);
  }
</style>
