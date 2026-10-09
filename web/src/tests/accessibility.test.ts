// Structural accessibility checks over the real views.
//
// These are not a substitute for using the UI with a screen reader; they
// catch the regressions that are mechanical: a control that is a `div`, an
// input with no label, a heading level skipped, a page that changes without
// telling anyone.

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import App from '../App.svelte';
import { EMPTY, episode, job, podcastDetail, stubApi, stubEventSource } from './harness';
import { events } from '../lib/events.svelte';
import { navigation } from '../lib/navigation.svelte';

const ID = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
const EPISODE = '01BX5ZZKBKACTAV9WEVGEMMVRZ';

const ROUTES = {
  '/api/v1/health': { body: { status: 'ok', version: '0.1.0' } },
  '/api/v1/status': {
    body: {
      version: '0.1.0',
      scheduler: {
        enabled: true,
        running: true,
        paused: false,
        paused_reason: null,
        paused_at: null,
        inflight: 0,
        concurrency: 4,
        interval_secs: 3600,
        due_now: 0,
        next_due_at: null,
        last_maintenance_at: null,
      },
      search: {
        state: 'ready',
        built_at: null,
        podcasts: 1,
        episodes: 1,
        detail: null,
        updated_at: '2026-01-01T10:00:00Z',
      },
      downloads: EMPTY.downloadStats,
      podcasts: 1,
      settings_problems: 0,
      schema: 1,
    },
  },
  '/api/v1/archive/stats': { body: EMPTY.archiveStats },
  '/api/v1/events': { body: EMPTY.events },
  '/api/v1/podcasts': { body: { podcasts: [podcastDetail()], schema: 1 } },
  [`/api/v1/podcasts/${ID}`]: { body: podcastDetail() },
  [`/api/v1/podcasts/${ID}/artwork`]: { body: EMPTY.artwork },
  [`/api/v1/podcasts/${ID}/episodes`]: {
    body: { episodes: [episode()], next_after: null, schema: 1 },
  },
  [`/api/v1/podcasts/${ID}/policy`]: {
    body: {
      podcast_id: ID,
      stored: null,
      effective: { mode: 'manual', max_backlog: 5, max_age_days: 90, priority: 'normal' },
      schema: 1,
    },
  },
  [`/api/v1/episodes/${EPISODE}`]: {
    body: {
      episode: episode({ first_seen_at: '2026-01-01T10:00:00Z' }),
      podcast_title: 'Darknet Diaries',
      archive: null,
      job: job(),
      schema: 1,
    },
  },
  '/api/v1/downloads': { body: { jobs: [job()], next_after: null, schema: 1 } },
  '/api/v1/downloads/stats': { body: EMPTY.downloadStats },
  '/api/v1/archive': { body: { files: [], schema: 1 } },
  '/api/v1/archive/manifests': { body: EMPTY.manifests },
  '/api/v1/settings': {
    body: {
      keys: [
        {
          key: 'UGUISU_FEED_REFRESH_INTERVAL_SECS',
          value: '3600',
          origin: 'default',
          stored: null,
          pinned: false,
          persistable: true,
          live: true,
        },
      ],
      rejected: [],
      unused: [],
      schema: 1,
    },
  },
  '/api/v1/discovery/providers': { body: { providers: [], cache: { entries: 0 } } },
  '/api/v1/search': {
    body: {
      outcome: 'empty_query',
      terms: [],
      truncated: false,
      podcasts: [],
      episodes: [],
      index: {
        state: 'ready',
        built_at: null,
        podcasts: 1,
        episodes: 1,
        detail: null,
        updated_at: '2026-01-01T10:00:00Z',
      },
      duration_ms: 1,
      schema: 1,
    },
  },
};

const PAGES = [
  '/',
  '/podcasts',
  `/podcasts/${ID}`,
  `/episodes/${EPISODE}`,
  '/discover',
  '/search',
  '/downloads',
  '/archive',
  '/archive/import',
  '/archive/repair',
  '/settings',
  '/service',
];

describe('accessibility', () => {
  beforeEach(() => {
    stubEventSource();
    stubApi(ROUTES);
  });

  afterEach(() => events.stop());

  it('gives every page one first-level heading', async () => {
    for (const page of PAGES) {
      navigation.go(page);
      const view = render(App);
      await waitFor(() => expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1));
      view.unmount();
    }
  });

  it('never skips a heading level', async () => {
    for (const page of PAGES) {
      navigation.go(page);
      const view = render(App);
      await waitFor(() => expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1));
      const levels = [...document.querySelectorAll('h1, h2, h3')].map((h) =>
        Number(h.tagName[1]),
      );
      let previous = 0;
      for (const level of levels) {
        expect(level - previous, `${page} jumps to h${level}`).toBeLessThanOrEqual(1);
        previous = Math.max(previous, level);
      }
      view.unmount();
    }
  });

  it('labels every form control', async () => {
    for (const page of PAGES) {
      navigation.go(page);
      const view = render(App);
      await waitFor(() => expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1));
      for (const control of document.querySelectorAll('input, select, textarea')) {
        const labelled =
          control.closest('label') !== null ||
          control.getAttribute('aria-label') !== null ||
          control.getAttribute('aria-labelledby') !== null;
        expect(labelled, `${page} has an unlabelled ${control.tagName.toLowerCase()}`).toBe(true);
      }
      view.unmount();
    }
  });

  it('uses real controls, not clickable divs', async () => {
    for (const page of PAGES) {
      navigation.go(page);
      const view = render(App);
      await waitFor(() => expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1));
      const fakes = [...document.querySelectorAll('div[onclick], span[onclick], li[onclick]')];
      expect(fakes, `${page} has a clickable non-control`).toHaveLength(0);
      view.unmount();
    }
  });

  it('gives every button an accessible name', async () => {
    for (const page of PAGES) {
      navigation.go(page);
      const view = render(App);
      await waitFor(() => expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1));
      for (const button of screen.queryAllByRole('button')) {
        const name = button.getAttribute('aria-label') ?? button.textContent?.trim() ?? '';
        expect(name.length, `${page} has a nameless button`).toBeGreaterThan(0);
      }
      view.unmount();
    }
  });

  it('announces a page change', async () => {
    navigation.go('/');
    render(App);
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });

    screen.getByRole('link', { name: 'Downloads' }).click();
    await waitFor(() => {
      const live = document.querySelector('[aria-live="polite"]');
      expect(live).toHaveTextContent('Downloads page');
    });
    expect(document.activeElement).toBe(document.querySelector('main'));
  });

  it('names every table column', async () => {
    for (const page of ['/downloads', '/archive']) {
      navigation.go(page);
      const view = render(App);
      await waitFor(() => expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1));
      for (const table of document.querySelectorAll('table')) {
        expect(table.querySelector('caption'), `${page} has an unnamed table`).not.toBeNull();
        for (const header of table.querySelectorAll('thead th')) {
          expect(header.getAttribute('scope')).toBe('col');
        }
      }
      view.unmount();
    }
  });
});
