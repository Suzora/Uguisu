// Text shared by the whole interface: the parts every view uses.
export const common = {
  format: {
    secondsLeft: (n: number) => `${n} s left`,
    minutesLeft: (n: number) => `${n} min left`,
    transferred: (done: string, total: string, share: number) => `${done} of ${total} (${share}%)`,
  },
  api: {
    unreachable: (detail: string) =>
      detail ? `cannot reach the Uguisu API: ${detail}` : 'cannot reach the Uguisu API',
    notJson: 'the server sent a body that is not JSON',
  },
  state: {
    loading: 'Loading…',
    empty: 'Nothing here yet',
    failed: 'Request failed',
    network: 'The Uguisu API did not answer. Is `uguisu serve` still running?',
    timeout: 'The request took too long and was given up on.',
    httpStatus: (status: number) => ` · HTTP ${status}`,
    retry: 'Try again',
  },
  pager: {
    shown: (n: number) => `${n} shown`,
    more: 'Load more',
  },
  progress: {
    sizeUnknown: 'size unknown',
  },
  player: {
    region: 'Now playing',
    failed:
      "This episode's audio could not be played. The archived file may be missing or unreadable — the archive view says which.",
    close: 'Close the player',
  },
};
