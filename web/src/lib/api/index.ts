// The API surface the UI uses, one function per operation.
//
// Each function names the route it calls and returns the parsed body. Views
// import from here and never build a URL themselves, so the day a generated
// client arrives this file is the only thing that has to change.

import { API_BASE, expectArray, request, setCsrfToken, url } from './client';
import type { RequestOptions } from './client';
import type {
  AddOutcome,
  ApiTokenRecord,
  ArchiveFile,
  ArchiveManifest,
  ArchivePage,
  ArchiveStats,
  UguisuEvent,
  DiscoverySearchResponse,
  DownloadJob,
  DownloadState,
  DownloadStats,
  DuplicateResolution,
  DuplicateResolved,
  EnqueueOutcome,
  EnqueueSummary,
  EpisodeDetail,
  EpisodePage,
  FeedMove,
  ImportBody,
  ImportRequest,
  JobDetail,
  JobPage,
  LibrarySearchResults,
  LoginResult,
  SessionExchange,
  NewApiToken,
  OpmlImport,
  PodcastArtwork,
  PodcastDetail,
  PodcastPage,
  PodcastRemoval,
  PolicyBody,
  PolicyMode,
  Priority,
  ProviderStatus,
  RefreshReport,
  ResolvedFeed,
  RestoreBody,
  RestoreRequest,
  SchedulerStatus,
  ServiceStatus,
  Session,
  SettingsReport,
  TokenScope,
  VerifiedFile,
  VerifySummary,
} from './types';

export { API_BASE, ApiFailure, messageFor, onUnauthorized, setCsrfToken, url } from './client';
export type { FailureKind, RequestOptions } from './client';
export * from './types';

type Opts = Pick<RequestOptions, 'signal' | 'timeoutMs'>;

export async function health(o: Opts = {}): Promise<{ status: string; version: string }> {
  return request('/health', o);
}

export async function status(o: Opts = {}): Promise<ServiceStatus> {
  return request('/status', o);
}

// Authentication

/**
 * What this request is and what the server needs. Public, so the shell can ask
 * before it has a credential, and the one place the CSRF token is picked up.
 */
export async function session(o: Opts = {}): Promise<Session> {
  const body = await request<Session>('/auth/session', o);
  setCsrfToken(body.csrf_token ?? null);
  return body;
}

export async function login(
  username: string,
  password: string,
  o: Opts = {},
): Promise<LoginResult> {
  const body = await request<LoginResult>('/auth/login', {
    ...o,
    method: 'POST',
    body: { username, password },
  });
  setCsrfToken(body.csrf_token);
  return body;
}

/**
 * Opens a session from the desktop shell's per-launch credential.
 *
 * The only function that sends a bearer token, and the only caller is the
 * desktop bootstrap. The answer's cookie is what authenticates everything
 * after it, so the credential is not kept anywhere.
 */
export async function exchange(secret: string, o: Opts = {}): Promise<SessionExchange> {
  const body = await request<SessionExchange>('/auth/exchange', {
    ...o,
    method: 'POST',
    bearer: secret,
  });
  setCsrfToken(body.csrf_token);
  return body;
}

export async function logout(o: Opts = {}): Promise<void> {
  await request<void>('/auth/logout', { ...o, method: 'POST' });
  setCsrfToken(null);
}

/**
 * Sets the password. `current_password` is required once a credential exists,
 * so the first call on a fresh install may leave it out and no later one can.
 */
export async function setPassword(
  password: { username?: string; current?: string; next: string },
  o: Opts = {},
): Promise<void> {
  return request('/auth/password', {
    ...o,
    method: 'POST',
    body: {
      username: password.username,
      current_password: password.current,
      new_password: password.next,
    },
  });
}

export async function listTokens(o: Opts = {}): Promise<ApiTokenRecord[]> {
  return expectArray<ApiTokenRecord>(await request('/auth/tokens', o), 'tokens');
}

/** The answer carries the secret; it is not retrievable afterwards. */
export async function createToken(
  token: { name: string; scope: TokenScope; expires_at?: string },
  o: Opts = {},
): Promise<NewApiToken> {
  return request('/auth/tokens', { ...o, method: 'POST', body: token });
}

export async function revokeToken(id: string, o: Opts = {}): Promise<void> {
  return request(`/auth/tokens/${encodeURIComponent(id)}`, { ...o, method: 'DELETE' });
}

// Library

