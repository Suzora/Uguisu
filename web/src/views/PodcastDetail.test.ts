import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import PodcastDetail from './PodcastDetail.svelte';
import { EMPTY, episode, job, podcastDetail, stubApi, wholeText } from '../tests/harness';
import { events } from '../lib/events.svelte';
import { player } from '../lib/player.svelte';
import { navigation } from '../lib/navigation.svelte';

const ID = '01ARZ3NDEKTSV4RRFFQ69G5FAV';

const POLICY = {
  body: {
    podcast_id: ID,
    stored: null,
    effective: { mode: 'manual', max_backlog: 5, max_age_days: 90, priority: 'normal' },
    schema: 1,
  },
};

function base(overrides: Record<string, unknown> = {}) {
  return {
    [`/api/v1/podcasts/${ID}`]: { body: podcastDetail() },
    [`/api/v1/podcasts/${ID}/policy`]: POLICY,
    [`/api/v1/podcasts/${ID}/artwork`]: { body: EMPTY.artwork },
    [`/api/v1/podcasts/${ID}/episodes`]: { body: EMPTY.episodes },
    '/api/v1/downloads': { body: EMPTY.jobs },
    ...overrides,
  };
}

describe('podcast detail', () => {
  afterEach(() => {
    events.stop();
    player.close();
  });

  it('shows what the podcast is', async () => {
    stubApi(base());
    render(PodcastDetail, { props: { id: ID, query: {} } });

    expect(await screen.findByRole('heading', { level: 1, name: 'Darknet Diaries' })).toBeInTheDocument();
    expect(screen.getByText('Jack Rhysider')).toBeInTheDocument();
    expect(screen.getByText('Technology')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Feed' })).toHaveAttribute(
      'href',
      'https://feeds.example/darknet.xml',
    );
  });

  it('reports a podcast that is not there', async () => {
    stubApi({
      [`/api/v1/podcasts/${ID}`]: {
        status: 404,
        body: { error: { kind: 'not_found', message: `podcast \`${ID}\` not found` } },
      },
      [`/api/v1/podcasts/${ID}/policy`]: POLICY,
      [`/api/v1/podcasts/${ID}/artwork`]: { body: EMPTY.artwork },
    });
    render(PodcastDetail, { props: { id: ID, query: {} } });
    expect(await screen.findByRole('alert')).toHaveTextContent('This podcast could not be read');
  });

  it('says a feed with no episodes has none', async () => {
    stubApi(base());
    render(PodcastDetail, { props: { id: ID, query: {} } });
    expect(await screen.findByText('No episodes yet')).toBeInTheDocument();
  });

  it('reports what a refresh did', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}`]: [{ body: podcastDetail() }, { body: podcastDetail({ episodes_total: 4 }) }],
        [`POST /api/v1/podcasts/${ID}/refresh`]: {
          body: {
            podcast_id: ID,
            outcome: 'fetched',
            episodes: { seen: 3, added: 2, updated: 1, unchanged: 0, malformed: 0, removed_detected: 0, ambiguous: 0 },
            warnings: [],
            duration_ms: 120,
            schema: 1,
          },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Refresh now' })).click();
    // The re-read after the action is the last thing that settles.
    expect(await screen.findByText('4 episodes')).toBeInTheDocument();
    expect(screen.getByText('Refreshed: 2 new, 1 updated.')).toBeInTheDocument();
    expect(screen.queryByText('Refreshed.')).not.toBeInTheDocument();
  });

  it('reports what queueing everything did', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}`]: [{ body: podcastDetail() }, { body: podcastDetail({ episodes_total: 3 }) }],
        [`POST /api/v1/podcasts/${ID}/downloads`]: {
          body: {
            created: 3,
            existing: 1,
            completed: 2,
            requeued: 0,
            skipped: [{ episode_id: '01BX5ZZKBKACTAV9WEVGEMMVRZ', reason: 'older than 90 days' }],
            schema: 1,
          },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Download every episode' })).click();
    expect(await screen.findByText('3 episodes')).toBeInTheDocument();
    expect(
      screen.getByText('3 queued, 1 already queued, 2 already archived, 1 skipped.'),
    ).toBeInTheDocument();
    expect(screen.queryByText('Queued.')).not.toBeInTheDocument();
  });

  it('reports a refused pause', async () => {
    stubApi(
      base({
        [`POST /api/v1/podcasts/${ID}/pause`]: {
          status: 409,
          body: { error: { kind: 'conflict', message: 'an archived podcast cannot be paused' } },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Pause refreshing' })).click();
    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('an archived podcast cannot be paused'),
    );
  });

  it('archives a podcast', async () => {
    const archived = podcastDetail();
    archived.podcast.status = 'archived';
    const api = stubApi(
      base({
        [`/api/v1/podcasts/${ID}`]: [{ body: podcastDetail() }, { body: archived }],
        [`POST /api/v1/podcasts/${ID}/archive`]: { body: { podcast_id: ID, status: 'archived', schema: 1 } },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Archive' })).click();
    expect(await screen.findByText('Archived: never refreshed until you resume it.')).toBeInTheDocument();
    expect(api.calls.some((c) => c.method === 'POST' && c.path.endsWith('/archive'))).toBe(true);
    expect(await screen.findByRole('button', { name: 'Resume' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Refresh now' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Archive' })).not.toBeInTheDocument();
  });

  it('removes a podcast after asking', async () => {
    const api = stubApi(
      base({
        [`DELETE /api/v1/podcasts/${ID}`]: {
          body: { podcast_id: ID, title: 'Darknet Diaries', episodes: 3, files: 2, schema: 1 },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Remove from the library' })).click();
    expect(
      await screen.findByText(/Remove Darknet Diaries and the records of its \d+ episodes\? Every file stays on disk/),
    ).toBeInTheDocument();
    expect(api.calls.some((c) => c.method === 'DELETE')).toBe(false);
    screen.getByRole('button', { name: 'Remove from the library' }).click();
    await waitFor(() => expect(navigation.current.href).toBe('/podcasts'));
    expect(api.calls.filter((c) => c.method === 'DELETE').map((c) => c.path)).toEqual([`/api/v1/podcasts/${ID}`]);
  });

  it('reports a refused removal', async () => {
    stubApi(
      base({
        [`DELETE /api/v1/podcasts/${ID}`]: {
          status: 409,
          body: { error: { kind: 'conflict', message: 'a download of Darknet Diaries is running' } },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Remove from the library' })).click();
    await screen.findByText(/Every file stays on disk/);
    screen.getByRole('button', { name: 'Remove from the library' }).click();
    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('a download of Darknet Diaries is running'),
    );
  });

  it('shows the effective policy and its source', async () => {
    stubApi(base());
    render(PodcastDetail, { props: { id: ID, query: {} } });
    expect(await screen.findByText('the global default')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Download new episodes automatically' }),
    ).toBeInTheDocument();
  });

  it('pages episodes with the cursor', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: [
          {
            body: {
              episodes: [episode()],
              next_after: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
              schema: 1,
            },
          },
          {
            body: {
              episodes: [episode({ id: '01SECOND', title: 'Episode 2: Later' })],
              next_after: null,
              schema: 1,
            },
          },
        ],
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });

    expect(await screen.findByText('Episode 1: The Beginning')).toBeInTheDocument();
    screen.getByRole('button', { name: 'Load more' }).click();
    expect(await screen.findByText('Episode 2: Later')).toBeInTheDocument();
    expect(screen.getByText('Episode 1: The Beginning')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
  });

  it('queues one episode', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: { body: { episodes: [episode()], next_after: null, schema: 1 } },
        'POST /api/v1/downloads': {
          status: 201,
          body: { outcome: 'created', job: job({ state: 'queued' }), schema: 1 },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Download' })).click();
    await waitFor(() => expect(screen.getByText('Queued for download.')).toBeInTheDocument());
  });

  const CANDIDATE = '01CANDZZKBKACTAV9WEVGEMMVR';
  const candidates = {
    body: {
      episodes: [
        episode({
          id: CANDIDATE,
          title: 'Episode 1: The Beginning (again)',
          archive_state: 'skipped',
          skip_reason: 'duplicate of 01BX5ZZKBKACTAV9WEVGEMMVRZ',
          duplicate_of_episode_id: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
          duplicate_reasons: ['same_enclosure_url', 'same_title'],
        }),
        episode(),
      ],
      next_after: null,
      schema: 1,
    },
  };

  it('offers to resolve a candidate', async () => {
    stubApi(base({ [`/api/v1/podcasts/${ID}/episodes`]: candidates }));
    render(PodcastDetail, { props: { id: ID, query: {} } });
    expect(
      await screen.findByText(
        wholeText('Possibly the same episode as “Episode 1: The Beginning” (same enclosure url, same title)'),
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Same episode' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Keep both' })).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Download' })).toHaveLength(1);
  });

  it('merges a candidate duplicate', async () => {
    const api = stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: [
          candidates,
          { body: { episodes: [episode()], next_after: null, schema: 1 } },
        ],
        [`POST /api/v1/episodes/${CANDIDATE}/resolve`]: {
          body: {
            resolution: 'same',
            candidate: CANDIDATE,
            original: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
            episode: episode(),
            queued: false,
            schema: 1,
          },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Same episode' })).click();
    expect(
      await screen.findByText('Merge into “Episode 1: The Beginning”? This cannot be undone.'),
    ).toBeInTheDocument();
    screen.getByRole('button', { name: 'Merge' }).click();
    await waitFor(() => expect(screen.getByText('Merged into the earlier episode.')).toBeInTheDocument());
    const post = api.calls.find((c) => c.method === 'POST' && c.path.endsWith('/resolve'));
    expect(post?.body).toEqual({ resolution: 'same' });
    await waitFor(() =>
      expect(screen.queryByText('Episode 1: The Beginning (again)')).not.toBeInTheDocument(),
    );
  });

  it('reports a refused merge', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: candidates,
        [`POST /api/v1/episodes/${CANDIDATE}/resolve`]: {
          status: 409,
          body: {
            error: { kind: 'conflict', message: 'conflict: both episodes are in the feed' },
            schema: 1,
          },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Same episode' })).click();
    (await screen.findByRole('button', { name: 'Merge' })).click();
    expect(await screen.findByText(/both episodes are in the feed/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Same episode' })).toBeInTheDocument();
  });

  it('moves to an announced feed after asking', async () => {
    const NEW = 'https://new.example/darknet.xml';
    const announced = podcastDetail({
      announced: {
        feed_url: NEW,
        discovered_at: '2026-01-01T10:00:00Z',
        fetch: {
          state: 'failed',
          consecutive_failures: 1,
          last_error_kind: 'invalid_podcast_feed',
          last_error_detail: 'title differs (`Darknet Diaries` vs `DD`) and no podcast:guid to compare',
        },
      },
    });
    const moved = {
      podcast_id: ID,
      from: 'https://feeds.example/darknet.xml',
      to: NEW,
      verified: false,
      check: 'title differs (`Darknet Diaries` vs `DD`) and no podcast:guid to compare',
      schema: 1,
    };
    const api = stubApi(
      base({
        [`/api/v1/podcasts/${ID}`]: [{ body: announced }, { body: podcastDetail() }],
        [`POST /api/v1/podcasts/${ID}/move-feed`]: [
          { body: { ...moved, moved: false, report: null } },
          {
            body: {
              ...moved,
              moved: true,
              report: {
                podcast_id: ID,
                outcome: 'fetched',
                episodes: { seen: 2, added: 0, updated: 0, unchanged: 2, malformed: 0, removed_detected: 0, ambiguous: 0 },
                warnings: [],
                duration_ms: 80,
              },
            },
          },
        ],
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });

    expect(await screen.findByRole('heading', { name: 'The feed announces a new address' })).toBeInTheDocument();
    expect(screen.getByText(NEW)).toBeInTheDocument();
    screen.getByRole('button', { name: 'Move to this feed' }).click();
    expect(await screen.findByText(/Uguisu cannot tell that this is the same podcast/)).toBeInTheDocument();
    screen.getByRole('button', { name: 'Move anyway' }).click();
    expect(await screen.findByText('Moved to the new feed. Refreshed: 0 new, 0 updated.')).toBeInTheDocument();
    const posts = api.calls.filter((c) => c.method === 'POST' && c.path.endsWith('/move-feed'));
    expect(posts.map((c) => c.body)).toEqual([
      { url: NEW, force: false },
      { url: NEW, force: true },
    ]);
    await waitFor(() =>
      expect(screen.queryByRole('heading', { name: 'The feed announces a new address' })).not.toBeInTheDocument(),
    );
  });

  it('plays an archived episode', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: {
          body: { episodes: [episode({ archive_state: 'archived' })], next_after: null, schema: 1 },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    (await screen.findByRole('button', { name: 'Play' })).click();
    await waitFor(() =>
      expect(player.current).toMatchObject({
        episodeId: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
        podcastTitle: 'Darknet Diaries',
        src: '/api/v1/archive/01BX5ZZKBKACTAV9WEVGEMMVRZ/media',
      }),
    );
  });

  it('links an episode to its page', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: {
          body: { episodes: [episode()], next_after: null, schema: 1 },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    expect(await screen.findByRole('link', { name: 'Episode 1: The Beginning' })).toHaveAttribute(
      'href',
      '/episodes/01BX5ZZKBKACTAV9WEVGEMMVRZ',
    );
  });

  it('offers no script as an episode page', async () => {
    const script = episode({ id: '01BX5ZZKBKACTAV9WEVGEMMVS0', link: 'javascript://x/%0aalert(1)' });
    const web = episode({ link: 'https://show.example/ep/1' });
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: {
          body: { episodes: [script, web], next_after: null, schema: 1 },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    const more = await screen.findAllByRole('button', { name: 'More' });
    await fireEvent.click(more[0]!);
    expect(screen.queryByRole('link', { name: /javascript/ })).not.toBeInTheDocument();
    await fireEvent.click(more[1]!);
    expect(await screen.findByRole('link', { name: 'https://show.example/ep/1' })).toHaveAttribute(
      'href',
      'https://show.example/ep/1',
    );
  });

  it('filters episodes by archive state', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: {
          body: { episodes: [episode()], next_after: null, schema: 1 },
        },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: { state: 'archived' } } });
    expect(await screen.findByText('No episode is in that state')).toBeInTheDocument();
  });

  it('shows the queue state of an episode', async () => {
    stubApi(
      base({
        [`/api/v1/podcasts/${ID}/episodes`]: {
          body: { episodes: [episode({ archive_state: 'downloading' })], next_after: null, schema: 1 },
        },
        '/api/v1/downloads': { body: { jobs: [job()], next_after: null, schema: 1 } },
      }),
    );
    render(PodcastDetail, { props: { id: ID, query: {} } });
    expect(await screen.findByText('11.4 MiB of 45.8 MiB (25%)')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
  });
});
