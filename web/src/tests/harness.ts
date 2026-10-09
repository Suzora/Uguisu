// Fixtures and fakes the frontend tests share.
//
// Nothing here touches the network: `stubApi` answers a route table, and
// `FakeEventSource` is driven by the test rather than by a server.

import { vi } from 'vitest';

/** One canned answer: a status and a JSON body, or a thrown network error. */
export interface Reply {
  status?: number;
  body?: unknown;
  /** Rejects the `fetch` call, the way an unreachable server does. */
  offline?: boolean;
  /** Returns a body that is not JSON. */
  text?: string;
  /** Extra response headers, e.g. `retry-after` on a 429. */
  headers?: Record<string, string>;
}

export type Routes = Record<string, Reply | Reply[]>;

/** One request the stub saw. */
export interface Call {
  method: string;
  /** The path, including the query string. */
  path: string;
  body: unknown;
  /** Request headers, lowercased, so a test can assert `x-uguisu-csrf`. */
  headers: Record<string, string>;
}

export interface StubbedApi {
  /** Every request made, in order. */
  readonly calls: Call[];
}

/**
 * Replaces `fetch` with a table lookup. A key is matched against the request
 * path without its query string, so `/api/v1/podcasts` covers every filter.
 * An array of replies is consumed one per call, which is how a retry or a
 * reconnect is given different answers.
 */
export function stubApi(routes: Routes): StubbedApi {
  const calls: Call[] = [];
  const queues = new Map<string, Reply[]>(
    // Every view boots inside a shell that asks what this request is, so the
    // session route answers by default; a test that cares overrides it.
    Object.entries({ '/api/v1/auth/session': { body: EMPTY.session }, ...routes }).map(
      ([key, value]) => [key, Array.isArray(value) ? [...value] : [value]],
    ),
  );

  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      // A real `fetch` rejects on an already-aborted signal, and the client's
      // cancellation handling depends on that.
      if (init?.signal?.aborted) {
        throw new DOMException('The operation was aborted.', 'AbortError');
      }
      const href = typeof input === 'string' ? input : input.toString();
      const [path] = href.split('?');
      const method = init?.method ?? 'GET';
      const headers: Record<string, string> = {};
      for (const [name, value] of Object.entries(init?.headers ?? {})) {
        headers[name.toLowerCase()] = String(value);
      }
      calls.push({
        method,
        path: href,
        body: init?.body === undefined ? undefined : JSON.parse(String(init.body)),
        headers,
      });

      const queue = queues.get(`${method} ${path}`) ?? queues.get(path ?? '');
      const reply = queue && (queue.length > 1 ? queue.shift() : queue[0]);
      if (!reply) {
        return new Response(JSON.stringify({ error: { kind: 'not_found', message: `no stub for ${method} ${path}` } }), {
          status: 404,
          headers: { 'content-type': 'application/json' },
        });
      }
      if (reply.offline) {
        throw new TypeError('Failed to fetch');
      }
      if (reply.text !== undefined) {
        return new Response(reply.text, {
          status: reply.status ?? 200,
          headers: { 'content-type': 'text/plain', ...reply.headers },
        });
      }
      const status = reply.status ?? 200;
      // `Response` refuses a body on these, exactly as a real one does, and
      // `logout` answers 204.
      const empty = status === 204 || status === 205 || status === 304;
      return new Response(empty ? null : JSON.stringify(reply.body ?? {}), {
        status,
        headers: { 'content-type': 'application/json', ...reply.headers },
      });
    }),
  );

  return { calls };
}

/** An `EventSource` a test opens, feeds and breaks by hand. */
export class FakeEventSource {
  static instances: FakeEventSource[] = [];

  readonly url: string;
  onopen: (() => void) | null = null;
  closed = false;
  readonly #listeners = new Map<string, ((event: MessageEvent) => void)[]>();

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(kind: string, listener: (event: MessageEvent) => void): void {
    this.#listeners.set(kind, [...(this.#listeners.get(kind) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
  }

  /** Pretends the connection came up. */
  open(): void {
    this.onopen?.();
  }

  /** Delivers one event, the way the server's SSE names it. */
  emit(kind: string, data: unknown): void {
    const message = new MessageEvent(kind, { data: JSON.stringify(data) });
    for (const listener of this.#listeners.get(kind) ?? []) {
      listener(message);
    }
  }

  /** Breaks the connection, which is what triggers a reconnect. */
  fail(): void {
    for (const listener of this.#listeners.get('error') ?? []) {
      listener(new MessageEvent('error'));
    }
  }

  static reset(): void {
    FakeEventSource.instances = [];
  }

  static get latest(): FakeEventSource {
    const last = FakeEventSource.instances.at(-1);
    if (!last) {
      throw new Error('no EventSource was opened');
    }
    return last;
  }
}

