import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import Archive from './Archive.svelte';
import { EMPTY, podcastDetail, stubApi } from '../tests/harness';
import { events } from '../lib/events.svelte';

function file(overrides: Record<string, unknown> = {}) {
  return {
    id: '01FILE',
    episode_id: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
    podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
    relative_path: 'Darknet Diaries/2026-01-01 Episode 1.mp3',
    size_bytes: 48_000_000,
    content_type: 'audio/mpeg',
    hash_algo: 'sha256',
    hash_value: 'abc123',
    origin: 'download',
    tag_state: 'written',
    sidecar_written_at: '2026-01-01T11:35:00Z',
    verification_state: 'verified',
    verification_reason: 'hash_match',
    verified_at: '2026-01-01T11:36:00Z',
    registered_at: '2026-01-01T11:35:00Z',
    ...overrides,
  };
}

const BASE = {
  '/api/v1/archive/stats': { body: { by_state: { verified: 1 }, total: 1, schema: 1 } },
  '/api/v1/archive/manifests': { body: EMPTY.manifests },
  '/api/v1/podcasts': { body: { podcasts: [podcastDetail()], schema: 1 } },
};

describe('archive view', () => {
  afterEach(() => events.stop());

  it('shows a file and its verification', async () => {
    stubApi({ ...BASE, '/api/v1/archive': { body: { files: [file()], schema: 1 } } });
    render(Archive, { props: { query: {} } });

    expect(
      await screen.findByRole('link', { name: 'Darknet Diaries/2026-01-01 Episode 1.mp3' }),
    ).toHaveAttribute('href', '/episodes/01BX5ZZKBKACTAV9WEVGEMMVRZ');
    expect(screen.getByText('hash match')).toBeInTheDocument();
    expect(screen.getByText('45.8 MiB')).toBeInTheDocument();
    expect(screen.getByText('written')).toBeInTheDocument();
  });

  it('pages the archive with the cursor', async () => {
    stubApi({
      ...BASE,
      '/api/v1/archive': [
        { body: { files: [file()], next_after: '01FILE', schema: 1 } },
        {
          body: {
            files: [file({ id: '01OLDER', relative_path: 'Darknet Diaries/2025-12-01 Episode 0.mp3' })],
            next_after: null,
            schema: 1,
          },
        },
      ],
    });
    render(Archive, { props: { query: {} } });

    expect(await screen.findByText('Darknet Diaries/2026-01-01 Episode 1.mp3')).toBeInTheDocument();
    screen.getByRole('button', { name: 'Load more' }).click();
    expect(await screen.findByText('Darknet Diaries/2025-12-01 Episode 0.mp3')).toBeInTheDocument();
    expect(screen.getByText('Darknet Diaries/2026-01-01 Episode 1.mp3')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
  });

  it('keeps the filter on later pages', async () => {
    const api = stubApi({
      ...BASE,
      '/api/v1/archive': [
        { body: { files: [file({ verification_state: 'missing' })], next_after: '01FILE', schema: 1 } },
        { body: { files: [], next_after: null, schema: 1 } },
      ],
    });
    render(Archive, { props: { query: { state: 'missing' } } });

    (await screen.findByRole('button', { name: 'Load more' })).click();
    await waitFor(() =>
      expect(api.calls.filter((c) => c.path.startsWith('/api/v1/archive?')).map((c) => c.path)).toEqual([
        '/api/v1/archive?state=missing&limit=200',
        '/api/v1/archive?state=missing&after=01FILE&limit=200',
      ]),
    );
  });

  it('says a filtered archive is empty differently', async () => {
    stubApi({ ...BASE, '/api/v1/archive': { body: { files: [], schema: 1 } } });
    render(Archive, { props: { query: { state: 'missing' } } });
    expect(await screen.findByText('No file is missing')).toBeInTheDocument();
  });

  it('reports what a verification found', async () => {
    const api = stubApi({
      ...BASE,
      '/api/v1/archive': { body: { files: [file()], schema: 1 } },
      'POST /api/v1/archive/verify': {
        body: { checked: 7, verified: 6, missing: 1, invalid: 0, unchecked: 0, depth: 'full', schema: 1 },
      },
    });
    render(Archive, { props: { query: {} } });
    (await screen.findByRole('button', { name: 'Verify every hash' })).click();
    await screen.findByText('Hashed 7: 6 intact, 1 missing, 0 invalid.');
    // Once the action is over, its report is what stays on screen.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByText('Hashed 7: 6 intact, 1 missing, 0 invalid.')).toBeInTheDocument();
    // No podcast is chosen, so none is named: the server refuses an empty id.
    expect(api.calls.find((c) => c.method === 'POST')?.body).toEqual({ depth: 'full' });
  });

  it('reports a single file that no longer matches', async () => {
    stubApi({
      ...BASE,
      '/api/v1/archive': { body: { files: [file()], schema: 1 } },
      'POST /api/v1/archive/01BX5ZZKBKACTAV9WEVGEMMVRZ/verify': {
        body: {
          file: file({ verification_state: 'invalid' }),
          state: 'invalid',
          reason: 'hash_mismatch',
          depth: 'full',
          detail: null,
          schema: 1,
        },
      },
    });
    render(Archive, { props: { query: {} } });
    (await screen.findByRole('button', { name: 'Verify' })).click();
    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('invalid: hash mismatch'),
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByRole('alert')).toHaveTextContent('invalid: hash mismatch');
    expect(screen.queryByText('Verified.')).not.toBeInTheDocument();
  });

  it('warns about manifests that drifted', async () => {
    stubApi({
      ...BASE,
      '/api/v1/archive': { body: { files: [file()], schema: 1 } },
      '/api/v1/archive/manifests': {
        body: {
          manifests: [
            {
              podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
              relative_path: '.uguisu/manifests/01ARZ/manifest.sha256',
              entries: 3,
              hash_value: 'deadbeef',
              stale: true,
              generated_at: '2026-01-01T10:00:00Z',
            },
          ],
          schema: 1,
        },
      },
    });
    render(Archive, { props: { query: {} } });
    expect(await screen.findByText(/1 manifest is out of date/)).toBeInTheDocument();
  });

  it('separates an unreadable archive from an empty one', async () => {
    stubApi({ ...BASE, '/api/v1/archive': { offline: true } });
    render(Archive, { props: { query: {} } });
    expect(await screen.findByRole('alert')).toHaveTextContent('The archive could not be read');
  });
});
