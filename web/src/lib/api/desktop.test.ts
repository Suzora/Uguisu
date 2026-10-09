import { afterEach, describe, expect, it, vi } from 'vitest';
import { bootstrapSession, inDesktopShell } from './desktop';
import { stubApi } from '../../tests/harness';

/** Installs a fake Tauri global whose one command answers `secret`. */
function shell(secret: () => Promise<unknown>) {
  const invoke = vi.fn(secret);
  vi.stubGlobal('__TAURI__', { core: { invoke } });
  return invoke;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('desktop bootstrap', () => {
  it('is inert in a browser', async () => {
    const api = stubApi({});
    expect(inDesktopShell()).toBe(false);
    await expect(bootstrapSession()).resolves.toBe(false);
    expect(api.calls.filter((c) => c.path.includes('/auth/exchange'))).toHaveLength(0);
  });

  it('exchanges the launch credential for a session', async () => {
    const invoke = shell(() => Promise.resolve('launch-secret'));
    const api = stubApi({
      'POST /api/v1/auth/exchange': {
        status: 201,
        body: { csrf_token: 'csrf-from-exchange', expires_at: '2026-10-22T00:00:00Z', schema: 1 },
      },
      'POST /api/v1/podcasts': { status: 201, body: { podcast: { id: '1' }, schema: 1 } },
    });

    expect(inDesktopShell()).toBe(true);
    await expect(bootstrapSession()).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith('launch_credential');

    const exchange = api.calls.find((c) => c.path.includes('/auth/exchange'));
    expect(exchange?.headers.authorization).toBe('Bearer launch-secret');

    // The credential buys the session and nothing else: the next request is an
    // ordinary cookie-and-CSRF one, with no bearer anywhere near it.
    const { addPodcast } = await import('./index');
    await addPodcast('https://example.com/feed.xml');
    const added = api.calls.find((c) => c.path.endsWith('/podcasts'));
    expect(added?.headers.authorization).toBeUndefined();
    expect(added?.headers['x-uguisu-csrf']).toBe('csrf-from-exchange');
  });

  it('gives up when the credential is already spent', async () => {
    shell(() => Promise.reject(new Error('the launch credential has already been used')));
    const api = stubApi({});
    await expect(bootstrapSession()).resolves.toBe(false);
    expect(api.calls.filter((c) => c.path.includes('/auth/exchange'))).toHaveLength(0);
  });

  it('gives up when the server refuses the credential', async () => {
    shell(() => Promise.resolve('stale-secret'));
    stubApi({
      'POST /api/v1/auth/exchange': {
        status: 403,
        body: { error: { kind: 'forbidden', message: 'not from this machine' }, schema: 1 },
      },
    });
    await expect(bootstrapSession()).resolves.toBe(false);
  });
});
