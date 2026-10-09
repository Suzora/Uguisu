import { beforeEach, describe, expect, it, vi } from 'vitest';
import { backoffMs, createEventStream } from './events.svelte';
import { FakeEventSource } from '../tests/harness';

describe('event stream', () => {
  beforeEach(() => {
    FakeEventSource.reset();
  });

  it('backs off and then stops growing', () => {
    expect(backoffMs(0)).toBe(500);
    expect(backoffMs(1)).toBe(1000);
    expect(backoffMs(6)).toBe(30_000);
    expect(backoffMs(20)).toBe(30_000);
  });

  it('opens exactly one connection', () => {
    const stream = createEventStream((url) => new FakeEventSource(url) as unknown as EventSource);
    stream.start();
    stream.start();
    stream.start();
    expect(FakeEventSource.instances).toHaveLength(1);
    stream.stop();
  });

  it('delivers an event to subscribers', () => {
    const stream = createEventStream((url) => new FakeEventSource(url) as unknown as EventSource);
    const seen: string[] = [];
    stream.subscribe((event) => seen.push(event.kind));
    stream.start();
    FakeEventSource.latest.open();
    FakeEventSource.latest.emit('download.started', {
      schema: 1,
      id: '01EV1',
      occurred_at: '2026-01-01T12:00:00Z',
      podcast_id: null,
      episode_id: null,
      kind: 'download.started',
    });
    expect(seen).toEqual(['download.started']);
    expect(stream.lastEventId).toBe('01EV1');
    stream.stop();
  });

  it('asks for a resync on every open', () => {
    vi.useFakeTimers();
    const stream = createEventStream((url) => new FakeEventSource(url) as unknown as EventSource);
    let resyncs = 0;
    stream.onResync(() => {
      resyncs += 1;
    });
    stream.start();
    FakeEventSource.latest.open();
    expect(resyncs).toBe(1);

    FakeEventSource.latest.fail();
    expect(stream.state).toBe('retrying');
    vi.advanceTimersByTime(500);
    expect(FakeEventSource.instances).toHaveLength(2);
    FakeEventSource.latest.open();
    expect(resyncs).toBe(2);
    stream.stop();
    vi.useRealTimers();
  });

  it('keeps one subscriber across a reconnect', () => {
    vi.useFakeTimers();
    const stream = createEventStream((url) => new FakeEventSource(url) as unknown as EventSource);
    const seen: string[] = [];
    stream.subscribe((event) => seen.push(event.id));
    stream.start();
    FakeEventSource.latest.open();
    FakeEventSource.latest.fail();
    vi.advanceTimersByTime(500);
    FakeEventSource.latest.open();
    FakeEventSource.latest.emit('download.completed', {
      schema: 1,
      id: '01EV2',
      occurred_at: '2026-01-01T12:00:01Z',
      podcast_id: null,
      episode_id: null,
      kind: 'download.completed',
    });
    // One delivery, not two: the old connection's listeners went with it.
    expect(seen).toEqual(['01EV2']);
    stream.stop();
    vi.useRealTimers();
  });

  it('stops retrying once closed', () => {
    vi.useFakeTimers();
    const stream = createEventStream((url) => new FakeEventSource(url) as unknown as EventSource);
    stream.start();
    FakeEventSource.latest.open();
    stream.stop();
    expect(FakeEventSource.latest.closed).toBe(true);
    vi.advanceTimersByTime(60_000);
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(stream.state).toBe('closed');
    vi.useRealTimers();
  });
});
