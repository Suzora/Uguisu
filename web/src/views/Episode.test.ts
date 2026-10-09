import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import Episode from './Episode.svelte';
import { episode, job, stubApi } from '../tests/harness';
import { events } from '../lib/events.svelte';
import { player } from '../lib/player.svelte';

const ID = '01BX5ZZKBKACTAV9WEVGEMMVRZ';

function detail(overrides: Record<string, unknown> = {}) {
  return {
    body: {
      episode: episode({
        first_seen_at: '2026-01-01T10:00:00Z',
        link: 'https://darknetdiaries.com/episode/1/',
      }),
      podcast_title: 'Darknet Diaries',
      archive: null,
      job: null,
      schema: 1,
      ...overrides,
    },
  };
}

describe('episode view', () => {
  afterEach(() => {
    events.stop();
    player.close();
  });

  it('renders an episode', async () => {
    const api = stubApi({ [`/api/v1/episodes/${ID}`]: detail() });
    render(Episode, { props: { id: ID } });

    expect(
      await screen.findByRole('heading', { level: 1, name: 'Episode 1: The Beginning' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('link', { name: '← Darknet Diaries' })).toHaveAttribute(
      'href',
      '/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV',
    );
    expect(screen.getByText('How it started.')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Episode page' })).toHaveAttribute(
      'href',
      'https://darknetdiaries.com/episode/1/',
    );
    expect(screen.getByText('expected')).toBeInTheDocument();
    expect(screen.getByText('Not in the archive.')).toBeInTheDocument();
    expect(screen.getByText('Never queued.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Play' })).not.toBeInTheDocument();
    expect(api.calls.map((c) => c.path)).toContain(`/api/v1/episodes/${ID}`);
  });

  it('offers no script as a link', async () => {
    stubApi({
      [`/api/v1/episodes/${ID}`]: detail({
        episode: episode({ first_seen_at: '2026-01-01T10:00:00Z', link: 'javascript://x/%0aalert(1)' }),
      }),
    });
    render(Episode, { props: { id: ID } });
    await screen.findByRole('heading', { level: 1 });
    expect(screen.queryByRole('link', { name: 'Episode page' })).not.toBeInTheDocument();
  });

  it('shows the archived file and plays it', async () => {
    stubApi({
      [`/api/v1/episodes/${ID}`]: detail({
        episode: episode({ archive_state: 'archived', first_seen_at: '2026-01-01T10:00:00Z' }),
        archive: {
          id: '01FILE',
          episode_id: ID,
          podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
          relative_path: 'Darknet Diaries/2026-01-01 Episode 1.mp3',
          size_bytes: 48_000_000,
          tag_state: 'written',
          verification_state: 'verified',
          verification_reason: 'hash_match',
          verified_at: '2026-01-01T11:36:00Z',
        },
        job: job({ state: 'completed', attempt_count: 2 }),
      }),
    });
    render(Episode, { props: { id: ID } });

    expect(await screen.findByText('Darknet Diaries/2026-01-01 Episode 1.mp3')).toBeInTheDocument();
    expect(screen.getByText('45.8 MiB')).toBeInTheDocument();
    expect(screen.getByText('hash match')).toBeInTheDocument();
    expect(screen.getByText('completed')).toBeInTheDocument();
    expect(screen.getByText('2 of 5')).toBeInTheDocument();

    screen.getByRole('button', { name: 'Play' }).click();
    await waitFor(() =>
      expect(player.current).toMatchObject({
        episodeId: ID,
        podcastTitle: 'Darknet Diaries',
        src: `/api/v1/archive/${ID}/media`,
      }),
    );
  });

  it('links missing file to repair', async () => {
    stubApi({
      [`/api/v1/episodes/${ID}`]: detail({
        episode: episode({ archive_state: 'missing', first_seen_at: '2026-01-01T10:00:00Z' }),
        archive: {
          id: '01FILE',
          episode_id: ID,
          podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
          relative_path: 'Darknet Diaries/2026-01-01 Episode 1.mp3',
          size_bytes: 48_000_000,
          tag_state: 'untagged',
          verification_state: 'missing',
          verification_reason: 'not_found',
          verified_at: '2026-01-01T11:36:00Z',
        },
        job: job({ state: 'completed' }),
      }),
    });
    render(Episode, { props: { id: ID } });

    expect(await screen.findByRole('link', { name: 'Repair this file' })).toHaveAttribute(
      'href',
      '/archive/repair?podcast=01ARZ3NDEKTSV4RRFFQ69G5FAV',
    );
    expect(screen.queryByRole('button', { name: 'Play' })).not.toBeInTheDocument();
  });

  it('shows why a download failed', async () => {
    stubApi({
      [`/api/v1/episodes/${ID}`]: detail({
        job: job({
          state: 'failed',
          state_reason: 'attempts_exhausted',
          last_error_kind: 'http_status',
          last_error_detail: 'the server answered 403',
        }),
      }),
    });
    render(Episode, { props: { id: ID } });

    expect(await screen.findByText('attempts exhausted')).toBeInTheDocument();
    expect(screen.getByText('http status: the server answered 403')).toBeInTheDocument();
  });

  it('reports an episode that is not there', async () => {
    stubApi({
      [`/api/v1/episodes/${ID}`]: {
        status: 404,
        body: { error: { kind: 'not_found', message: 'episode not found' } },
      },
    });
    render(Episode, { props: { id: ID } });

    expect(await screen.findByRole('alert')).toHaveTextContent('This episode could not be read');
  });
});