/** One page of the library, filtered and sorted by the server (ADR 0057). */
export async function podcastPage(
  filter: { q?: string; status?: string; sort?: string; after?: string; limit?: number } = {},
  o: Opts = {},
): Promise<PodcastPage> {
  const page = await request<PodcastPage>('/podcasts', { ...o, query: filter });
  return { ...page, podcasts: expectArray<PodcastDetail>(page, 'podcasts') };
}

/** Every podcast, following the cursor to the end; for views that look titles up by id. */
export async function listPodcasts(o: Opts = {}): Promise<PodcastDetail[]> {
  const podcasts: PodcastDetail[] = [];
  let after: string | undefined;
  do {
    const page = await podcastPage({ limit: 500, after }, o);
    podcasts.push(...page.podcasts);
    after = page.next_after ?? undefined;
  } while (after);
  return podcasts;
}

export async function getPodcast(id: string, o: Opts = {}): Promise<PodcastDetail> {
  return request(`/podcasts/${encodeURIComponent(id)}`, o);
}

export async function listEpisodes(
  id: string,
  page: { after?: string; limit?: number } = {},
  o: Opts = {},
): Promise<EpisodePage> {
  return request(`/podcasts/${encodeURIComponent(id)}/episodes`, { ...o, query: page });
}

export async function getEpisode(id: string, o: Opts = {}): Promise<EpisodeDetail> {
  return request(`/episodes/${encodeURIComponent(id)}`, o);
}

export async function addPodcast(input: string, o: Opts = {}): Promise<AddOutcome> {
  return request('/podcasts', { ...o, method: 'POST', body: { input }, timeoutMs: 60_000 });
}

/**
 * Plans an OPML import, or with `apply` adds its new feeds (ADR 0049).
 * Applying resolves every new feed, which takes minutes for a few hundred,
 * so it is not timed out here.
 */
export async function importOpml(
  opml: string,
  apply: boolean,
  mode: PolicyMode | null,
  o: Opts = {},
): Promise<OpmlImport> {
  const body = mode === null ? { opml, apply } : { opml, apply, policy: { mode } };
  return request('/podcasts/opml', { ...o, method: 'POST', body, ...(apply ? { timeoutMs: 0 } : {}) });
}

/** The URL of the whole library as an OPML file. */
export function opmlExportUrl(): string {
  return url('/podcasts/opml');
}

/** Moves a podcast to the feed at `url`; one that fails the same-show check only with `force` (ADR 0052). */
export async function moveFeed(id: string, url: string, force = false, o: Opts = {}): Promise<FeedMove> {
  return request(`/podcasts/${encodeURIComponent(id)}/move-feed`, {
    ...o,
    method: 'POST',
    body: { url, force },
  });
}

export async function refreshPodcast(
  id: string,
  force = false,
  o: Opts = {},
): Promise<RefreshReport> {
  return request(`/podcasts/${encodeURIComponent(id)}/refresh`, {
    ...o,
    method: 'POST',
    query: { force },
    timeoutMs: 60_000,
  });
}

export async function pausePodcast(id: string, o: Opts = {}): Promise<{ status: string }> {
  return request(`/podcasts/${encodeURIComponent(id)}/pause`, { ...o, method: 'POST' });
}

export async function resumePodcast(id: string, o: Opts = {}): Promise<{ status: string }> {
  return request(`/podcasts/${encodeURIComponent(id)}/resume`, { ...o, method: 'POST' });
}

/** Stops fetching a podcast and keeps everything; `resumePodcast` undoes it (ADR 0055). */
export async function archivePodcast(id: string, o: Opts = {}): Promise<{ status: string }> {
  return request(`/podcasts/${encodeURIComponent(id)}/archive`, { ...o, method: 'POST' });
}

/** Removes a podcast's records; every file stays on disk (ADR 0055). */
export async function removePodcast(id: string, o: Opts = {}): Promise<PodcastRemoval> {
  return request(`/podcasts/${encodeURIComponent(id)}`, { ...o, method: 'DELETE' });
}

/** `at: null` means "as soon as the scheduler looks". */
export async function schedulePodcast(
  id: string,
  at: string | null,
  o: Opts = {},
): Promise<SchedulerStatus> {
  return request(`/podcasts/${encodeURIComponent(id)}/schedule`, {
    ...o,
    method: 'POST',
    body: { at },
  });
}

export async function podcastArtwork(
  id: string,
  o: Opts = {},
): Promise<{ current: PodcastArtwork | null; history: PodcastArtwork[] }> {
  return request(`/podcasts/${encodeURIComponent(id)}/artwork`, o);
}

/** The URL of the artwork bytes Uguisu stores, or `null` when it has none. */
export function artworkImageUrl(podcastId: string, hash: string | null | undefined): string | null {
  return hash ? url(`/podcasts/${encodeURIComponent(podcastId)}/artwork/image`, { v: hash }) : null;
}

