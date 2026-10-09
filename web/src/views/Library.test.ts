import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import Library from './Library.svelte';
import { EMPTY, podcastDetail, stubApi } from '../tests/harness';

describe('library view', () => {
  it('shows a podcast with its counts', async () => {
    stubApi({
      '/api/v1/podcasts': { body: { podcasts: [podcastDetail()], schema: 1 } },
      '/api/v1/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV/artwork': { body: EMPTY.artwork },
    });
    render(Library, { props: { query: {} } });

    expect(await screen.findByRole('link', { name: 'Darknet Diaries' })).toHaveAttribute(
      'href',
      '/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV',
    );
    expect(screen.getByText('2 episodes')).toBeInTheDocument();
    expect(screen.getByText('active')).toBeInTheDocument();
  });

  it('loads more with the cursor', async () => {
    const second = podcastDetail({
      podcast: {
        id: '01OTHER3NDEKTSV4RRFFQ69G5F',
        title: 'Reply All',
        sort_title: 'reply all',
        author: 'Gimlet',
        status: 'active',
        categories: [],
      },
    });
    const api = stubApi({
      '/api/v1/podcasts': [
        { body: { podcasts: [podcastDetail()], next_after: '01ARZ3NDEKTSV4RRFFQ69G5FAV', schema: 1 } },
        { body: { podcasts: [second], next_after: null, schema: 1 } },
      ],
      '/api/v1/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV/artwork': { body: EMPTY.artwork },
      '/api/v1/podcasts/01OTHER3NDEKTSV4RRFFQ69G5F/artwork': { body: EMPTY.artwork },
    });
    render(Library, { props: { query: { sort: 'episodes' } } });

    expect(await screen.findByText('Darknet Diaries')).toBeInTheDocument();
    expect(screen.queryByText('Reply All')).not.toBeInTheDocument();
    screen.getByRole('button', { name: 'Load more' }).click();

    expect(await screen.findByText('Reply All')).toBeInTheDocument();
    expect(screen.getByText('Darknet Diaries')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
    const lists = api.calls.filter((c) => c.path.startsWith('/api/v1/podcasts?'));
    expect(lists.map((c) => c.path)).toEqual([
      '/api/v1/podcasts?sort=episodes&limit=48',
      '/api/v1/podcasts?sort=episodes&after=01ARZ3NDEKTSV4RRFFQ69G5FAV&limit=48',
    ]);
  });

  it('marks a podcast whose feed announces a move', async () => {
    stubApi({
      '/api/v1/podcasts': {
        body: {
          podcasts: [podcastDetail({ announced: { feed_url: 'https://new.example/feed.xml' } })],
          schema: 1,
        },
      },
      '/api/v1/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV/artwork': { body: EMPTY.artwork },
    });
    render(Library, { props: { query: {} } });
    expect(await screen.findByText('new feed announced')).toBeInTheDocument();
  });

  it('says an empty library is empty', async () => {
    stubApi({ '/api/v1/podcasts': { body: EMPTY.podcasts } });
    render(Library, { props: { query: {} } });
    expect(await screen.findByText('No podcasts yet')).toBeInTheDocument();
  });

  it('separates a failure from emptiness', async () => {
    stubApi({ '/api/v1/podcasts': { offline: true } });
    render(Library, { props: { query: {} } });
    expect(await screen.findByRole('alert')).toHaveTextContent('The library could not be read');
    expect(screen.queryByText('No podcasts yet')).not.toBeInTheDocument();
  });

  it('asks the server for the filter', async () => {
    const api = stubApi({
      '/api/v1/podcasts': { body: { podcasts: [podcastDetail()], schema: 1 } },
      '/api/v1/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV/artwork': { body: EMPTY.artwork },
    });
    render(Library, { props: { query: { q: ' dark ', status: 'active', sort: 'refreshed' } } });

    expect(await screen.findByText('Darknet Diaries')).toBeInTheDocument();
    const lists = api.calls.filter((c) => c.path.startsWith('/api/v1/podcasts?'));
    expect(lists.map((c) => c.path)).toEqual([
      '/api/v1/podcasts?q=dark&status=active&sort=refreshed&limit=48',
    ]);
  });

  it('falls back to the title sort', async () => {
    const api = stubApi({ '/api/v1/podcasts': { body: EMPTY.podcasts } });
    render(Library, { props: { query: { sort: 'sideways' } } });

    expect(await screen.findByText('No podcasts yet')).toBeInTheDocument();
    expect(api.calls.filter((c) => c.path.startsWith('/api/v1/podcasts?')).map((c) => c.path)).toEqual([
      '/api/v1/podcasts?sort=title&limit=48',
    ]);
    expect(screen.getByRole('combobox', { name: 'Sort' })).toHaveValue('title');
  });

  it('says a filter matched nothing', async () => {
    stubApi({ '/api/v1/podcasts': { body: EMPTY.podcasts } });
    render(Library, { props: { query: { status: 'paused' } } });
    await waitFor(() =>
      expect(screen.getByText('No podcast matches this filter')).toBeInTheDocument(),
    );
  });

  it('offers the library as OPML', async () => {
    stubApi({ '/api/v1/podcasts': { body: EMPTY.podcasts } });
    render(Library, { props: { query: {} } });

    const link = await screen.findByRole('link', { name: 'Export OPML' });
    expect(link).toHaveAttribute('href', '/api/v1/podcasts/opml');
    expect(link).toHaveAttribute('download', 'uguisu.opml');
  });
});
