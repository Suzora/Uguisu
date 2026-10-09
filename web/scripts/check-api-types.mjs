// Fails when src/lib/api/generated/schema.d.ts is not what the OpenAPI
// document produces. Renders in process rather than shelling out to `diff`,
// so it behaves the same on every CI leg.
//
//   node scripts/check-api-types.mjs          # check
//   node scripts/check-api-types.mjs --write  # regenerate (what `pnpm api:types` runs)

import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import openapiTS, { astToString } from 'openapi-typescript';

const here = fileURLToPath(new URL('.', import.meta.url));
const document = new URL('../../docs/api/openapi.json', import.meta.url);
const target = `${here}../src/lib/api/generated/schema.d.ts`;

const rendered = astToString(await openapiTS(document));

if (process.argv.includes('--write')) {
  await writeFile(target, rendered);
  console.log(target);
  process.exit(0);
}

const committed = await readFile(target, 'utf8').catch(() => '');
if (committed === rendered) {
  process.exit(0);
}
console.error(
  'src/lib/api/generated/schema.d.ts is not what docs/api/openapi.json produces;\n' +
    'run `pnpm api:types` (and never edit the generated file by hand)',
);
process.exit(1);
