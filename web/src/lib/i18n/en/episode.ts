// One episode's page.
export const episode = {
  unreadable: 'This episode could not be read',
  back: (podcast: string) => `← ${podcast}`,
  play: 'Play',
  dates: {
    title: 'Dates',
    published: 'Published',
    firstSeen: 'First seen',
    removed: 'Gone from the feed',
  },
  notes: {
    title: 'Show notes',
    none: 'No show notes in the feed.',
    page: 'Episode page',
  },
  archive: {
    title: 'Archive',
    none: 'Not in the archive.',
    file: 'File',
    size: 'Size',
    state: 'Verification',
    checked: 'Last checked',
    tags: 'Tags',
  },
  job: {
    title: 'Download',
    none: 'Never queued.',
    state: 'State',
    attempts: 'Attempts',
    attemptsOf: (count: number, max: number) => `${count} of ${max}`,
    lastError: 'Last error',
    updated: 'Updated',
  },
};
