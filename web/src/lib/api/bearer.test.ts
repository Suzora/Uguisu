// The browser authenticates with its session cookie and nothing else: no
// request it makes carries `Authorization` (Phase 9's acceptance). The one
// exception is the desktop shell's exchange of its launch credential, which
// `desktop.test.ts` covers.
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as api from './index';
import { stubApi } from '../../tests/harness';

/** Every script under `src/` that ships, by path, as it is on disk. */
const SOURCES = import.meta.glob<string>(['../../**/*.ts', '../../**/*.svelte', '!../../**/*.test.ts', '!../../tests/**'], {
  query: '?raw',
  import: 'default',
  eager: true,
});

/** Functions that build a URL or handle state, and the exchange itself. */
const NOT_A_REQUEST = new Set([
  'ApiFailure',
  'artworkImageUrl',
  'eventStreamUrl',
  'exchange',
  'mediaUrl',
  'messageFor',
  'onUnauthorized',
  'opmlExportUrl',
  'setCsrfToken',
  'url',
]);

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('no bearer from the browser', () => {
  it('one call site sets one', () => {
    const setters = Object.entries(SOURCES).filter(([, text]) => /\bbearer:/.test(text));
    expect(setters.map(([file]) => file)).toEqual(['./index.ts']);
    const headers = Object.entries(SOURCES).filter(([, text]) => /['"]authorization['"]/i.test(text));
    expect(headers.map(([file]) => file)).toEqual(['./client.ts']);
  });

  it('no request carries authorization', async () => {
    const stub = stubApi({});
    const requests = Object.entries(api).filter(
      ([name, value]) => typeof value === 'function' && !NOT_A_REQUEST.has(name),
    );
    const id = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
    for (const [name, call] of requests) {
      const before = stub.calls.length;
      try {
        await (call as (...args: unknown[]) => Promise<unknown>)(id, id, id, id);
      } catch {
        // Every answer is a stubbed 404; only what was sent matters here.
      }
      expect(stub.calls.length, `${name} made no request`).toBeGreaterThan(before);
    }
    expect(requests.length).toBeGreaterThan(40);
    for (const call of stub.calls) {
      expect(call.headers.authorization, `${call.method} ${call.path}`).toBeUndefined();
    }
  });
});
