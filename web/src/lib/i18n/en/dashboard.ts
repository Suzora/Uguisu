// The Dashboard page.
import { plural } from './locale';

export const dashboard = {
  title: 'Dashboard',
  unreadable: 'The service status could not be read',
  library: {
    title: 'Library',
    subscribed: (n: number) => plural(n, { one: 'podcast subscribed', other: 'podcasts subscribed' }),
    totalsUnavailable: 'Archive totals unavailable.',
    archivedFiles: (n: number) =>
      plural(n, { one: `${n} archived file`, other: `${n} archived files` }),
    missing: (n: number) => `${n} missing`,
    invalid: (n: number) => `${n} invalid`,
    open: 'Open the library',
  },
  downloads: {
    title: 'Downloads',
    running: 'running now',
    queued: (n: number) => `${n} queued`,
    retrying: (n: number) => `${n} retrying`,
    paused: (n: number) => `${n} paused`,
    failed: (n: number) => `${n} failed`,
    completed: (n: number) => `${n} completed`,
    // Workers are not running: start <uguisu serve>.
    noWorkers: {
      before: 'Workers are not running: start ',
      command: 'uguisu serve',
      after: '.',
    },
    pausedAll: (reason: string) => `paused: ${reason}`,
    open: 'Open the queue',
  },
  scheduler: {
    title: 'Scheduler',
    due: (n: number) => plural(n, { one: 'podcast due now', other: 'podcasts due now' }),
    paused: 'paused',
    running: 'running',
    stopped: 'stopped',
    next: (when: string) => `next: ${when}`,
    interval: (minutes: number) => `every ${minutes} min`,
    open: 'Open the service view',
  },
  search: {
    title: 'Search index',
    indexed: (episodes: number, podcasts: number) =>
      `${plural(episodes, { one: `${episodes} episode`, other: `${episodes} episodes` })} · ${plural(podcasts, {
        one: `${podcasts} podcast`,
        other: `${podcasts} podcasts`,
      })} indexed`,
    open: 'Search the library',
  },
  // <n> stored settings are being ignored. <Review them>.
  ignoredSettings: {
    before: (n: number) =>
      plural(n, {
        one: `${n} stored setting is being ignored. `,
        other: `${n} stored settings are being ignored. `,
      }),
    review: 'Review them',
    after: '.',
  },
  activity: {
    title: 'Recent activity',
    unreadable: 'The event log could not be read',
    none: 'No events yet',
    noneHint: 'Adding a podcast is the first one.',
    episode: 'episode',
  },
};
