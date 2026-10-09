// One request's lifecycle, shared by every view.
//
// A view holds a `Resource`, calls `load()` with a fetcher, and renders
// `loading` / `error` / `data`. The resource aborts a request that a newer
// one supersedes, which is what keeps a typed search box from racing itself,
// and it keeps the last good value while reloading so a refresh does not
// blank the page.

import { ApiFailure } from './api';

export class Resource<T> {
  data = $state<T | null>(null);
  error = $state<unknown>(null);
  loading = $state(false);
  /** Whether a request has ever finished, so "empty" is distinguishable. */
  loaded = $state(false);

  #controller: AbortController | null = null;

  /** Runs `fetcher`, superseding any request still in flight. */
  async load(fetcher: (signal: AbortSignal) => Promise<T>): Promise<T | null> {
    this.#controller?.abort();
    const controller = new AbortController();
    this.#controller = controller;
    this.loading = true;
    try {
      const value = await fetcher(controller.signal);
      if (controller.signal.aborted) {
        return null;
      }
      this.data = value;
      this.error = null;
      this.loaded = true;
      return value;
    } catch (error) {
      if (controller.signal.aborted || (error instanceof ApiFailure && error.failure === 'cancelled')) {
        return null;
      }
      this.error = error;
      this.loaded = true;
      return null;
    } finally {
      if (this.#controller === controller) {
        this.loading = false;
        this.#controller = null;
      }
    }
  }

  /** Cancels an in-flight request, e.g. when the view unmounts. */
  cancel(): void {
    this.#controller?.abort();
    this.#controller = null;
  }

  /** What `StateBlock` should show, or `null` when there is data to render. */
  get state(): 'loading' | 'error' | 'ready' {
    if (this.error !== null) {
      return 'error';
    }
    return this.data === null && this.loading ? 'loading' : 'ready';
  }
}
