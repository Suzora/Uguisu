// The Downloads page.
import { plural } from './locale';

export const downloads = {
  title: 'Downloads',
  everyState: 'Show every state',
  // No workers are running in this process, so nothing will be downloaded until <uguisu serve> is started.
  noWorkers: {
    before: 'No workers are running in this process, so nothing will be downloaded until ',
    command: 'uguisu serve',
    after: ' is started.',
  },
  queuePaused: (reason: string) => `The whole queue is paused (${reason}).`,
  pauseAll: 'Pause everything',
  pausedAll: 'The queue is paused.',
  resumeAll: 'Resume everything',
  resumedAll: 'The queue is running again.',
  retryFailed: 'Retry every failure',
  retriedFailed: (n: number) =>
    plural(n, { one: `${n} failed job queued again.`, other: `${n} failed jobs queued again.` }),
  onlyPodcast: (podcast: string) => `Only ${podcast}`,
  everyPodcast: '(show all)',
  unreadable: 'The queue could not be read',
  emptyTitle: 'The queue is empty',
  emptyHint: 'Download an episode from a podcast page.',
  noneInState: (state: string) => `No job is ${state}`,
  caption: 'Download jobs, newest first',
  columns: {
    episode: 'Episode',
    state: 'State',
    progress: 'Progress',
    attempts: 'Attempts',
    updated: 'Updated',
    actions: 'Actions',
  },
  nextTry: (when: string) => `next try ${when}`,
  progressOf: (episode: string) => `Progress of ${episode}`,
  pause: 'Pause',
  paused: 'Paused.',
  cancel: 'Cancel',
  cancelled: 'Cancelled.',
  resume: 'Resume',
  resumed: 'Resumed.',
  retry: 'Retry',
  retried: 'Queued again.',
};
