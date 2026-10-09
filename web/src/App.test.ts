import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import App from './App.svelte';
import { EMPTY, stubApi, stubEventSource } from '../src/tests/harness';
import { events } from './lib/events.svelte';
import { navigation } from './lib/navigation.svelte';

const ROUTES = {
  '/api/v1/health': { body: { status: 'ok', version: '0.1.0' } },
  '/api/v1/status': { offline: true },
  '/api/v1/archive/stats': { body: EMPTY.archiveStats },
  '/api/v1/events': { body: EMPTY.events },
  '/api/v1/podcasts': { body: EMPTY.podcasts },
  '/api/v1/downloads': { body: EMPTY.jobs },
  '/api/v1/downloads/stats': { body: EMPTY.downloadStats },
  '/api/v1/archive': { body: { files: [], schema: 1 } },
  '/api/v1/archive/manifests': { body: EMPTY.manifests },
  '/api/v1/settings': { body: { keys: [], rejected: [], unused: [], schema: 1 } },
  '/api/v1/discovery/providers': { body: { providers: [], cache: { entries: 0 } } },
  '/api/v1/search': {
    body: {
      outcome: 'empty_query',
      terms: [],
      truncated: false,
      podcasts: [],
      episodes: [],
      index: { state: 'ready', built_at: null, podcasts: 0, episodes: 0, detail: null, updated_at: '2026-01-01T10:00:00Z' },
      duration_ms: 1,
      schema: 1,
    },
  },
};

describe('application shell', () => {
  beforeEach(() => {
    stubEventSource();
    window.history.replaceState(null, '', '/');
    navigation.go('/');
  });

  afterEach(() => events.stop());

  it('opens a deep link directly', async () => {
    stubApi(ROUTES);
    navigation.go('/downloads?state=failed');
    render(App);

    expect(await screen.findByRole('heading', { level: 1, name: 'Downloads' })).toBeInTheDocument();
    expect(await screen.findByText('No job is failed')).toBeInTheDocument();
    expect(document.title).toBe('Downloads · Uguisu');
  });

  it('marks the current section in the navigation', async () => {
    stubApi(ROUTES);
    navigation.go('/settings');
    render(App);

    const current = await screen.findByRole('link', { name: 'Settings' });
    expect(current).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('link', { name: 'Dashboard' })).not.toHaveAttribute('aria-current');
  });

  it('follows an in-app link without a reload', async () => {
    stubApi(ROUTES);
    render(App);
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });

    screen.getByRole('link', { name: 'Archive' }).click();
    expect(await screen.findByRole('heading', { level: 1, name: 'Archive' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/archive');
  });

  it('restores the previous view on back', async () => {
    stubApi(ROUTES);
    render(App);
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });

    screen.getByRole('link', { name: 'Archive' }).click();
    await screen.findByRole('heading', { level: 1, name: 'Archive' });

    window.history.back();
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 1, name: 'Dashboard' })).toBeInTheDocument(),
    );
  });

  it('keeps query state in the url', async () => {
    stubApi(ROUTES);
    navigation.go('/search?q=rust&kind=episodes');
    render(App);

    const input = await screen.findByLabelText('Search text');
    expect(input).toHaveValue('rust');
    expect(screen.getByLabelText('What to search')).toHaveValue('episodes');
  });

  it('says a path with no page has none', async () => {
    stubApi(ROUTES);
    navigation.go('/nowhere');
    render(App);
    expect(await screen.findByRole('heading', { level: 1, name: 'Not found' })).toBeInTheDocument();
  });

  it('announces an unreachable api once', async () => {
    stubApi({ ...ROUTES, '/api/v1/health': { offline: true } });
    render(App);
    const alerts = await screen.findAllByRole('alert');
    expect(alerts[0]).toHaveTextContent('The Uguisu API is not answering');
  });

  it('reports the live stream state', async () => {
    stubApi(ROUTES);
    render(App);
    expect(await screen.findByText('connecting')).toBeInTheDocument();

    const { FakeEventSource } = await import('../src/tests/harness');
    FakeEventSource.latest.open();
    await waitFor(() => expect(screen.getByText('live')).toBeInTheDocument());
  });

  it('offers a skip link before the navigation', async () => {
    stubApi(ROUTES);
    render(App);
    const skip = await screen.findByRole('link', { name: 'Skip to content' });
    expect(skip).toHaveAttribute('href', '#main');
    expect(document.querySelector('main')).toHaveAttribute('id', 'main');
  });

  it('shows the login form when a credential is needed', async () => {
    stubApi({
      ...ROUTES,
      '/api/v1/auth/session': {
        body: { ...EMPTY.session, auth_required: true, credential_set: true, username: 'uguisu' },
      },
    });
    render(App);

    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeInTheDocument();
    // Nothing behind the gate is reachable, not even the navigation.
    expect(screen.queryByRole('link', { name: 'Podcasts' })).not.toBeInTheDocument();
  });

  it('shows the views when no credential is needed', async () => {
    stubApi(ROUTES);
    render(App);

    expect(await screen.findByRole('link', { name: 'Podcasts' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Sign in' })).not.toBeInTheDocument();
  });

  it('offers a way out once signed in', async () => {
    stubApi({
      ...ROUTES,
      '/api/v1/auth/session': [
        {
          body: {
            ...EMPTY.session,
            auth_required: true,
            credential_set: true,
            authenticated: true,
            username: 'uguisu',
            csrf_token: 'csrf-1',
          },
        },
        { body: { ...EMPTY.session, auth_required: true, credential_set: true } },
      ],
      'POST /api/v1/auth/logout': { status: 204 },
    });
    render(App);

    const out = await screen.findByRole('button', { name: 'Sign out (uguisu)' });
    out.click();
    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeInTheDocument();
  });
});
