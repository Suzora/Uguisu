import { describe, expect, it, vi } from 'vitest';
import { ApiFailure, onUnauthorized, request, setCsrfToken, url } from './client';
import { stubApi } from '../../tests/harness';

describe('api client', () => {
  it('encodes query parameters', () => {
    expect(url('/search', { q: 'a&b', limit: 5 })).toBe('/api/v1/search?q=a%26b&limit=5');
    expect(url('/podcasts', { state: undefined, podcast: '' })).toBe('/api/v1/podcasts');
  });

  it('returns a parsed body', async () => {
    stubApi({ '/api/v1/health': { body: { status: 'ok', version: '0.1.0' } } });
    await expect(request('/health')).resolves.toEqual({ status: 'ok', version: '0.1.0' });
  });

  it('reads the error envelope', async () => {
    stubApi({
      '/api/v1/podcasts/x': {
        status: 404,
        body: { error: { kind: 'not_found', message: 'podcast `x` not found' } },
      },
    });
    const failure = await request('/podcasts/x').catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ApiFailure);
    expect(failure).toMatchObject({
      failure: 'http',
      status: 404,
      kind: 'not_found',
      message: 'podcast `x` not found',
    });
  });

  it('separates an unavailable engine', async () => {
    stubApi({
      '/api/v1/status': {
        status: 503,
        body: { error: { kind: 'engine_unavailable', message: 'no data directory' } },
      },
    });
    const failure = (await request('/status').catch((e: unknown) => e)) as ApiFailure;
    expect(failure.failure).toBe('unavailable');
    expect(failure.retryable).toBe(true);
  });

  it('reports a conflict as one', async () => {
    stubApi({
      'PUT /api/v1/settings/UGUISU_BIND': {
        status: 409,
        body: { error: { kind: 'conflict', message: 'UGUISU_BIND is set in the environment' } },
      },
    });
    const failure = (await request('/settings/UGUISU_BIND', {
      method: 'PUT',
      body: { value: 'x' },
    }).catch((e: unknown) => e)) as ApiFailure;
    expect(failure.isConflict).toBe(true);
    expect(failure.retryable).toBe(false);
  });

  it('reports an unreachable server', async () => {
    stubApi({ '/api/v1/health': { offline: true } });
    const failure = (await request('/health').catch((e: unknown) => e)) as ApiFailure;
    expect(failure.failure).toBe('network');
    expect(failure.message).toContain('cannot reach the Uguisu API');
  });

  it('reports a body that is not json', async () => {
    stubApi({ '/api/v1/health': { text: '<html>nope</html>' } });
    const failure = (await request('/health').catch((e: unknown) => e)) as ApiFailure;
    expect(failure.failure).toBe('malformed');
  });

  it('keeps a plain-text rejection readable', async () => {
    stubApi({ 'POST /api/v1/podcasts': { status: 415, text: 'Unsupported Media Type' } });
    const failure = (await request('/podcasts', { method: 'POST', body: {} }).catch(
      (e: unknown) => e,
    )) as ApiFailure;
    expect(failure.status).toBe(415);
    expect(failure.message).toBe('Unsupported Media Type');
  });

  it('joins a message with its suggestion', async () => {
    stubApi({
      'POST /api/v1/discovery/resolve': {
        status: 422,
        body: {
          schema: 1,
          error: {
            kind: 'no_feed_available',
            message: 'nothing at that address is a feed',
            suggestion: 'search for the show instead',
          },
        },
      },
    });
    const failure = (await request('/discovery/resolve', {
      method: 'POST',
      body: { input: 'x' },
    }).catch((e: unknown) => e)) as ApiFailure;
    expect(failure.kind).toBe('no_feed_available');
    expect(failure.message).toBe(
      'nothing at that address is a feed — search for the show instead',
    );
  });

  it('honours a caller cancellation', async () => {
    stubApi({ '/api/v1/health': { body: {} } });
    const controller = new AbortController();
    controller.abort();
    const failure = (await request('/health', { signal: controller.signal }).catch(
      (e: unknown) => e,
    )) as ApiFailure;
    expect(failure.failure).toBe('cancelled');
  });

  it('separates a missing credential from a refused one', async () => {
    stubApi({
      '/api/v1/podcasts': {
        status: 401,
        body: { error: { kind: 'unauthenticated', message: 'this request carries no credential' } },
      },
      '/api/v1/settings': {
        status: 403,
        body: { error: { kind: 'forbidden', message: 'this token may only read' } },
      },
    });
    const missing = (await request('/podcasts').catch((e: unknown) => e)) as ApiFailure;
    const refused = (await request('/settings').catch((e: unknown) => e)) as ApiFailure;

    expect(missing.failure).toBe('unauthorized');
    expect(missing.needsLogin).toBe(true);
    expect(missing.retryable).toBe(false);
    expect(refused.failure).toBe('forbidden');
    expect(refused.needsLogin).toBe(false);
  });

  it('carries the wait a rate limit asked for', async () => {
    stubApi({
      '/api/v1/podcasts': {
        status: 429,
        headers: { 'retry-after': '300' },
        body: { error: { kind: 'too_many_requests', message: 'too many failed logins' } },
      },
    });
    const failure = (await request('/podcasts').catch((e: unknown) => e)) as ApiFailure;
    expect(failure.retryAfterSeconds).toBe(300);
  });

  it('tells the shell once a credential is gone', async () => {
    stubApi({
      '/api/v1/podcasts': { status: 401, body: { error: { kind: 'unauthenticated', message: 'no' } } },
      'POST /api/v1/auth/login': {
        status: 401,
        body: { error: { kind: 'unauthenticated', message: 'no' } },
      },
    });
    const seen = vi.fn();
    onUnauthorized(seen);
    try {
      await request('/podcasts').catch(() => {});
      expect(seen).toHaveBeenCalledOnce();
      // A wrong password is the login form's business, not the shell's.
      await request('/auth/login', { method: 'POST', body: {} }).catch(() => {});
      expect(seen).toHaveBeenCalledOnce();
    } finally {
      onUnauthorized(null);
    }
  });

  it('sends the csrf token on a mutation only', async () => {
    setCsrfToken('csrf-1');
    try {
      const api = stubApi({
        '/api/v1/podcasts': { body: { podcasts: [] } },
        'POST /api/v1/podcasts': { body: {} },
      });
      await request('/podcasts');
      await request('/podcasts', { method: 'POST', body: { input: 'x' } });

      expect(api.calls[0]?.headers['x-uguisu-csrf']).toBeUndefined();
      expect(api.calls[1]?.headers['x-uguisu-csrf']).toBe('csrf-1');
    } finally {
      setCsrfToken(null);
    }
  });

  it('gives up after the deadline', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (_input: unknown, init?: RequestInit) =>
          new Promise((_resolve, reject) => {
            init?.signal?.addEventListener('abort', () => reject(new Error('aborted')));
          }),
      ),
    );
    const failure = (await request('/health', { timeoutMs: 5 }).catch(
      (e: unknown) => e,
    )) as ApiFailure;
    expect(failure.failure).toBe('timeout');
  });
});
