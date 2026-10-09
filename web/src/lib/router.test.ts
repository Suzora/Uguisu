import { describe, expect, it } from 'vitest';
import { hrefFor, parseLocation, parsePath, pathFor, sectionOf, withQuery } from './router';

describe('router', () => {
  it('maps every path to a route', () => {
    expect(parsePath('/')).toEqual({ name: 'dashboard' });
    expect(parsePath('/podcasts')).toEqual({ name: 'library' });
    expect(parsePath('/podcasts/01ABC')).toEqual({ name: 'podcast', id: '01ABC' });
    expect(parsePath('/episodes/01EP')).toEqual({ name: 'episode', id: '01EP' });
    expect(parsePath('/episodes')).toEqual({ name: 'unknown', path: '/episodes' });
    expect(parsePath('/downloads')).toEqual({ name: 'downloads' });
    expect(parsePath('/archive')).toEqual({ name: 'archive' });
    expect(parsePath('/archive/import')).toEqual({ name: 'archiveImport' });
    expect(parsePath('/archive/repair')).toEqual({ name: 'archiveRepair' });
    expect(parsePath('/archive/elsewhere')).toEqual({ name: 'unknown', path: '/archive/elsewhere' });
    expect(parsePath('/discover')).toEqual({ name: 'discover' });
    expect(parsePath('/search')).toEqual({ name: 'search' });
    expect(parsePath('/settings')).toEqual({ name: 'settings' });
    expect(parsePath('/service')).toEqual({ name: 'service' });
    expect(parsePath('/nope')).toEqual({ name: 'unknown', path: '/nope' });
  });

  it('round-trips a path', () => {
    for (const path of ['/', '/podcasts', '/podcasts/01ABC', '/episodes/01EP', '/downloads', '/archive/import', '/archive/repair', '/settings']) {
      expect(pathFor(parsePath(path))).toBe(path);
    }
  });

  it('decodes a podcast id', () => {
    expect(parsePath('/podcasts/01%41BC')).toEqual({ name: 'podcast', id: '01ABC' });
  });

  it('builds an episode link', () => {
    expect(hrefFor({ name: 'episode', id: '01EP/x' })).toBe('/episodes/01EP%2Fx');
    expect(parsePath('/episodes/01EP%2Fx')).toEqual({ name: 'episode', id: '01EP/x' });
  });

  it('keeps query state out of the route', () => {
    const location = parseLocation('/search?q=rust+async&kind=episodes');
    expect(location.route).toEqual({ name: 'search' });
    expect(location.query).toEqual({ q: 'rust async', kind: 'episodes' });
  });

  it('drops empty query values', () => {
    expect(hrefFor({ name: 'downloads' }, { state: 'failed', podcast: '' })).toBe(
      '/downloads?state=failed',
    );
    expect(hrefFor({ name: 'downloads' }, { state: undefined })).toBe('/downloads');
  });

  it('merges a query patch', () => {
    const location = parseLocation('/podcasts?q=dark&sort=added');
    expect(withQuery(location, { q: 'light' })).toBe('/podcasts?q=light&sort=added');
    expect(withQuery(location, { sort: undefined })).toBe('/podcasts?q=dark');
  });

  it('marks a podcast page as the library section', () => {
    expect(sectionOf({ name: 'podcast', id: '01ABC' })).toBe('library');
    expect(sectionOf({ name: 'episode', id: '01EP' })).toBe('library');
    expect(sectionOf({ name: 'archive' })).toBe('archive');
    expect(sectionOf({ name: 'archiveImport' })).toBe('archive');
    expect(sectionOf({ name: 'archiveRepair' })).toBe('archive');
  });
});
