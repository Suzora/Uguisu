import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import ArchiveImport from './ArchiveImport.svelte';
import { EMPTY, stubApi } from '../tests/harness';

const ITEM = {
  source_path: 'Show/2025-09-01 - Episode.mp3',
  size_bytes: 2048,
  action: 'import',
  podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  episode_id: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
  confidence: 92,
  matched_by: 'scored',
  target_path: 'Show/2025-09-01 Episode.mp3',
  detail: null,
};

function plan(applied: boolean) {
  return {
    applied,
    source_root: '/srv/podgrab/assets',
    format: 'podgrab',
    threshold: 80,
    scanned: 2,
    imported: 1,
    already_present: 0,
    conflicts: 0,
    ambiguous: 0,
    unmatched: 1,
    invalid: 0,
    unreadable: 0,
    items: [
      ITEM,
      { ...ITEM, source_path: 'Show/unknown.mp3', action: 'unmatched', episode_id: null, target_path: null, confidence: 31, detail: 'no episode explains the file well enough' },
    ],
  };
}

async function readFolder(path: string): Promise<void> {
  await fireEvent.input(screen.getByLabelText('Folder on the server'), { target: { value: path } });
  await fireEvent.click(screen.getByRole('button', { name: 'Read the folder' }));
}

describe('archive import view', () => {
  it('copies with the planned request', async () => {
    const api = stubApi({
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/archive/import': [{ body: plan(false) }, { body: plan(true) }],
    });
    render(ArchiveImport);
    await readFolder('/srv/podgrab/assets');

    expect(await screen.findByText('2 files read; 1 can be copied.')).toBeInTheDocument();
    expect(screen.getByText('no episode explains the file well enough')).toBeInTheDocument();
    const [read] = api.calls.filter((c) => c.method === 'POST');
    // The path travels in the body, never in the URL.
    expect(read?.path).toBe('/api/v1/archive/import');
    expect(read?.body).toEqual({ path: '/srv/podgrab/assets', format: null, podgrab_db: null, podcast: null, apply: false });

    await fireEvent.click(screen.getByRole('button', { name: 'Copy 1 file' }));
    expect(await screen.findByText('1 file copied.')).toBeInTheDocument();
    const posts = api.calls.filter((c) => c.method === 'POST');
    expect(posts).toHaveLength(2);
    expect(posts[1]?.body).toEqual({ ...(read?.body as object), apply: true });
    expect(screen.getByRole('link', { name: 'Check the copies on the Archive page' })).toHaveAttribute(
      'href',
      '/archive?state=unchecked',
    );
  });

  it('narrows the plan to one outcome', async () => {
    stubApi({
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/archive/import': { body: plan(false) },
    });
    render(ArchiveImport);
    await readFolder('/srv/podgrab/assets');
    await screen.findByText('Show/unknown.mp3');
    await fireEvent.click(screen.getAllByRole('button').find((b) => b.textContent?.includes('unmatched'))!);
    await waitFor(() => expect(screen.queryByText('Show/2025-09-01 - Episode.mp3')).not.toBeInTheDocument());
    expect(screen.getByText('Show/unknown.mp3')).toBeInTheDocument();
  });

  it('shows the server refusal', async () => {
    stubApi({
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/archive/import': {
        status: 400,
        body: { error: { kind: 'import_source_invalid', message: '/nowhere is not a directory' } },
      },
    });
    render(ArchiveImport);
    await readFolder('/nowhere');
    expect(await screen.findByText('/nowhere is not a directory')).toBeInTheDocument();
  });

  it('lost copy invites a rerun', async () => {
    stubApi({
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/archive/import': [{ body: plan(false) }, { offline: true }],
    });
    render(ArchiveImport);
    await readFolder('/srv/podgrab/assets');
    await fireEvent.click(await screen.findByRole('button', { name: 'Copy 1 file' }));
    expect(
      await screen.findByText(
        'cannot reach the Uguisu API: Failed to fetch. What was copied stays; running it again continues with the rest.',
      ),
    ).toBeInTheDocument();
  });

  it('a refused copy is not lost', async () => {
    stubApi({
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/archive/import': [
        { body: plan(false) },
        { status: 400, body: { error: { kind: 'import_source_invalid', message: 'podgrab.db is in use' } } },
      ],
    });
    render(ArchiveImport);
    await readFolder('/srv/podgrab/assets');
    await fireEvent.click(await screen.findByRole('button', { name: 'Copy 1 file' }));
    expect(await screen.findByText('podgrab.db is in use')).toBeInTheDocument();
    expect(screen.queryByText(/running it again continues/)).not.toBeInTheDocument();
  });
});