/** The URL of an archived episode's audio. */
export function mediaUrl(episodeId: string): string {
  return url(`/archive/${encodeURIComponent(episodeId)}/media`);
}

// Downloads

export async function listDownloads(
  filter: { state?: DownloadState | ''; podcast?: string; after?: string; limit?: number } = {},
  o: Opts = {},
): Promise<JobPage> {
  return request('/downloads', { ...o, query: filter });
}

export async function downloadStats(o: Opts = {}): Promise<DownloadStats> {
  return request('/downloads/stats', o);
}

export async function getDownload(id: string, o: Opts = {}): Promise<JobDetail> {
  return request(`/downloads/${encodeURIComponent(id)}`, o);
}

/** `POST /episodes/{id}/resolve`: settles a candidate duplicate (ADR 0051). */
export async function resolveDuplicate(
  id: string,
  resolution: DuplicateResolution,
  o: Opts = {},
): Promise<DuplicateResolved> {
  return request(`/episodes/${encodeURIComponent(id)}/resolve`, {
    ...o,
    method: 'POST',
    body: { resolution },
  });
}

export async function enqueueEpisode(
  episodeId: string,
  priority: Priority = 'normal',
  o: Opts = {},
): Promise<EnqueueOutcome> {
  return request('/downloads', {
    ...o,
    method: 'POST',
    body: { episode_id: episodeId, priority },
  });
}

export async function enqueuePodcast(
  id: string,
  priority: Priority = 'normal',
  o: Opts = {},
): Promise<EnqueueSummary> {
  return request(`/podcasts/${encodeURIComponent(id)}/downloads`, {
    ...o,
    method: 'POST',
    body: { priority },
  });
}

type JobCommand = 'cancel' | 'pause' | 'resume' | 'retry';

export async function commandDownload(
  id: string,
  command: JobCommand,
  o: Opts = {},
): Promise<DownloadJob> {
  const body = await request<{ job: DownloadJob }>(
    `/downloads/${encodeURIComponent(id)}/${command}`,
    { ...o, method: 'POST' },
  );
  return body.job;
}

export async function pauseAllDownloads(o: Opts = {}): Promise<void> {
  await request('/downloads/pause', { ...o, method: 'POST' });
}

export async function resumeAllDownloads(o: Opts = {}): Promise<void> {
  await request('/downloads/resume', { ...o, method: 'POST' });
}

export async function retryFailedDownloads(o: Opts = {}): Promise<{ requeued: number }> {
  return request('/downloads/retry-failed', { ...o, method: 'POST' });
}

// Archive

export async function listArchive(
  filter: { state?: string; podcast?: string; after?: string; limit?: number } = {},
  o: Opts = {},
): Promise<ArchivePage> {
  const page = await request<ArchivePage>('/archive', { ...o, query: filter });
  return { ...page, files: expectArray<ArchiveFile>(page, 'files') };
}

export async function archiveStats(o: Opts = {}): Promise<ArchiveStats> {
  return request('/archive/stats', o);
}

export async function archiveFile(episodeId: string, o: Opts = {}): Promise<ArchiveFile> {
  return request(`/archive/${encodeURIComponent(episodeId)}`, o);
}

export async function verifyArchive(
  filter: { podcast?: string; state?: string; depth?: string } = {},
  o: Opts = {},
): Promise<VerifySummary> {
  // An empty value means every podcast or state, which the server spells by
  // leaving the field out; it refuses `""` as an id.
  const body = Object.fromEntries(Object.entries(filter).filter(([, value]) => value !== undefined && value !== ''));
  return request('/archive/verify', { ...o, method: 'POST', body, timeoutMs: 0 });
}

export async function verifyOne(
  episodeId: string,
  depth = 'full',
  o: Opts = {},
): Promise<VerifiedFile> {
  return request(`/archive/${encodeURIComponent(episodeId)}/verify`, {
    ...o,
    method: 'POST',
    body: { depth },
    timeoutMs: 0,
  });
}

/**
 * Plans an archive import, or applies it with `apply: true`. One request
 * that answers when every file is read or copied, so it has no deadline.
 */
export async function importArchive(body: ImportRequest, o: Opts = {}): Promise<ImportBody> {
  return request('/archive/import', { ...o, method: 'POST', body, timeoutMs: 0 });
}

/**
 * Plans putting missing files back from a server folder, or does it with
 * `apply: true` (ADR 0060). It hashes candidates, so it has no deadline.
 */
