<script lang="ts">
  import Link from './Link.svelte';
  import { navigation } from '../navigation.svelte';
  import { m } from '../i18n';
  import { sectionOf, type RouteName } from '../router';

  interface Item {
    name: RouteName;
    href: string;
    label: string;
  }

  const ITEMS: Item[] = [
    { name: 'dashboard', href: '/', label: m.app.nav.dashboard },
    { name: 'library', href: '/podcasts', label: m.app.nav.library },
    { name: 'downloads', href: '/downloads', label: m.app.nav.downloads },
    { name: 'archive', href: '/archive', label: m.app.nav.archive },
    { name: 'discover', href: '/discover', label: m.app.nav.discover },
    { name: 'search', href: '/search', label: m.app.nav.search },
    { name: 'settings', href: '/settings', label: m.app.nav.settings },
    { name: 'service', href: '/service', label: m.app.nav.service },
  ];

  const active = $derived(sectionOf(navigation.current.route));
</script>

<nav aria-label={m.app.nav.label}>
  <ul>
    {#each ITEMS as item (item.name)}
      <li>
        <Link href={item.href} current={active === item.name}>{item.label}</Link>
      </li>
    {/each}
  </ul>
</nav>

<style>
  nav {
    overflow-x: auto;
    scrollbar-width: thin;
  }

  ul {
    display: flex;
    gap: 0.25rem;
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li :global(a) {
    display: block;
    padding: 0.45rem 0.7rem;
    border-radius: var(--radius);
    color: var(--muted);
    text-decoration: none;
    white-space: nowrap;
  }

  li :global(a:hover) {
    color: var(--fg);
    background: var(--bg);
  }

  li :global(a[aria-current='page']) {
    color: var(--fg);
    background: var(--bg);
    font-weight: 600;
    box-shadow: inset 0 -2px 0 var(--accent);
  }
</style>
