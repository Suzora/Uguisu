<!--
  The application shell: identity, primary navigation, the connection
  indicator, one view, and the playback bar.

  The shell owns the two things that must outlive a view — the SSE connection
  and the player — and nothing else.
-->
<script lang="ts">
  import Nav from './lib/components/Nav.svelte';
  import Player from './lib/components/Player.svelte';
  import Link from './lib/components/Link.svelte';
  import { events } from './lib/events.svelte';
  import { navigation } from './lib/navigation.svelte';
  import { health, logout, messageFor, onUnauthorized, session } from './lib/api';
  import { bootstrapSession } from './lib/api/desktop';
  import * as desktopNotifications from './lib/desktop-notifications';
  import { locale, m } from './lib/i18n';
  import type { Session } from './lib/api';

  import Dashboard from './views/Dashboard.svelte';
  import Library from './views/Library.svelte';
  import PodcastDetail from './views/PodcastDetail.svelte';
  import Episode from './views/Episode.svelte';
  import Discover from './views/Discover.svelte';
  import Search from './views/Search.svelte';
  import Downloads from './views/Downloads.svelte';
  import Archive from './views/Archive.svelte';
  import ArchiveImport from './views/ArchiveImport.svelte';
  import ArchiveRepair from './views/ArchiveRepair.svelte';
  import Settings from './views/Settings.svelte';
  import Service from './views/Service.svelte';
  import Login from './views/Login.svelte';

  const route = $derived(navigation.current.route);
  const query = $derived(navigation.current.query);
  const connection = $derived(events.state);

  let main = $state<HTMLElement | null>(null);
  let announcement = $state('');
  let version = $state<string | null>(null);
  let reachable = $state<boolean | null>(null);
  let reachError = $state<string | null>(null);
  let auth = $state<Session | null>(null);
  let signingOut = $state(false);

  // `null` is "not asked yet": the shell shows neither the views nor the login
  // form until the server has said which of the two this request may see.
  const locked = $derived(auth !== null && auth.auth_required && !auth.authenticated);

  async function readSession(): Promise<void> {
    try {
      let current = await session();
      // Inside the desktop shell the first read is unauthenticated by design:
      // the shell holds a per-launch credential that buys exactly one session
      // (ADR 0042). In a browser this is a no-op and the login view follows.
      if (current.auth_required && !current.authenticated && (await bootstrapSession())) {
        current = await session();
      }
      auth = current;
    } catch {
      // A server that cannot answer the public session route is already
      // reported by the health check below; the shell stays as it was.
    }
  }

  $effect(() => {
    void readSession();
    // A session that expires mid-visit shows up as a 401 on whatever the view
    // asked for next, not as a navigation, so the shell re-reads it there.
    onUnauthorized(() => void readSession());
    return () => onUnauthorized(null);
  });

  // The event stream is authenticated like every other read, so it waits until
  // there is something to authenticate with.
  $effect(() => {
    if (locked) {
      return;
    }
    events.start();
    // Rides the same stream rather than opening a second one, and is a no-op
    // outside the desktop shell.
    const quiet = desktopNotifications.start();
    return () => {
      quiet();
      events.stop();
    };
  });

  $effect(() => {
    document.documentElement.lang = locale;
  });

  $effect(() => {
    document.title = m.app.documentTitle(m.app.titles[route.name]);
  });

  // A client-side navigation changes no focus by itself, so a reader using a
  // screen reader or the keyboard would not learn the page changed. Moving
  // focus to the content and naming the page in a live region does both.
  $effect(() => {
    if (navigation.visit === 0) {
      return;
    }
    announcement = m.app.announcePage(m.app.titles[navigation.current.route.name]);
    main?.focus();
  });

  $effect(() => {
    let cancelled = false;
    health()
      .then((body) => {
        if (!cancelled) {
          version = body.version;
          reachable = true;
          reachError = null;
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          reachable = false;
          reachError = messageFor(error);
        }
      });
    return () => {
      cancelled = true;
    };
  });
</script>

