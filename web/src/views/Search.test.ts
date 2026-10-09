import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Search from './Search.svelte';
import { stubApi } from '../tests/harness';

function results(overrides: Record<string, unknown>) {
  return {
    body: {
      outcome: 'ok',
      terms: ['rust'],
      truncated: false,
      podcasts: [],
      episodes: [],
      index: {
        state: 'ready',
        built_at: '2026-01-01T10:00:00Z',
        podcasts: 3,
        episodes: 120,
        detail: null,
        updated_at: '2026-01-01T10:00:00Z',
      },
      duration_ms: 4,
      schema: 1,
      ...overrides,
    },
  };
}

describe('local search', () => {
  it('asks for something before searching', async () => {
    stubApi({ '/api/v1/search': results({ outcome: 'empty_query', terms: [] }) });
    render(Search, { props: { query: {} } });
    expect(await screen.findByText('Type something to search')).toBeInTheDocument();
    expect(screen.getByText(/120 episodes across 3 podcasts/)).toBeInTheDocument();
  });

  it('says the index is building, not that nothing matched', async () => {
    stubApi({
      '/api/v1/search': results({
        outcome: 'index_building',
        index: {
          state: 'building',
          built_at: null,
          podcasts: 1,
          episodes: 42,
          detail: '42 of 900',
          updated_at: '2026-01-01T10:00:00Z',
        },
      }),
    });
    render(Search, { props: { query: { q: 'rust' } } });
    expect(await screen.findByText('The search index is still being built')).toBeInTheDocument();
    expect(screen.getByText(/42 episodes indexed so far/)).toBeInTheDocument();
    expect(screen.queryByText(/Nothing matched/)).not.toBeInTheDocument();
  });

  it('offers a rebuild for a stale index', async () => {
    stubApi({
      '/api/v1/search': results({
        outcome: 'index_stale',
        index: {
          state: 'stale',
          built_at: null,
          podcasts: 0,
          episodes: 0,
          detail: null,
          updated_at: '2026-01-01T10:00:00Z',
        },
      }),
    });
    render(Search, { props: { query: { q: 'rust' } } });
    expect(await screen.findByText('The search index is out of date')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Rebuild the index' })).toBeInTheDocument();
  });

  it('names the terms that matched nothing', async () => {
    stubApi({ '/api/v1/search': results({ outcome: 'no_results', terms: ['zebra'] }) });
    render(Search, { props: { query: { q: 'zebra' } } });
    expect(await screen.findByText('Nothing matched “zebra”')).toBeInTheDocument();
  });

  it('links an episode hit to its page', async () => {
    stubApi({
      '/api/v1/search': results({
        episodes: [
          {
            episode_id: '01EP',
            podcast_id: '01POD',
            podcast_title: 'Darknet Diaries',
            title: 'Rust in anger',
            snippet: 'about rust',
            published_at: '2026-01-01T09:00:00Z',
            duration_secs: 60,
            archive_state: 'archived',
            relevance: 1,
            score: 2.5,
            signals: [{ name: 'title', weight: 1, value: 1, contribution: 2, note: 'title match' }],
          },
        ],
      }),
    });
    render(Search, { props: { query: { q: 'rust' } } });
    const link = await screen.findByRole('link', { name: 'Rust in anger' });
    expect(link).toHaveAttribute('href', '/episodes/01EP');
    expect(screen.getByRole('link', { name: 'Darknet Diaries' })).toHaveAttribute('href', '/podcasts/01POD');
    expect(screen.getByText('archived')).toBeInTheDocument();
  });

  it('explains a score on request', async () => {
    stubApi({
      '/api/v1/search': results({
        podcasts: [
          {
            podcast_id: '01POD',
            title: 'Darknet Diaries',
            author: 'Jack',
            snippet: 'about rust',
            relevance: 1,
            score: 3.25,
            signals: [{ name: 'title', weight: 2, value: 1, contribution: 2, note: 'exact title' }],
          },
        ],
      }),
    });
    render(Search, { props: { query: { q: 'darknet' } } });
    const toggle = await screen.findByRole('button', { name: 'score 3.25' });
    expect(screen.queryByText(/exact title/)).not.toBeInTheDocument();
    toggle.click();
    expect(await screen.findByText(/exact title/)).toBeInTheDocument();
  });
});