/** A podcast as `GET /api/v1/podcasts` returns it. */
export function podcastDetail(overrides: Record<string, unknown> = {}) {
  return {
    podcast: {
      id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
      title: 'Darknet Diaries',
      sort_title: 'darknet diaries',
      subtitle: null,
      author: 'Jack Rhysider',
      publisher: null,
      description_html: null,
      description_text: 'True stories from the dark side of the internet.',
      website: 'https://darknetdiaries.com/',
      artwork_url: null,
      language: 'en',
      categories: ['Technology'],
      explicit: false,
      copyright: null,
      status: 'active',
      refresh_interval_secs: 3600,
      next_refresh_at: '2026-01-01T12:00:00Z',
      last_refresh_at: '2026-01-01T11:00:00Z',
      last_error: null,
      ...(overrides.podcast as object | undefined),
    },
    source: {
      feed_url: 'https://feeds.example/darknet.xml',
      canonical_url: null,
      website_url: null,
      fetch: {
        state: 'fetched',
        consecutive_failures: 0,
        last_http_status: 200,
        last_error_kind: null,
        last_error_detail: null,
        last_attempt_at: '2026-01-01T11:00:00Z',
        last_success_at: '2026-01-01T11:00:00Z',
      },
    },
    episodes_total: 2,
    episodes_present: 2,
    ...overrides,
  };
}

/** An episode as `GET /api/v1/podcasts/{id}/episodes` returns it. */
export function episode(overrides: Record<string, unknown> = {}) {
  return {
    id: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
    podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
    title: 'Episode 1: The Beginning',
    subtitle: null,
    description_text: 'How it started.',
    link: null,
    published_at: '2026-01-01T09:00:00Z',
    duration_secs: 3723,
    season: 1,
    episode_number: 1,
    explicit: false,
    artwork_url: null,
    archive_state: 'expected',
    skip_reason: null,
    duplicate_of_episode_id: null,
    duplicate_reasons: [],
    enclosures: [
      {
        id: '01BX5ZZKBKACTAV9WEVGEMMVR0',
        url: 'https://media.example/1.mp3',
        mime_type: 'audio/mpeg',
        length_bytes: 48_000_000,
        is_primary: true,
        kind: 'audio',
      },
    ],
    ...overrides,
  };
}

/** A download job as the queue returns it. */
export function job(overrides: Record<string, unknown> = {}) {
  return {
    id: '01JOB5ZZKBKACTAV9WEVGEMMVR',
    episode_id: '01BX5ZZKBKACTAV9WEVGEMMVRZ',
    episode_title: 'EP 1: Ear to the Ground',
    podcast_id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
    podcast_title: 'Darknet Diaries',
    source_url: 'https://media.example/1.mp3',
    state: 'downloading',
    state_reason: null,
    priority: 'normal',
    attempt_count: 1,
    max_attempts: 5,
    next_attempt_at: null,
    bytes_downloaded: 12_000_000,
    total_bytes: 48_000_000,
    last_http_status: 200,
    last_error_kind: null,
    last_error_detail: null,
    started_at: '2026-01-01T11:30:00Z',
    finished_at: null,
    created_at: '2026-01-01T11:29:00Z',
    updated_at: '2026-01-01T11:31:00Z',
    ...overrides,
  };
}

/** Empty but well-formed bodies, for routes a view loads incidentally. */
export const EMPTY = {
  session: {
    auth_required: false,
    credential_set: false,
    authenticated: false,
    schema: 1,
  },
  downloadStats: {
    by_state: {},
    running: 0,
    workers_started: true,
    paused_all: null,
    next_retry_at: null,
    orphan_parts: 0,
    schema: 1,
  },
  archiveStats: { by_state: {}, total: 0, schema: 1 },
  jobs: { jobs: [], next_after: null, schema: 1 },
  episodes: { episodes: [], next_after: null, schema: 1 },
  podcasts: { podcasts: [], schema: 1 },
  artwork: { current: null, history: [], schema: 1 },
  events: { events: [], schema: 1 },
  manifests: { manifests: [], schema: 1 },
};

/**
 * Matches an element whose whole text equals `expected` after collapsing
 * whitespace. Needed because Svelte splits interpolated text into separate
 * nodes, which the default matcher looks at one at a time.
 */
export function wholeText(expected: string) {
  return (_content: string, element: Element | null): boolean =>
    element?.textContent?.replace(/\s+/g, ' ').trim() === expected;
}

/** Installs {@link FakeEventSource} as the global `EventSource`. */
export function stubEventSource(): void {
  FakeEventSource.reset();
  vi.stubGlobal('EventSource', FakeEventSource);
}
