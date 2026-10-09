// The one place the UI talks to the API.
//
// Every request goes through `request()`: it builds the URL, applies a
// deadline, parses the body once, and turns everything that can go wrong into
// an `ApiFailure` the caller can branch on. Components never call `fetch`.

import { m } from '../i18n';

/** Where the API lives. Same origin in production, proxied in `pnpm dev`. */
export const API_BASE = '/api/v1';

/** How long a request may take before the client gives up. */
export const DEFAULT_TIMEOUT_MS = 15_000;

/**
 * What went wrong, as a closed set the UI can render differently.
 *
 * `unavailable` is separate from `http` because a server that is up but has
 * no library needs a different message from one that rejected the request.
 */
export type FailureKind =
  | 'network'
  | 'timeout'
  | 'cancelled'
  | 'http'
  | 'unauthorized'
  | 'forbidden'
  | 'unavailable'
  | 'malformed';

export class ApiFailure extends Error {
  readonly failure: FailureKind;
  /** HTTP status, or `null` when no response arrived. */
  readonly status: number | null;
  /** The server's stable `error.kind`, when it sent an error envelope. */
  readonly kind: string | null;
  /** `Retry-After` on a 429, in seconds, when the server sent one. */
  readonly retryAfterSeconds: number | null;

  constructor(
    failure: FailureKind,
    message: string,
    status: number | null,
    kind: string | null,
    retryAfterSeconds: number | null = null,
  ) {
    super(message);
    this.name = 'ApiFailure';
    this.failure = failure;
    this.status = status;
    this.kind = kind;
    this.retryAfterSeconds = retryAfterSeconds;
  }

  /** Whether retrying the same request could plausibly succeed. */
  get retryable(): boolean {
    return (
      this.failure === 'network' ||
      this.failure === 'timeout' ||
      this.failure === 'unavailable' ||
      (this.status !== null && this.status >= 500)
    );
  }

  /** A conflict the user can act on: a pinned setting, a refused transition. */
  get isConflict(): boolean {
    return this.status === 409;
  }

  /** Whether the shell should show the login view instead of this failure. */
  get needsLogin(): boolean {
    return this.failure === 'unauthorized';
  }
}

/**
 * The session's CSRF token, held in memory only.
 *
 * A mutating request made with the session cookie must echo it (the server
 * answers `403 csrf_required` otherwise). It never goes into `localStorage`:
 * a token that outlives the tab is a token an attacker can read later, and the
 * shell reads it back from `GET /auth/session` on every boot anyway.
 */
let csrfToken: string | null = null;

/** Records the token a login or a session read returned. */
export function setCsrfToken(token: string | null): void {
  csrfToken = token;
}

/**
 * What to do when a request is refused for want of a credential.
 *
 * One handler, registered by the shell, so a session that expired mid-visit
 * puts every view behind the login form instead of each view rendering its own
 * 401. The `/auth/` routes are exempt: a wrong password is a 401 the login form
 * itself reports.
 */
let unauthorizedHandler: (() => void) | null = null;

/** Registers the shell's reaction to a 401; `null` removes it. */
export function onUnauthorized(handler: (() => void) | null): void {
  unauthorizedHandler = handler;
}

export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE';
  /** JSON request body. */
  body?: unknown;
  /** Query parameters; `undefined` and `null` entries are dropped. */
  query?: Record<string, string | number | boolean | undefined | null>;
  /** Caller cancellation, e.g. a superseded keystroke. */
  signal?: AbortSignal;
  /** Overrides {@link DEFAULT_TIMEOUT_MS}; `0` disables the deadline. */
  timeoutMs?: number;
  /**
   * A bearer credential for this one request.
   *
   * Deliberately not a general `headers` map: exactly one caller sets this,
   * the desktop bootstrap in {@link ../desktop}, and nothing else in the app
   * can attach an `Authorization` header. Every other request authenticates
   * with the session cookie the bootstrap obtains.
   */
  bearer?: string;
}

/** Builds a URL under {@link API_BASE} with encoded query parameters. */
export function url(path: string, query?: RequestOptions['query']): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query ?? {})) {
    if (value !== undefined && value !== null && value !== '') {
      search.set(key, String(value));
    }
  }
  const suffix = search.toString();
  return `${API_BASE}${path}${suffix ? `?${suffix}` : ''}`;
}

