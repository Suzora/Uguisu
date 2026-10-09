// The URL is the UI's state. This module is the only place that knows the
// mapping between a path and a view, in both directions.
//
// It is deliberately not a router framework: there are twelve routes, two of
// them take a parameter, and the query string carries the rest. Everything
// here is a pure function so it can be tested without a DOM; the reactive
// wrapper lives in `navigation.svelte.ts`.

export type Route =
  | { name: 'dashboard' }
  | { name: 'library' }
  | { name: 'podcast'; id: string }
  | { name: 'episode'; id: string }
  | { name: 'discover' }
  | { name: 'search' }
  | { name: 'downloads' }
  | { name: 'archive' }
  | { name: 'archiveImport' }
  | { name: 'archiveRepair' }
  | { name: 'settings' }
  | { name: 'service' }
  | { name: 'unknown'; path: string };

export type RouteName = Route['name'];

/** A parsed location: which view, and the query state that view reads. */
export interface Location {
  route: Route;
  query: Record<string, string>;
  /** The path and query exactly as the address bar shows them. */
  href: string;
}

/** Parses `pathname` into a route, without touching the query string. */
export function parsePath(pathname: string): Route {
  const segments = pathname.split('/').filter((s) => s.length > 0);
  if (segments.length === 0) {
    return { name: 'dashboard' };
  }
  const [head, second] = segments;
  switch (head) {
    case 'podcasts':
      return second === undefined
        ? { name: 'library' }
        : { name: 'podcast', id: decodeURIComponent(second) };
    case 'episodes':
      return second === undefined
        ? { name: 'unknown', path: pathname }
        : { name: 'episode', id: decodeURIComponent(second) };
    case 'discover':
      return { name: 'discover' };
    case 'search':
      return { name: 'search' };
    case 'downloads':
      return { name: 'downloads' };
    case 'archive':
      if (second === 'import') {
        return { name: 'archiveImport' };
      }
      if (second === 'repair') {
        return { name: 'archiveRepair' };
      }
      return second === undefined ? { name: 'archive' } : { name: 'unknown', path: pathname };
    case 'settings':
      return { name: 'settings' };
    case 'service':
      return { name: 'service' };
    default:
      return { name: 'unknown', path: pathname };
  }
}

/** The path a route lives at; the inverse of {@link parsePath}. */
export function pathFor(route: Route): string {
  switch (route.name) {
    case 'dashboard':
      return '/';
    case 'library':
      return '/podcasts';
    case 'podcast':
      return `/podcasts/${encodeURIComponent(route.id)}`;
    case 'episode':
      return `/episodes/${encodeURIComponent(route.id)}`;
    case 'archiveImport':
      return '/archive/import';
    case 'archiveRepair':
      return '/archive/repair';
    case 'unknown':
      return route.path;
    default:
      return `/${route.name}`;
  }
}

/** A link target: a route plus the query state the view should open with. */
export function hrefFor(route: Route, query: Record<string, string | undefined> = {}): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== '') {
      search.set(key, value);
    }
  }
  const suffix = search.toString();
  return `${pathFor(route)}${suffix ? `?${suffix}` : ''}`;
}

/** Parses a full `path?query` string into the location a view can read. */
export function parseLocation(href: string): Location {
  const [pathname = '/', search = ''] = href.split('?');
  const query: Record<string, string> = {};
  for (const [key, value] of new URLSearchParams(search)) {
    query[key] = value;
  }
  return { route: parsePath(pathname), query, href };
}

/** The same location with `patch` merged in; an empty value drops a key. */
export function withQuery(
  location: Location,
  patch: Record<string, string | undefined>,
): string {
  const merged: Record<string, string | undefined> = { ...location.query };
  for (const [key, value] of Object.entries(patch)) {
    merged[key] = value;
  }
  return hrefFor(location.route, merged);
}

/** Where each route sits in the primary navigation, for the current-page mark. */
export function sectionOf(route: Route): RouteName {
  if (route.name === 'podcast' || route.name === 'episode') {
    return 'library';
  }
  return route.name === 'archiveImport' || route.name === 'archiveRepair' ? 'archive' : route.name;
}