export async function restoreArchive(body: RestoreRequest, o: Opts = {}): Promise<RestoreBody> {
  return request('/archive/restore', { ...o, method: 'POST', body, timeoutMs: 0 });
}

/** Queues a missing file's episode for download again (ADR 0060). */
export async function redownload(episodeId: string, o: Opts = {}): Promise<EnqueueOutcome> {
  return request(`/archive/${encodeURIComponent(episodeId)}/redownload`, { ...o, method: 'POST' });
}

export async function listManifests(o: Opts = {}): Promise<ArchiveManifest[]> {
  return expectArray<ArchiveManifest>(await request('/archive/manifests', o), 'manifests');
}

export async function getPolicy(id: string, o: Opts = {}): Promise<PolicyBody> {
  return request(`/podcasts/${encodeURIComponent(id)}/policy`, o);
}

export async function setPolicy(
  id: string,
  update: { mode: string; max_backlog?: number | null; max_age_days?: number | null },
  o: Opts = {},
): Promise<PolicyBody> {
  return request(`/podcasts/${encodeURIComponent(id)}/policy`, {
    ...o,
    method: 'PUT',
    body: update,
  });
}

export async function clearPolicy(id: string, o: Opts = {}): Promise<{ cleared: boolean }> {
  return request(`/podcasts/${encodeURIComponent(id)}/policy`, { ...o, method: 'DELETE' });
}

// Service

export async function scheduler(o: Opts = {}): Promise<SchedulerStatus> {
  return request('/scheduler', o);
}

export async function pauseScheduler(reason: string | null, o: Opts = {}): Promise<unknown> {
  return request('/scheduler/pause', { ...o, method: 'POST', body: { reason } });
}

export async function resumeScheduler(o: Opts = {}): Promise<unknown> {
  return request('/scheduler/resume', { ...o, method: 'POST' });
}

export async function runSchedulerPass(
  o: Opts = {},
): Promise<{ due: number; started: number; paused: boolean }> {
  return request('/scheduler/run', { ...o, method: 'POST' });
}

export async function runMaintenance(
  o: Opts = {},
): Promise<{ events_pruned: number; cache_expired: number }> {
  return request('/scheduler/maintenance', { ...o, method: 'POST', timeoutMs: 0 });
}

export async function refreshAllPodcasts(o: Opts = {}): Promise<unknown> {
  return request('/podcasts/refresh', { ...o, method: 'POST', timeoutMs: 0 });
}

// Settings

export async function settings(o: Opts = {}): Promise<SettingsReport> {
  return request('/settings', o);
}

export async function setSetting(key: string, value: string, o: Opts = {}): Promise<unknown> {
  return request(`/settings/${encodeURIComponent(key)}`, {
    ...o,
    method: 'PUT',
    body: { value, updated_by: 'web' },
  });
}

export async function clearSetting(key: string, o: Opts = {}): Promise<{ cleared: boolean }> {
  return request(`/settings/${encodeURIComponent(key)}`, { ...o, method: 'DELETE' });
}

// Search

export async function searchLibrary(
  query: { q: string; kind?: string; limit?: number; prefix?: boolean },
  o: Opts = {},
): Promise<LibrarySearchResults> {
  return request('/search', { ...o, query });
}

export async function reindex(
  o: Opts = {},
): Promise<{ podcasts: number; episodes: number; duration_ms: number }> {
  return request('/search/reindex', { ...o, method: 'POST', timeoutMs: 0 });
}

// Discovery

export async function searchProviders(
  q: string,
  limit = 15,
  o: Opts = {},
): Promise<DiscoverySearchResponse> {
  return request('/discovery/search', { ...o, query: { q, limit }, timeoutMs: 30_000 });
}

export async function providers(o: Opts = {}): Promise<ProviderStatus[]> {
  return expectArray<ProviderStatus>(await request('/discovery/providers', o), 'providers');
}

export async function resolveFeed(input: string, o: Opts = {}): Promise<ResolvedFeed> {
  return request('/discovery/resolve', {
    ...o,
    method: 'POST',
    body: { input },
    timeoutMs: 30_000,
  });
}

// Events

export async function recentEvents(limit = 25, o: Opts = {}): Promise<UguisuEvent[]> {
  return expectArray<UguisuEvent>(await request('/events', { ...o, query: { limit } }), 'events');
}

/** The URL of the live event stream, excluding the noisiest kinds. */
export function eventStreamUrl(exclude: string[] = []): string {
  return url('/events', exclude.length > 0 ? { exclude: exclude.join(',') } : undefined);
}