function failureFor(
  status: number,
  kind: string | null,
  message: string,
  retryAfterSeconds: number | null,
): ApiFailure {
  if (status === 401) {
    return new ApiFailure('unauthorized', message, status, kind);
  }
  if (status === 403) {
    return new ApiFailure('forbidden', message, status, kind);
  }
  if (status === 503) {
    return new ApiFailure('unavailable', message, status, kind);
  }
  return new ApiFailure('http', message, status, kind, retryAfterSeconds);
}

/** `Retry-After` as seconds; the HTTP-date form is not used by this API. */
function retryAfter(response: Response): number | null {
  const header = response.headers.get('retry-after');
  if (header === null) {
    return null;
  }
  const seconds = Number.parseInt(header, 10);
  return Number.isFinite(seconds) ? seconds : null;
}

/**
 * Reads the `{ error: { kind, message, suggestion? } }` envelope, or falls
 * back to text.
 *
 * Every route answers this one shape, `/discovery/resolve` included. Where it
 * offers a `suggestion`, the two are joined so a rendered message says both
 * what went wrong and what to do about it.
 */
async function errorFrom(response: Response): Promise<ApiFailure> {
  const wait = retryAfter(response);
  const text = await response.text().catch(() => '');
  try {
    const body: unknown = JSON.parse(text);
    if (
      typeof body === 'object' &&
      body !== null &&
      'error' in body &&
      typeof (body as { error: unknown }).error === 'object'
    ) {
      const envelope = (body as {
        error: { kind?: unknown; message?: unknown; suggestion?: unknown };
      }).error;
      const kind = typeof envelope.kind === 'string' ? envelope.kind : null;
      const parts = [envelope.message, envelope.suggestion].filter(
        (part): part is string => typeof part === 'string' && part !== '',
      );
      const message = parts.length > 0 ? parts.join(' — ') : response.statusText;
      return failureFor(response.status, kind, message, wait);
    }
  } catch {
    // Not JSON: a response that never reached a handler at all.
  }
  const message = text.trim() || `${response.status} ${response.statusText}`;
  return failureFor(response.status, null, message, wait);
}

/**
 * Performs one API request and returns its parsed body.
 *
 * Throws {@link ApiFailure} for every outcome that is not a parsed success,
 * so no caller can mistake an error for data.
 */
export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const deadline = timeoutMs > 0 ? AbortSignal.timeout(timeoutMs) : undefined;
  const signals = [options.signal, deadline].filter((s): s is AbortSignal => s !== undefined);
  const signal = signals.length > 1 ? AbortSignal.any(signals) : signals[0];

  const method = options.method ?? 'GET';
  const headers: Record<string, string> = { accept: 'application/json' };
  if (options.body !== undefined) {
    headers['content-type'] = 'application/json';
  }
  // A bearer principal is exempt server-side, and the browser never sends one.
  if (method !== 'GET' && csrfToken !== null) {
    headers['x-uguisu-csrf'] = csrfToken;
  }
  if (options.bearer !== undefined) {
    headers['authorization'] = `Bearer ${options.bearer}`;
  }

  let response: Response;
  try {
    response = await fetch(url(path, options.query), {
      method,
      headers,
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
      signal,
    });
  } catch (cause) {
    if (options.signal?.aborted) {
      throw new ApiFailure('cancelled', 'request cancelled', null, null);
    }
    if (deadline?.aborted) {
      throw new ApiFailure('timeout', `no answer within ${timeoutMs} ms`, null, null);
    }
    throw new ApiFailure('network', reasonFor(cause), null, null);
  }

  if (!response.ok) {
    const failure = await errorFrom(response);
    if (failure.needsLogin && !path.startsWith('/auth/')) {
      setCsrfToken(null);
      unauthorizedHandler?.();
    }
    throw failure;
  }
  if (response.status === 204) {
    return undefined as T;
  }
  try {
    return (await response.json()) as T;
  } catch {
    throw new ApiFailure('malformed', m.common.api.notJson, response.status, null);
  }
}

function reasonFor(cause: unknown): string {
  const detail = cause instanceof Error ? cause.message : String(cause);
  return m.common.api.unreachable(detail);
}

/** Asserts that `body[field]` is an array, so a shape change is not silent. */
export function expectArray<T>(body: unknown, field: string): T[] {
  const value = (body as Record<string, unknown> | null)?.[field];
  if (!Array.isArray(value)) {
    throw new ApiFailure('malformed', `the response has no \`${field}\` array`, 200, null);
  }
  return value as T[];
}

/** A human sentence for any failure, used wherever an error is rendered. */
export function messageFor(error: unknown): string {
  if (error instanceof ApiFailure) {
    return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}
