// The desktop shell's native notifications.
export const notifications = {
  someEpisode: 'An episode',
  archived: (title: string) => `${title} is archived.`,
  failedTitle: (podcast: string) => `${podcast}: download failed`,
  failed: (title: string, outcome: string) => `${title} — ${outcome}`,
  gaveUp: (attempts: number) => `Uguisu gave up after ${attempts} attempts.`,
  notRetried: 'It was not retried.',
  refreshFailedTitle: 'Feed refresh failed',
  refreshFailed: 'A podcast feed could not be refreshed.',
};
