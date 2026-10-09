// The English catalogue: the interface as it ships (ADR 0053).
import { app } from './app';
import { archive } from './archive';
import { archiveImport } from './archiveImport';
import { archiveRepair } from './archiveRepair';
import { common } from './common';
import { dashboard } from './dashboard';
import { discover } from './discover';
import { downloads } from './downloads';
import { episode } from './episode';
import { episodes } from './episodes';
import { library } from './library';
import { locale } from './locale';
import { login } from './login';
import { notifications } from './notifications';
import { podcast } from './podcast';
import { search } from './search';
import { service } from './service';
import { settings } from './settings';

/**
 * Words for values of the API's closed vocabularies (`not_modified`), keyed
 * by value. English shows the values themselves; a translation fills this
 * from the enums of `docs/api/openapi.json`.
 */
const values: Record<string, string> = {};

export const en = {
  locale,
  values,
  common,
  app,
  episodes,
  notifications,
  login,
  dashboard,
  library,
  podcast,
  episode,
  discover,
  search,
  downloads,
  archive,
  archiveImport,
  archiveRepair,
  settings,
  service,
};
