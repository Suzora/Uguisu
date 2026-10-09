import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import ArchiveRepair from './ArchiveRepair.svelte';
import { EMPTY, job, stubApi } from '../tests/harness';

const PODCAST = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
const EPISODE = '01BX5ZZKBKACTAV9WEVGEMMVRZ';

function missing() {
  return {
    id: '01C0000000000000000000000A',
    episode_id: EPISODE,
    podcast_id: PODCAST,
    relative_path: 'Show/2025-09-01 Episode.mp3',
    size_bytes: 2048,
    content_type: 'audio/mpeg',
    sniffed_type: null,
    hash_algo: 'sha256',
    hash_value: 'ab'.repeat(32),
    origin: 'download',
    verification_state: 'missing',
    verification_reason: 'not_found',
    verified_at: '2026-10-01T10:00:00Z',
    tag_state: 'untagged',
    sidecar_written_at: null,
    registered_at: '2026-09-01T10:00:00Z',
    created_at: '2026-09-01T10:00:00Z',
    updated_at: '2026-10-01T10:00:00Z',
  };
}

function plan(applied: boolean) {
  return {
    applied,
    source_root: '/mnt/backup',
    scanned: 3,
    items: [
      {
        episode_id: EPISODE,
        podcast_id: PODCAST,
        target_path: 'Show/2025-09-01 Episode.mp3',
        action: 'restore',
        source_path: 'old/episode.mp3',
        detail: null,
      },
    ],
  };
}

const LISTS = {
  '/api/v1/podcasts': { body: EMPTY.podcasts },
  '/api/v1/archive': { body: { files: [missing()], next_after: null, schema: 1 } },
};

describe('archive repair view', () => {
  it('lists what is missing', async () => {
    const api = stubApi(LISTS);
    render(ArchiveRepair, { props: { query: {} } });
    expect(await screen.findByText('1 file is missing.')).toBeInTheDocument();
    expect(screen.getByText('Show/2025-09-01 Episode.mp3')).toBeInTheDocument();
    expect(api.calls.find((c) => c.path.startsWith('/api/v1/archive?'))?.path).toContain('state=missing');
  });

  it('restores from a folder, then applies', async () => {
    const api = stubApi({ ...LISTS, '/api/v1/archive/restore': [{ body: plan(false) }, { body: plan(true) }] });
    render(ArchiveRepair, { props: { query: {} } });
    await screen.findByText('1 file is missing.');
    await fireEvent.input(screen.getByLabelText('Folder on the server'), { target: { value: '/mnt/backup' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Look in the folder' }));

    expect(await screen.findByText('3 files looked at; 1 can be put back.')).toBeInTheDocument();
    expect(screen.getByText('old/episode.mp3')).toBeInTheDocument();
    const [looked] = api.calls.filter((c) => c.method === 'POST');
    // The path travels in the body, never in the URL.
    expect(looked?.body).toEqual({ path: '/mnt/backup', podcast: null, apply: false });

    await fireEvent.click(screen.getByRole('button', { name: 'Put 1 file back' }));
    expect(await screen.findByText('1 file put back and verified.')).toBeInTheDocument();
    const posts = api.calls.filter((c) => c.method === 'POST');
    expect(posts[1]?.body).toEqual({ path: '/mnt/backup', podcast: null, apply: true });
  });

  it('downloads one file again', async () => {
    const api = stubApi({
      ...LISTS,
      [`/api/v1/archive/${EPISODE}/redownload`]: { body: { outcome: 'requeued', job: job() } },
    });
    render(ArchiveRepair, { props: { query: {} } });
    await fireEvent.click(await screen.findByRole('button', { name: 'Download again' }));
    expect(await screen.findByRole('link', { name: 'Follow them on the Downloads page' })).toBeInTheDocument();
    expect(api.calls.some((c) => c.method === 'POST' && c.path === `/api/v1/archive/${EPISODE}/redownload`)).toBe(true);
    expect(screen.queryByRole('button', { name: 'Download again' })).not.toBeInTheDocument();
  });

  it('downloads the rest past a refusal', async () => {
    const other = { ...missing(), id: '01C0000000000000000000000B', episode_id: '01BX5ZZKBKACTAV9WEVGEMMVS0', relative_path: 'Show/other.mp3' };
    const api = stubApi({
      '/api/v1/podcasts': { body: EMPTY.podcasts },
      '/api/v1/archive': { body: { files: [missing(), other], next_after: null, schema: 1 } },
      [`/api/v1/archive/${EPISODE}/redownload`]: {
        status: 409,
        body: { error: { kind: 'conflict', message: 'still there' } },
      },
      '/api/v1/archive/01BX5ZZKBKACTAV9WEVGEMMVS0/redownload': { body: { outcome: 'requeued', job: job() } },
    });
    render(ArchiveRepair, { props: { query: {} } });
    await fireEvent.click(await screen.findByRole('button', { name: 'Download the 2 listed files again' }));
    expect(
      await screen.findByText('1 download queued; 1 refused: Show/2025-09-01 Episode.mp3: still there.'),
    ).toBeInTheDocument();
    expect(api.calls.some((c) => c.path === '/api/v1/archive/01BX5ZZKBKACTAV9WEVGEMMVS0/redownload')).toBe(true);
  });

  it('shows why a download is refused', async () => {
    stubApi({
      ...LISTS,
      [`/api/v1/archive/${EPISODE}/redownload`]: {
        status: 409,
        body: { error: { kind: 'conflict', message: 'Show/2025-09-01 Episode.mp3 is still there, or cannot be checked' } },
      },
    });
    render(ArchiveRepair, { props: { query: {} } });
    await fireEvent.click(await screen.findByRole('button', { name: 'Download again' }));
    expect(await screen.findByText(/is still there, or cannot be checked/)).toBeInTheDocument();
  });
});
