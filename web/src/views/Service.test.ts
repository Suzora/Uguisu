import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import Service from './Service.svelte';
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
        inflight: 1,
        concurrency: 4,
        interval_secs: 3600,
        due_now: 2,
        next_due_at: '2026-01-01T13:00:00Z',
        last_maintenance_at: '2026-01-01T03:00:00Z',
        ...(overrides.scheduler as object | undefined),
      },
      search: {
        state: 'ready',
        built_at: '2026-01-01T10:00:00Z',
        podcasts: 3,
        episodes: 120,
        detail: null,
        updated_at: '2026-01-01T10:00:00Z',
      },
      downloads: EMPTY.downloadStats,
      podcasts: 3,
      settings_problems: 0,
      schema: 1,
      ...overrides,
    },
  };
}

describe('service view', () => {
  afterEach(() => events.stop());

  it('shows what the scheduler is doing', async () => {
    stubApi({ '/api/v1/status': status() });
    render(Service);

    const card = (await screen.findByRole('heading', { name: 'Feed scheduler' })).closest('section');
    expect(card).toHaveTextContent('running');
    expect(card).toHaveTextContent('1 of 4');
    expect(card).toHaveTextContent('60 min');
  });

  it('offers a resume only when paused', async () => {
    stubApi({
      '/api/v1/status': status({
        scheduler: { paused: true, paused_reason: 'maintenance window' },
      }),
    });
    render(Service);

    expect(await screen.findByText('maintenance window')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Resume the scheduler' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Pause the scheduler' })).not.toBeInTheDocument();
  });

  it('reports a pass that found nothing to do', async () => {
    stubApi({
      '/api/v1/status': status(),
      'POST /api/v1/scheduler/run': { status: 202, body: { due: 0, started: 0, paused: false, schema: 1 } },
    });
    render(Service);
    (await screen.findByRole('button', { name: 'Run one pass now' })).click();
    await waitFor(() =>
      expect(screen.getByText('0 of 0 due podcasts started refreshing.')).toBeInTheDocument(),
    );
  });

  it('reports a pass the pause suppressed', async () => {
    stubApi({
      '/api/v1/status': status(),
      'POST /api/v1/scheduler/run': { status: 202, body: { due: 3, started: 0, paused: true, schema: 1 } },
    });
    render(Service);
    (await screen.findByRole('button', { name: 'Run one pass now' })).click();
    await waitFor(() =>
      expect(screen.getByText('The scheduler is paused, so nothing was started.')).toBeInTheDocument(),
    );
  });

  it('reports what housekeeping pruned', async () => {
    stubApi({
      '/api/v1/status': status(),
      'POST /api/v1/scheduler/maintenance': { body: { events_pruned: 40, cache_expired: 2, schema: 1 } },
    });
    render(Service);
    (await screen.findByRole('button', { name: 'Run housekeeping' })).click();
    await waitFor(() =>
      expect(screen.getByText('40 events pruned, 2 cache rows expired.')).toBeInTheDocument(),
    );
  });

  it('states that the api has no authentication', async () => {
    stubApi({ '/api/v1/status': status() });
    render(Service);
    expect(await screen.findByText(/no authentication/)).toBeInTheDocument();
  });

  it('reports a service that did not answer', async () => {
    stubApi({ '/api/v1/status': { offline: true } });
    render(Service);
    expect(await screen.findByRole('alert')).toHaveTextContent('The service did not answer');
  });
});