<a class="skip" href="#main">{m.app.skip}</a>

<header>
  <div class="bar">
    <Link href="/" class="brand">{m.app.name}</Link>
    {#if !locked}
      <Nav />
    {/if}
    <p class="connection small" data-state={connection}>
      <span class="dot" aria-hidden="true"></span>
      <span class="visually-hidden">{m.app.connection.label}</span>
      {connection === 'open'
        ? m.app.connection.open
        : connection === 'connecting'
          ? m.app.connection.connecting
          : m.app.connection.reconnecting}
    </p>
    {#if auth?.authenticated}
      <button
        type="button"
        class="signout small"
        disabled={signingOut}
        onclick={async () => {
          signingOut = true;
          try {
            await logout();
            await readSession();
          } finally {
            signingOut = false;
          }
        }}
      >
        {m.app.signOut(auth.username ?? null)}
      </button>
    {/if}
  </div>
  {#if reachable === false}
    <p class="offline" role="alert">{m.app.offline(reachError ?? '')}</p>
  {/if}
</header>

<main id="main" tabindex="-1" bind:this={main}>
  {#if locked}
    <Login username={auth?.username ?? null} onauthenticated={() => void readSession()} />
  {:else if route.name === 'dashboard'}
    <Dashboard />
  {:else if route.name === 'library'}
    <Library {query} />
  {:else if route.name === 'podcast'}
    {#key route.id}
      <PodcastDetail id={route.id} {query} />
    {/key}
  {:else if route.name === 'episode'}
    {#key route.id}
      <Episode id={route.id} />
    {/key}
  {:else if route.name === 'discover'}
    <Discover {query} />
  {:else if route.name === 'search'}
    <Search {query} />
  {:else if route.name === 'downloads'}
    <Downloads {query} />
  {:else if route.name === 'archive'}
    <Archive {query} />
  {:else if route.name === 'archiveImport'}
    <ArchiveImport />
  {:else if route.name === 'archiveRepair'}
    <ArchiveRepair {query} />
  {:else if route.name === 'settings'}
    <Settings {query} />
  {:else if route.name === 'service'}
    <Service />
  {:else}
    <h1>{m.app.notFound.title}</h1>
    <p class="muted">
      {m.app.notFound.before}<code>{route.path}</code>{m.app.notFound.between}<Link href="/"
        >{m.app.notFound.back}</Link
      >{m.app.notFound.end}
    </p>
  {/if}
</main>

<p class="visually-hidden" role="status" aria-live="polite">{announcement}</p>

<footer>
  <p class="muted small">{m.app.footer(version ?? '')}</p>
</footer>

<Player />

<style>
  .skip {
    position: absolute;
    left: -999px;
  }

  .skip:focus {
    left: 0.5rem;
    top: 0.5rem;
    z-index: 10;
    padding: 0.5rem 0.75rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  header {
    position: sticky;
    top: 0;
    z-index: 5;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    max-width: 72rem;
    margin: 0 auto;
    padding: 0.5rem 1rem;
  }

  .bar :global(.brand) {
    font-weight: 700;
    font-size: 1.1rem;
    color: var(--fg);
    text-decoration: none;
  }

  .connection {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    margin: 0 0 0 auto;
    color: var(--muted);
  }

  .dot {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--muted);
  }

  .connection[data-state='open'] .dot {
    background: var(--ok);
  }

  .connection[data-state='retrying'] .dot {
    background: var(--warn);
  }

  .signout {
    padding: 0.25rem 0.5rem;
  }

  .offline {
    margin: 0;
    padding: 0.5rem 1rem;
    background: var(--err);
    color: #fff;
    font-size: 0.9rem;
  }

  main {
    max-width: 72rem;
    margin: 0 auto;
    padding: 1.25rem 1rem 2rem;
  }

  main:focus {
    outline: none;
  }

  footer {
    max-width: 72rem;
    margin: 0 auto;
    padding: 0 1rem 2rem;
  }

  @media (width <= 40rem) {
    .connection {
      margin-left: 0;
    }
  }
</style>
