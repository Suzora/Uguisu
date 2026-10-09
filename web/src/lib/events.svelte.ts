// One SSE connection for the whole application (ADR 0033).
//
// Views subscribe to the kinds they care about; they never open a stream
// themselves, so a tab has exactly one connection however many views are
// mounted. The stream is an optimisation: every reconnect tells subscribers
// to re-read the API, because `download.progress` is transient and a gap in
// the stream is otherwise invisible.

import { eventStreamUrl } from './api';
import type { UguisuEvent } from './api';

/** How long to wait before reconnect attempt `n`, capped at 30 s. */
export function backoffMs(attempt: number): number {
  return Math.min(30_000, 500 * 2 ** Math.min(attempt, 6));
}

export type ConnectionState = 'connecting' | 'open' | 'retrying' | 'closed';

type Listener = (event: UguisuEvent) => void;
type ResyncListener = () => void;

/** A stream source, so tests can drive one without a network. */
export interface StreamFactory {
  (url: string): EventSource;
}

class EventStream {
  #state = $state<ConnectionState>('closed');
  #lastEventId = $state<string | null>(null);
  #attempt = 0;
  #source: EventSource | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #listeners = new Set<Listener>();
  #resync = new Set<ResyncListener>();
  #open = false;
  readonly #factory: StreamFactory;

  constructor(factory: StreamFactory = (url) => new EventSource(url)) {
    this.#factory = factory;
  }

  /** Whether the browser is receiving events right now. */
  get state(): ConnectionState {
    return this.#state;
  }

  /** The id of the newest event seen, for diagnostics and replay. */
  get lastEventId(): string | null {
    return this.#lastEventId;
  }

  /** Opens the stream. Calling it again while open does nothing. */
  start(): void {
    if (this.#open) {
      return;
    }
    this.#open = true;
    this.#connect();
  }

  /** Closes the stream and cancels any pending reconnect. */
  stop(): void {
    this.#open = false;
    this.#clearTimer();
    this.#source?.close();
    this.#source = null;
    this.#state = 'closed';
  }

  /** Receives every event until the returned function is called. */
  subscribe(listener: Listener): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * Runs after every successful (re)connect. A view uses it to re-read the
   * API, because events that arrived while the stream was down are lost.
   */
  onResync(listener: ResyncListener): () => void {
    this.#resync.add(listener);
    return () => {
      this.#resync.delete(listener);
    };
  }

  #connect(): void {
    this.#clearTimer();
    this.#source?.close();
    this.#state = this.#attempt === 0 ? 'connecting' : 'retrying';

    const source = this.#factory(eventStreamUrl());
    this.#source = source;

    source.onopen = () => {
      this.#attempt = 0;
      this.#state = 'open';
      for (const listener of this.#resync) {
        listener();
      }
    };
    source.addEventListener('error', () => {
      if (!this.#open) {
        return;
      }
      this.#state = 'retrying';
      source.close();
      const delay = backoffMs(this.#attempt);
      this.#attempt += 1;
      this.#timer = setTimeout(() => this.#connect(), delay);
    });
    // Every event carries its dotted kind as the SSE event name, so the
    // default `message` handler never fires and each name needs its own
    // listener.
    for (const kind of KINDS) {
      source.addEventListener(kind, (message) => this.#dispatch(message as MessageEvent));
    }
  }

  #dispatch(message: MessageEvent): void {
    let event: UguisuEvent;
    try {
      event = JSON.parse(message.data as string) as UguisuEvent;
    } catch {
      return;
    }
    if (typeof event.id === 'string') {
      this.#lastEventId = event.id;
    }
    for (const listener of this.#listeners) {
      listener(event);
    }
  }

  #clearTimer(): void {
    if (this.#timer !== null) {
      clearTimeout(this.#timer);
      this.#timer = null;
    }
  }
}

/**
 * The event names the server sends (`EventKind::name()`). SSE dispatches by
 * name, so a name missing here is a message the UI never sees.
 */
export const KINDS = [
  'podcast.added',
  'podcast.removed',
  'podcast.feed.refresh.started',
  'podcast.feed.refresh.completed',
  'podcast.feed.refresh.failed',
  'podcast.feed.not_modified',
  'podcast.metadata.updated',
  'episode.discovered',
  'episode.updated',
  'episode.removal_detected',
  'episode.identity_ambiguous',
  'episode.duplicate_resolved',
  'feed.url.change_detected',
  'feed.url.changed',
  'download.queued',
  'download.started',
  'download.progress',
  'download.paused',
  'download.resumed',
  'download.retry_scheduled',
  'download.completed',
  'download.failed',
  'download.cancelled',
  'download.paused_all',
  'download.resumed_all',
  'archive.registered',
  'archive.verified',
  'archive.missing',
  'archive.invalid',
  'archive.relocated',
  'archive.source_changed',
  'archive.policy_queued',
  'archive.policy_skipped',
  'archive.sidecar.written',
  'archive.sidecar.invalid',
  'archive.manifest.written',
  'archive.manifest.mismatch',
  'archive.rebuild.completed',
  'archive.imported',
  'archive.import.skipped',
  'archive.import.completed',
  'archive.tagged',
  'archive.tags.skipped',
  'podcast.artwork.fetched',
  'podcast.artwork.unchanged',
  'podcast.artwork.failed',
  'scheduler.tick',
  'scheduler.paused',
  'scheduler.resumed',
  'settings.changed',
  'settings.rejected',
  'search.reindexed',
] as const;

/** The application's single event stream. */
export const events = new EventStream();

/** Builds an isolated stream for a test. */
export function createEventStream(factory: StreamFactory): EventStream {
  return new EventStream(factory);
}

export type { EventStream };
