// The podcast library.
import { plural } from './locale';

export const library = {
  title: 'Podcasts',
  exportOpml: 'Export OPML',
  add: 'Add a podcast',
  filter: 'Filter by title',
  status: {
    label: 'Status',
    any: 'Every status',
    active: 'Active',
    paused: 'Paused',
    error: 'Error',
    archived: 'Archived',
  },
  sort: {
    label: 'Sort',
    title: 'Title',
    added: 'Recently added',
    refreshed: 'Recently refreshed',
    episodes: 'Episode count',
  },
  unreadable: 'The library could not be read',
  emptyTitle: 'No podcasts yet',
  emptyHint: 'Find one on the Discover page, or add a feed URL there directly.',
  noMatch: 'No podcast matches this filter',
  episodes: (n: number) => plural(n, { one: `${n} episode`, other: `${n} episodes` }),
  inFeed: (n: number) => `${n} in feed`,
  refreshed: (when: string) => `refreshed ${when}`,
  announced: 'new feed announced',
};
