// The archive import page (ADR 0059).
import { plural } from './locale';

export const archiveImport = {
  title: 'Import an archive',
  intro:
    'Reads a folder another tool wrote, matches each file to an episode and copies the files it can place. Nothing in the folder is moved or changed, and nothing is copied until you ask.',
  link: 'Import an archive another tool wrote',
  path: 'Folder on the server',
  pathHint:
    'A path on the machine Uguisu runs on, not on this computer. The Flatpak app cannot read folders outside its sandbox: use uguisu archive import there.',
  format: 'Layout',
  formats: { detect: 'Detect', podgrab: 'Podgrab', generic: 'Generic' },
  podgrabDb: 'Podgrab database (optional)',
  podgrabDbHint: 'The path of podgrab.db on the server. Stop Podgrab first; the file is only read.',
  podcast: 'Only this podcast (optional)',
  everyPodcast: 'Every podcast',
  read: 'Read the folder',
  reading: 'Reading every file and its tags…',
  planned: (scanned: number, imported: number) =>
    `${plural(scanned, { one: `${scanned} file read`, other: `${scanned} files read` })}; ${plural(imported, {
      one: `${imported} can be copied`,
      other: `${imported} can be copied`,
    })}.`,
  applied: (imported: number) =>
    plural(imported, { one: `${imported} file copied.`, other: `${imported} files copied.` }),
  copy: (n: number) => plural(n, { one: `Copy ${n} file`, other: `Copy ${n} files` }),
  copying: (done: number, total: number) => `Copied ${done} of ${total}…`,
  lost: (reason: string) => `${reason}. What was copied stays; running it again continues with the rest.`,
  unreadable: (n: number) =>
    plural(n, {
      one: `${n} entry could not be read (a link, a name that is not usable, or no permission) and was skipped.`,
      other: `${n} entries could not be read (links, names that are not usable, or no permission) and were skipped.`,
    }),
  everyOutcome: 'Every outcome',
  caption: 'What the import would do with each file',
  columns: {
    file: 'File',
    outcome: 'Outcome',
    episode: 'Episode',
    confidence: 'Confidence',
    target: 'Target',
    note: 'Note',
  },
  confidence: (percent: number) => `${percent}%`,
  more: (left: number) => plural(left, { one: `Show ${left} more`, other: `Show ${left} more` }),
  afterwards: {
    verify: 'Check the copies on the Archive page',
    downloads:
      'Turn on automatic downloads only now: an episode that was imported is not fetched again.',
  },
};
