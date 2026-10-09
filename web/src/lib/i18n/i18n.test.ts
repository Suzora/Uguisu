import { describe, expect, it } from 'vitest';
import { label, m } from '.';
import { plurals } from './plural';

describe('catalogue', () => {
  it('unpacks a snake_case value', () => {
    expect(label('hash_mismatch')).toBe('hash mismatch');
    expect(label(null)).toBe('—');
  });

  it('prefers the catalogue word', () => {
    m.values.hash_mismatch = 'checksum differs';
    try {
      expect(label('hash_mismatch')).toBe('checksum differs');
    } finally {
      delete m.values.hash_mismatch;
    }
  });

  it('reads a count of one as singular', () => {
    expect(m.library.episodes(1)).toBe('1 episode');
    expect(m.archive.total(1)).toBe('1 file in total');
    expect(m.login.retryAfter(1)).toBe('Try again in 1 second.');
    expect(m.discover.opml.planned(1, 1)).toBe('1 of 1 feed is new.');
    expect(m.service.scheduler.housekeepingReport(1, 2)).toBe('1 event pruned, 2 cache rows expired.');
    expect(m.dashboard.search.indexed(1, 3)).toBe('1 episode · 3 podcasts indexed');
  });

  it('picks plural forms by locale', () => {
    const forms = { one: 'one', few: 'few', many: 'many', other: 'other' };
    expect([1, 2, 5].map((n) => plurals('en')(n, forms))).toEqual(['one', 'other', 'other']);
    // Polish has three forms where English has two.
    expect([1, 2, 5].map((n) => plurals('pl')(n, forms))).toEqual(['one', 'few', 'many']);
  });
});
