import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import Downloads from './Downloads.svelte';
import {
  EMPTY,
  FakeEventSource,
  job,
  podcastDetail,
  stubApi,
  stubEventSource,
  wholeText,
} from '../tests/harness';
import { events } from '../lib/events.svelte';

function stats(overrides: Record<string, unknown> = {}) {
  return { body: { ...EMPTY.downloadStats, ...overrides } };
}

describe('downloads view', () => {
  // The stream is a module singleton, so each test must leave it closed.
  afterEach(() => events.stop());

  it('renders a job with its podcast and progress', async () => {
    const api = stubApi({
      '/api/v1/downloads/stats': stats({ by_state: { downloading: 1 }, running: 1 }),
      '/api/v1/downloads': { body: { jobs: [job()], next_after: null, schema: 1 } },
    });
    render(Downloads, { props: { query: {} } });

    expect(await screen.findByRole('link', { name: 'Darknet Diaries' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'EP 1: Ear to the Ground' })).toHaveAttribute(
      'href',
      '/episodes/01BX5ZZKBKACTAV9WEVGEMMVRZ',
    );
    expect(screen.getByText('11.4 MiB of 45.8 MiB (25%)')).toBeInTheDocument();
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '25');
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
    // The row carries both titles, so the library is never read.
    expect(api.calls.filter((c) => c.path.startsWith('/api/v1/podcasts'))).toEqual([]);
  });

  it('shows a failure reason without the logs', async () => {
    stubApi({
      '/api/v1/downloads/stats': stats({ by_state: { failed: 1 } }),
      '/api/v1/downloads': {
        body: {
          jobs: [
            job({
              state: 'failed',
              state_reason: 'attempts_exhausted',
              last_error_kind: 'http_status',
              last_error_detail: 'the server answered 403',
            }),
          ],
          next_after: null,
          schema: 1,
        },
      },
    });
    render(Downloads, { props: { query: {} } });

    expect(await screen.findByText('http status: the server answered 403')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
  });

  it('names an untitled episode by its id', async () => {
    stubApi({
      '/api/v1/downloads/stats': stats({ by_state: { downloading: 1 } }),
      '/api/v1/downloads': { body: { jobs: [job({ episode_title: '' })], next_after: null, schema: 1 } },
    });
    render(Downloads, { props: { query: {} } });
    expect(await screen.findByRole('link', { name: '01BX5ZZKBKACTAV9WEVGEMMVRZ' })).toHaveAttribute(
      'href',
      '/episodes/01BX5ZZKBKACTAV9WEVGEMMVRZ',
    );
  });

  it('names the podcast it is filtered to', async () => {
    stubApi({
      '/api/v1/downloads/stats': stats(),
      '/api/v1/downloads': { body: EMPTY.jobs },
      '/api/v1/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV': { body: podcastDetail() },
    });
    render(Downloads, { props: { query: { podcast: '01ARZ3NDEKTSV4RRFFQ69G5FAV' } } });
    expect(await screen.findByText(/Only Darknet Diaries/)).toBeInTheDocument();
  });

  it('says a filtered queue is empty differently', async () => {
    stubApi({
      '/api/v1/downloads/stats': stats(),
      '/api/v1/downloads': { body: EMPTY.jobs },
    });
    render(Downloads, { props: { query: { state: 'failed' } } });
    expect(await screen.findByText('No job is failed')).toBeInTheDocument();
  });

  it('warns when no worker will pick jobs up', async () => {
    stubApi({
      '/api/v1/downloads/stats': stats({ workers_started: false }),
      '/api/v1/downloads': { body: EMPTY.jobs },
    });
    render(Downloads, { props: { query: {} } });
    expect(await screen.findByText(/No workers are running/)).toBeInTheDocument();
  });

  it('reports a refused transition', async () => {
    stubApi({
      '/api/v1/downloads/stats': stats({ by_state: { downloading: 1 } }),
      '/api/v1/downloads': { body: { jobs: [job()], next_after: null, schema: 1 } },
      'POST /api/v1/downloads/01JOB5ZZKBKACTAV9WEVGEMMVR/pause': {
        status: 409,
        body: { error: { kind: 'conflict', message: 'cannot pause a completed job' } },
      },
    });
    render(Downloads, { props: { query: {} } });
    (await screen.findByRole('button', { name: 'Pause' })).click();
    expect(await screen.findByText('cannot pause a completed job')).toBeInTheDocument();
  });

  it('counts the failures queued again', async () => {
    const failed = job({ state: 'failed', state_reason: 'attempts_exhausted' });
    stubApi({
      '/api/v1/downloads/stats': [stats({ by_state: { failed: 2 } }), stats({ by_state: { queued: 2 } })],
      '/api/v1/downloads': [
        { body: { jobs: [failed, { ...failed, id: '01JOB2' }], next_after: null, schema: 1 } },
        { body: { jobs: [job({ state: 'queued' }), job({ id: '01JOB2', state: 'queued' })], next_after: null, schema: 1 } },
      ],
      'POST /api/v1/downloads/retry-failed': { body: { requeued: 2, schema: 1 } },
    });
    render(Downloads, { props: { query: {} } });
    const retryAll = screen.getByRole('button', { name: 'Retry every failure' });
    await waitFor(() => expect(retryAll).toBeEnabled());
    retryAll.click();
    // The re-read after the action is the last thing that settles.
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'Pause' })).toHaveLength(2));
    expect(screen.getByText('2 failed jobs queued again.')).toBeInTheDocument();
    expect(screen.queryByText('Queued again.')).not.toBeInTheDocument();
  });

  it('applies live progress to the row', async () => {
    stubEventSource();
    stubApi({
      '/api/v1/downloads/stats': stats({ by_state: { downloading: 1 } }),
      '/api/v1/downloads': { body: { jobs: [job()], next_after: null, schema: 1 } },
    });
    render(Downloads, { props: { query: {} } });
    await screen.findByRole('button', { name: 'Cancel' });

    // The shell owns the connection, so a view test opens it by hand.
    events.start();
    FakeEventSource.latest.open();
    FakeEventSource.latest.emit('download.progress', {
      schema: 1,
      id: '01EV',
      occurred_at: '2026-01-01T12:00:00Z',
      podcast_id: null,
      episode_id: null,
      kind: 'download.progress',
      job_id: '01JOB5ZZKBKACTAV9WEVGEMMVR',
      bytes_downloaded: 24_000_000,
      total_bytes: 48_000_000,
      percentage: 50,
      speed_bps: 1_048_576,
      eta_secs: 24,
    });

    await waitFor(() =>
      expect(screen.getByText('22.9 MiB of 45.8 MiB (50%)')).toBeInTheDocument(),
    );
    expect(screen.getByText(wholeText('1.0 MiB/s · 24 s left'))).toBeInTheDocument();
    events.stop();
  });

  it('re-reads the queue after a reconnect', async () => {
    stubEventSource();
    stubApi({
      '/api/v1/downloads/stats': [stats({ by_state: { downloading: 1 } }), stats({ by_state: { completed: 1 } })],
      '/api/v1/downloads': [
        { body: { jobs: [job()], next_after: null, schema: 1 } },
        { body: { jobs: [job({ state: 'completed', bytes_downloaded: 48_000_000 })], next_after: null, schema: 1 } },
      ],
    });
    render(Downloads, { props: { query: {} } });
    await screen.findByRole('button', { name: 'Cancel' });

    events.start();
    FakeEventSource.latest.open();
    FakeEventSource.latest.emit('download.progress', {
      schema: 1,
      id: '01EV',
      occurred_at: '2026-01-01T12:00:00Z',
      podcast_id: null,
      episode_id: null,
      kind: 'download.progress',
      job_id: '01JOB5ZZKBKACTAV9WEVGEMMVR',
      bytes_downloaded: 24_000_000,
      total_bytes: 48_000_000,
      percentage: 50,
      speed_bps: 1_048_576,
      eta_secs: 24,
    });
    await waitFor(() =>
      expect(screen.getByText('22.9 MiB of 45.8 MiB (50%)')).toBeInTheDocument(),
    );

    // A resync must drop the live overlay and trust the API again.
    FakeEventSource.latest.open();
    await waitFor(() => expect(screen.getByText('completed')).toBeInTheDocument());
    expect(screen.queryByText(wholeText('1.0 MiB/s · 24 s left'))).not.toBeInTheDocument();
    events.stop();
  });
});
