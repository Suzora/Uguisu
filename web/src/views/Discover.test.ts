import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import Discover from './Discover.svelte';
import { EMPTY, podcastDetail, stubApi } from '../tests/harness';

const PROVIDERS = {
  body: {
    providers: [
      {
        id: 'apple',
        name: 'Apple Podcasts',
        enabled: true,
        disabled_reason: null,
        attribution: 'Data from Apple Podcasts',
        health: { circuit: { state: 'closed' }, consecutive_failures: 0, avg_latency_ms: 120 },
      },
      {
        id: 'podcastindex',
        name: 'Podcast Index',
        enabled: false,
        disabled_reason: 'no credentials configured',
        attribution: null,
        health: { circuit: { state: 'closed' }, consecutive_failures: 0, avg_latency_ms: null },
      },
    ],
    cache: { entries: 0 },
  },
};

function candidate(overrides: Record<string, unknown> = {}) {
  return {
    rank: 1,
    score: 0.91,
    confidence: 0.8,
    candidate: {
      title: 'Darknet Diaries',
      author: 'Jack Rhysider',
      publisher: null,
      description: 'True stories from the dark side of the internet.',
      artwork: null,
      website: 'https://darknetdiaries.com/',
      feed_url: 'https://feeds.example/darknet.xml',
      episode_count: 150,
      identities: [{ provider: 'apple', provider_ref: '123', url: null }],
      ...(overrides.candidate as object | undefined),
    },
    explanation: {
      signals: [{ name: 'title', weight: 2, value: 1, contribution: 1.8, note: 'exact title' }],
      total: 1.8,
      max_total: 2,
    },
    ambiguities: [],
    ...overrides,
  };
}

function search(overrides: Record<string, unknown> = {}) {
  return {
    body: {
      schema: 1,
      outcome: 'results',
      query: { raw: 'darknet', kind: { kind: 'term' } },
      results: [candidate()],
      providers: [
        { provider: 'apple', status: 'ok', candidates: 1, latency_ms: 120, from_cache: false, error: null, error_kind: null },
      ],
      attribution: ['Data from Apple Podcasts'],
      timing: { total_ms: 130, first_results_ms: 90 },
      complete: true,
      ...overrides,
    },
  };
}

