import { afterEach, describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Dashboard from './Dashboard.svelte';
import { EMPTY, stubApi } from '../tests/harness';
import { events } from '../lib/events.svelte';

function status(overrides: Record<string, unknown> = {}) {
  return {
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
        due_now: 2,
        next_due_at: '2026-01-01T13:00:00Z',
        last_maintenance_at: '2026-01-01T03:00:00Z',
      },
      search: {
        state: 'ready',
        built_at: '2026-01-01T10:00:00Z',
        podcasts: 3,
        episodes: 120,
        detail: null,
        updated_at: '2026-01-01T10:00:00Z',
      },
      downloads: { ...EMPTY.downloadStats, by_state: { queued: 1, failed: 2 } },
      podcasts: 3,
      settings_problems: 0,
      schema: 1,
      ...overrides,
    },
  };
}

describe('dashboard', () => {
  afterEach(() => events.stop());

  it('summarises the library and the queue', async () => {
    stubApi({
      '/api/v1/status': status(),
      '/api/v1/archive/stats': { body: { by_state: { verified: 7 }, total: 7, schema: 1 } },
      '/api/v1/events': { body: EMPTY.events },
    });
    render(Dashboard);

    expect(await screen.findByText('podcasts subscribed')).toBeInTheDocument();
    expect(screen.getByText('1 queued')).toBeInTheDocument();
    expect(screen.getByText('2 failed')).toBeInTheDocument();
    expect(screen.getByText('7 archived files')).toBeInTheDocument();
  });

  it('links an episode event to its page', async () => {
    stubApi({
      '/api/v1/status': status(),
      '/api/v1/archive/stats': { body: EMPTY.archiveStats },
      '/api/v1/events': {
        body: {
          events: [
            {
              id: '01EV1',
              kind: 'episode.discovered',
              occurred_at: '2026-01-01T10:00:00Z',
              podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
              episode_id: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
              title: 'Episode 1: The Beginning',
              identity_key: 'guid:1',
              schema: 1,
            },
            {
              id: '01EV2',
              kind: 'download.completed',
              occurred_at: '2026-01-01T11:00:00Z',
              podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
              episode_id: '01EP2',
              schema: 1,
            },
          ],
          schema: 1,
        },
      },
    });
    render(Dashboard);

    expect(await screen.findByRole('link', { name: 'Episode 1: The Beginning' })).toHaveAttribute(
      'href',
      '/episodes/01BX5ZZKBKACTAV9WEVGEMMVRZ',
    );
    expect(screen.getByRole('link', { name: 'episode' })).toHaveAttribute('href', '/episodes/01EP2');
  });

  it('separates an unreadable total from a zero', async () => {
    stubApi({
      '/api/v1/status': status(),
      '/api/v1/archive/stats': { offline: true },
      '/api/v1/events': { body: EMPTY.events },
    });
    render(Dashboard);

    expect(await screen.findByText('Archive totals unavailable.')).toBeInTheDocument();
    expect(screen.queryByText('0 archived files')).not.toBeInTheDocument();
  });

  it('says when nothing has happened yet', async () => {
    stubApi({
      '/api/v1/status': status(),
      '/api/v1/archive/stats': { body: EMPTY.archiveStats },
      '/api/v1/events': { body: EMPTY.events },
    });
    render(Dashboard);
    expect(await screen.findByText('No events yet')).toBeInTheDocument();
  });

  it('points at settings that are being ignored', async () => {
    stubApi({
      '/api/v1/status': status({ settings_problems: 2 }),
      '/api/v1/archive/stats': { body: EMPTY.archiveStats },
      '/api/v1/events': { body: EMPTY.events },
    });
    render(Dashboard);
    expect(await screen.findByText(/2 stored settings are being ignored/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Review them' })).toHaveAttribute('href', '/settings');
  });

  it('reports an unreachable service instead of zeroes', async () => {
    stubApi({
      '/api/v1/status': { offline: true },
      '/api/v1/archive/stats': { body: EMPTY.archiveStats },
      '/api/v1/events': { body: EMPTY.events },
    });
    render(Dashboard);
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'The service status could not be read',
    );
  });

  it('warns that no worker is running', async () => {
    stubApi({
      '/api/v1/status': status({
        downloads: { ...EMPTY.downloadStats, workers_started: false },
      }),
      '/api/v1/archive/stats': { body: EMPTY.archiveStats },
      '/api/v1/events': { body: EMPTY.events },
    });
    render(Dashboard);
    expect(await screen.findByText(/Workers are not running/)).toBeInTheDocument();
  });
});
