<script lang="ts">
  import { navigate } from '../navigation.svelte';

  interface Props {
    href: string;
    class?: string;
    title?: string;
    current?: boolean;
    children: import('svelte').Snippet;
  }

  const { href, class: className = '', title, current = false, children }: Props = $props();
</script>

<a
  {href}
  {title}
  class={className}
  aria-current={current ? 'page' : undefined}
  onclick={(event) => navigate(event, href)}
>
  {@render children()}
</a>