describe('discovery', () => {
  it('invites a search before doing anything', async () => {
    stubApi({ '/api/v1/discovery/providers': PROVIDERS, '/api/v1/podcasts': { body: EMPTY.podcasts } });
    render(Discover, { props: { query: {} } });
    expect(await screen.findByText('Search for a podcast')).toBeInTheDocument();
  });

  it('offers to add a result', async () => {
    stubApi({
      '/api/v1/discovery/providers': PROVIDERS,
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/discovery/search': search(),
    });
    render(Discover, { props: { query: { q: 'darknet' } } });

    expect(await screen.findByText('Darknet Diaries')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add' })).toBeInTheDocument();
    expect(screen.getByText('Data from Apple Podcasts')).toBeInTheDocument();
  });

  it('marks a result the library already has', async () => {
    stubApi({
      '/api/v1/discovery/providers': PROVIDERS,
      '/api/v1/podcasts': { body: { podcasts: [podcastDetail()], schema: 1 } },
      '/api/v1/discovery/search': search(),
    });
    render(Discover, { props: { query: { q: 'darknet' } } });

    expect(await screen.findByText('Already in the library')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Add' })).not.toBeInTheDocument();
  });

  it('says which provider fell over', async () => {
    stubApi({
      '/api/v1/discovery/providers': PROVIDERS,
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/discovery/search': search({
        providers: [
          { provider: 'apple', status: 'ok', candidates: 1, latency_ms: 120, from_cache: false, error: null, error_kind: null },
          { provider: 'gpoddernet', status: 'timed_out', candidates: 0, latency_ms: 3000, from_cache: false, error: 'deadline', error_kind: 'timeout' },
        ],
      }),
    });
    render(Discover, { props: { query: { q: 'darknet' } } });
    expect(await screen.findByText(/gpoddernet timed_out: deadline/)).toBeInTheDocument();
    expect(screen.getByText(/these results are incomplete/)).toBeInTheDocument();
  });

  it('separates a resolution failure from no results', async () => {
    stubApi({
      '/api/v1/discovery/providers': PROVIDERS,
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/discovery/search': search({ outcome: 'no_results', results: [] }),
      'POST /api/v1/discovery/resolve': {
        status: 422,
        body: { kind: 'no_feed_available', suggestion: 'search for the show by name instead' },
      },
    });
    render(Discover, { props: { query: { q: 'https://open.spotify.com/show/abc' } } });

    (await screen.findByRole('button', { name: 'Resolve this URL' })).click();
    await waitFor(() =>
      expect(screen.getByText(/search for the show by name instead/)).toBeInTheDocument(),
    );
  });

  it('shows a resolved feed before adding it', async () => {
    stubApi({
      '/api/v1/discovery/providers': PROVIDERS,
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/discovery/search': search({ outcome: 'no_results', results: [] }),
      'POST /api/v1/discovery/resolve': {
        body: {
          input: 'https://feeds.example/darknet.xml',
          feed_url: 'https://feeds.example/darknet.xml',
          canonical_url: null,
          moved_to: null,
          website: 'https://darknetdiaries.com/',
          title: 'Darknet Diaries',
          author: 'Jack Rhysider',
          artwork: null,
          podcast_guid: null,
          item_count: 150,
          items_with_media: 150,
          newest_item: '2026-01-01T09:00:00Z',
          provenance: [{ kind: 'fetch', url: null, ok: true, detail: 'parsed as RSS', elapsed_ms: 40 }],
          warnings: [],
        },
      },
    });
    render(Discover, { props: { query: { q: 'https://feeds.example/darknet.xml' } } });

    (await screen.findByRole('button', { name: 'Resolve this URL' })).click();
    expect(await screen.findByText('Resolved feed')).toBeInTheDocument();
    expect(screen.getByText(/150 items · 150 with media/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add to the library' })).toBeInTheDocument();
  });

  describe('OPML import', () => {
    const DOCUMENT =
      '<opml version="2.0"><body><outline text="A" xmlUrl="https://a.example/feed"/>' +
      '<outline text="B" xmlUrl="feed://b.example/rss"/></body></opml>';

    function report(applied: boolean) {
      return {
        body: {
          schema: 1,
          applied,
          counts: {
            add: 1,
            already_present: 0,
            duplicate: 0,
            invalid: 1,
            needs_review: 0,
            conflict: 0,
            failed: 0,
          },
          items: [
            {
              title: 'A',
              xml_url: 'https://a.example/feed',
              action: 'add',
              podcast_id: applied ? '01ARZ3NDEKTSV4RRFFQ69G5FAV' : null,
              detail: null,
            },
            {
              title: 'B',
              xml_url: 'feed://b.example/rss',
              action: 'invalid',
              podcast_id: null,
              detail: 'feed: is not http or https',
            },
          ],
        },
      };
    }

    function base() {
      return {
        '/api/v1/discovery/providers': PROVIDERS,
        '/api/v1/podcasts': { body: EMPTY.podcasts },
      };
    }

    async function choose(): Promise<void> {
      const file = new File([DOCUMENT], 'subscriptions.opml', { type: 'text/x-opml' });
      await userEvent.upload(await screen.findByLabelText('OPML file'), file);
    }

    it('plans an OPML file before adding', async () => {
      const { calls } = stubApi({ ...base(), 'POST /api/v1/podcasts/opml': report(false) });
      render(Discover, { props: { query: {} } });
      await choose();

      expect(await screen.findByText('1 of 2 feeds are new.')).toBeInTheDocument();
      expect(screen.getByText('not a feed URL')).toBeInTheDocument();
      expect(screen.getByText('feed: is not http or https')).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Add 1 podcast' })).toBeInTheDocument();
      const posts = calls.filter((c) => c.method === 'POST');
      expect(posts).toHaveLength(1);
      expect(posts[0]?.body).toEqual({ opml: DOCUMENT, apply: false });
    });

    it('adds with the chosen policy', async () => {
      const { calls } = stubApi({
        ...base(),
        'POST /api/v1/podcasts/opml': [report(false), report(true)],
      });
      render(Discover, { props: { query: {} } });
      await choose();

      await userEvent.selectOptions(
        await screen.findByLabelText('New episodes'),
        'Download automatically',
      );
      await userEvent.click(screen.getByRole('button', { name: 'Add 1 podcast' }));

      expect(await screen.findByText(/Added 1 of 2 feeds/)).toBeInTheDocument();
      expect(screen.getByText('added')).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /^Add \d/ })).not.toBeInTheDocument();
      const posts = calls.filter((c) => c.method === 'POST');
      expect(posts[1]?.body).toEqual({ opml: DOCUMENT, apply: true, policy: { mode: 'auto' } });
    });

    it('reports an unusable OPML file', async () => {
      stubApi({
        ...base(),
        'POST /api/v1/podcasts/opml': {
          status: 400,
          body: {
            schema: 1,
            error: { kind: 'invalid', message: 'this is XML with a <rss> root, not an OPML file' },
          },
        },
      });
      render(Discover, { props: { query: {} } });
      await choose();

      expect(await screen.findByText(/not an OPML file/)).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /^Add \d/ })).not.toBeInTheDocument();
    });
  });
});
