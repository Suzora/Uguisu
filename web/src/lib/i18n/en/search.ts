// Local search over the library.
import { plural } from './locale';

const episodes = (n: number) => plural(n, { one: `${n} episode`, other: `${n} episodes` });
const podcasts = (n: number) => plural(n, { one: `${n} podcast`, other: `${n} podcasts` });

export const search = {
  title: 'Search the library',
  intro: 'Podcasts and episodes already added. Operators are ordinary words — there is nothing to escape.',
  text: 'Search text',
  placeholder: 'Words from a title or the show notes',
  kind: 'What to search',
  kinds: {
    all: 'Everything',
    podcasts: 'Podcasts',
    episodes: 'Episodes',
  },
  reindexed: (episodeCount: number, podcastCount: number, ms: number) =>
    `Indexed ${episodes(episodeCount)} across ${podcasts(podcastCount)} in ${ms} ms.`,
  failed: 'The search failed',
  empty: {
    title: 'Type something to search',
    hint: (episodeCount: number, podcastCount: number) =>
      `The index holds ${episodes(episodeCount)} across ${podcasts(podcastCount)}.`,
  },
  building: {
    title: 'The search index is still being built',
    hint: (episodeCount: number, detail?: string | null) =>
      `${episodes(episodeCount)} indexed so far${detail ? ` — ${detail}` : ''}. Results will be incomplete until it finishes.`,
  },
  stale: {
    title: 'The search index is out of date',
    hint: (builtOn: string | null) =>
      `It was last built ${builtOn ? `on ${builtOn}` : 'never'}, so this query found nothing. Rebuilding may take a while on a large library.`,
    rebuild: 'Rebuild the index',
    notice: 'The index is out of date, so newer episodes may be missing.',
    rebuildIt: 'Rebuild it',
  },
  rebuilding: 'Rebuilding…',
  noResults: {
    title: (terms: string[]) => `Nothing matched ${terms.map((t) => `“${t}”`).join(' ')}`,
    truncated: 'The query was shortened before searching.',
  },
  summary: (n: number, ms: number) =>
    `${plural(n, { one: `${n} result`, other: `${n} results` })} in ${ms} ms`,
  results: {
    podcasts: 'Podcasts',
    episodes: 'Episodes',
    score: (score: string) => `score ${score}`,
  },
};
