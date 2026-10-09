// The Archive page.
import { plural } from './locale';

export const archive = {
  title: 'Archive',
  total: (n: number) => plural(n, { one: `${n} file in total`, other: `${n} files in total` }),
  totalsUnreadable: (reason: string) => `Archive totals could not be read: ${reason}`,
  checkButton: 'Check that the files are there',
  checking: 'Checking…',
  checkedNotice: 'Checked.',
  checkedReport: (checked: number, verified: number, missing: number, invalid: number) =>
    `Checked ${checked}: ${verified} intact, ${missing} missing, ${invalid} invalid.`,
  verifyAllButton: 'Verify every hash',
  hashing: 'Hashing…',
  verifiedNotice: 'Verified.',
  hashedReport: (checked: number, verified: number, missing: number, invalid: number) =>
    `Hashed ${checked}: ${verified} intact, ${missing} missing, ${invalid} invalid.`,
  hashingHint: 'Hashing reads every archived byte and can take a long time.',
  staleManifests: {
    // 2 manifests are out of date. They are rewritten by <uguisu archive manifest write>.
    count: (n: number) =>
      plural(n, { one: `${n} manifest is out of date.`, other: `${n} manifests are out of date.` }),
    before: (n: number) => plural(n, { one: 'It is rewritten by ', other: 'They are rewritten by ' }),
    command: 'uguisu archive manifest write',
    end: '.',
  },
  onlyPodcast: (title: string) => `Only ${title}`,
  everyPodcast: '(show every podcast)',
  unreadable: 'The archive could not be read',
  none: 'Nothing is archived yet',
  noneHint: 'Download an episode and it appears here.',
  noneInState: (state: string) => `No file is ${state}`,
  caption: 'Archived files, newest first',
  columns: {
    file: 'File',
    state: 'State',
    size: 'Size',
    sidecar: 'Sidecar',
    tags: 'Tags',
    checked: 'Checked',
    actions: 'Actions',
  },
  noSidecar: 'none',
  verifyButton: 'Verify',
  revealButton: 'Reveal',
  revealedNotice: 'Shown in the file manager.',
};
