// The page that repairs missing archived files (ADR 0060).
import { plural } from './locale';

export const archiveRepair = {
  title: 'Repair missing files',
  intro:
    'A file is missing when a check finds nothing at its path. Put it back from a folder that holds exactly the same bytes, or download it again. Nothing that is in place is ever replaced.',
  link: 'Repair missing files',
  repairOne: 'Repair this file',
  onlyPodcast: (title: string) => `Only ${title}.`,
  everyPodcast: 'Every podcast',
  check: {
    title: '1. Find what is missing',
    button: 'Check every file again',
    checking: 'Checking…',
    hint: 'A check looks whether each file is at its path. It reads no bytes.',
    report: (checked: number, missing: number) =>
      `${plural(checked, { one: `${checked} file checked`, other: `${checked} files checked` })}; ${plural(missing, {
        one: `${missing} missing`,
        other: `${missing} missing`,
      })}.`,
    count: (n: number, more: boolean) =>
      more
        ? `At least ${n} files are missing.`
        : plural(n, { one: `${n} file is missing.`, other: `${n} files are missing.` }),
    none: 'No file is missing, as far as the last check knows.',
    unreadable: 'The missing files could not be read',
    caption: 'Archived files that are missing',
    columns: { file: 'File', size: 'Size', checked: 'Last checked', actions: 'Actions' },
  },
  folder: {
    title: '2. Put them back from a folder',
    path: 'Folder on the server',
    pathHint:
      'A backup, or a disk the files were moved to, on the machine Uguisu runs on. A file is used only when its bytes are exactly the archived ones, whatever its name, as long as it keeps its extension. The folder is only read. The Flatpak app cannot read folders outside its sandbox: use uguisu archive restore there.',
    look: 'Look in the folder',
    looking: 'Hashing files of the right size…',
    planned: (scanned: number, found: number) =>
      `${plural(scanned, { one: `${scanned} file looked at`, other: `${scanned} files looked at` })}; ${plural(found, {
        one: `${found} can be put back`,
        other: `${found} can be put back`,
      })}.`,
    applied: (n: number) => plural(n, { one: `${n} file put back and verified.`, other: `${n} files put back and verified.` }),
    restore: (n: number) => plural(n, { one: `Put ${n} file back`, other: `Put ${n} files back` }),
    lost: (reason: string) => `${reason}. What was put back stays; running it again continues with the rest.`,
    caption: 'What the folder holds for each missing file',
    columns: { file: 'Missing file', outcome: 'Outcome', source: 'In the folder', note: 'Note' },
  },
  again: {
    title: '3. Download the rest again',
    hint: 'A new download may not be the same bytes as the lost file: hosts re-encode and insert ads. Put a file back from a folder when you have one.',
    one: 'Download again',
    all: (n: number) => plural(n, { one: `Download the ${n} listed file again`, other: `Download the ${n} listed files again` }),
    queuedAll: (n: number) => plural(n, { one: `${n} download queued.`, other: `${n} downloads queued.` }),
    refused: (n: number, why: string[]) =>
      `${plural(n, { one: `${n} download queued`, other: `${n} downloads queued` })}; ${plural(why.length, {
        one: `${why.length} refused`,
        other: `${why.length} refused`,
      })}: ${why.join('; ')}.`,
    downloads: 'Follow them on the Downloads page',
  },
};
