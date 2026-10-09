<!--
  Podcast artwork.

  Uguisu's own copy is preferred: it is served from this origin, so opening
  the UI does not tell every publisher's CDN that this library exists. The
  feed's remote URL is the fallback, and a missing image degrades to the
  podcast's initials rather than a broken-image icon.
-->
<script lang="ts">
  interface Props {
    // The API's optional URLs are absent rather than null, so both arrive here.
    src: string | null | undefined;
    title: string;
    size?: number;
  }

  const { src, title, size = 64 }: Props = $props();
  let broken = $state(false);

  const initials = $derived(
    title
      .split(/\s+/)
      .filter((word) => word.length > 0)
      .slice(0, 2)
      .map((word) => word[0]?.toUpperCase() ?? '')
      .join(''),
  );

  $effect(() => {
    void src;
    broken = false;
  });
</script>

{#if src && !broken}
  <img
    {src}
    alt=""
    width={size}
    height={size}
    loading="lazy"
    decoding="async"
    onerror={() => (broken = true)}
  />
{:else}
  <span class="placeholder" style:width="{size}px" style:height="{size}px" aria-hidden="true">
    {initials || '—'}
  </span>
{/if}

<style>
  img,
  .placeholder {
    border-radius: var(--radius);
    object-fit: cover;
    background: var(--border);
    flex: none;
  }

  .placeholder {
    display: grid;
    place-items: center;
    color: var(--muted);
    font-weight: 600;
  }
</style>
