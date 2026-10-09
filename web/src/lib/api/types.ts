// The API's shapes, re-exported from the generated schema under the names the
// views already use.
//
// Nothing here is written by hand any more: `generated/schema.d.ts` comes from
// `docs/api/openapi.json`, which comes from the Rust types (ADR 0039). A name
// on the left that the backend spells differently on the right is the only
// thing this file decides; `pnpm api:types` regenerates the schema and
// `web/scripts/check-api-types.mjs` fails when it is stale.

import type { components } from './generated/schema';

type S = components['schemas'];

/** Every enveloped body carries the API schema version. */
export type Schemad = S['Envelope'];

export type PodcastStatus = S['PodcastStatus'];
export type ArchiveState = S['ArchiveState'];
export type DownloadState = S['DownloadState'];
export type VerificationState = S['VerificationState'];
export type TagState = S['TagState'];
export type Priority = S['Priority'];
export type PolicyMode = S['PolicyMode'];
export type Origin = S['Origin'];
export type IndexState = S['IndexState'];
export type SearchOutcome = S['DiscoverySearchOutcome'];
export type LibrarySearchOutcome = S['SearchOutcome'];

export type Podcast = S['Podcast'];
export type FetchStatus = S['FetchStatus'];
export type PodcastSource = S['PodcastSource'];
export type PodcastDetail = S['PodcastDetail'];
export type PodcastPage = S['PodcastPage'];
export type Enclosure = S['Enclosure'];
export type Episode = S['Episode'];
export type EpisodePage = S['EpisodePage'];
export type EpisodeDetail = S['EpisodeDetail'];
export type DuplicateResolution = S['DuplicateResolution'];
export type DuplicateResolved = S['DuplicateResolved'];
export type FeedMove = S['FeedMove'];
export type PodcastRemoval = S['PodcastRemoval'];

export type DownloadJob = S['JobSummary'];
export type JobPage = S['JobPage'];
export type ProgressSnapshot = S['ProgressSnapshot'];
export type DownloadAttempt = S['DownloadAttempt'];
export type JobDetail = S['JobDetail'];
export type DownloadStats = S['DownloadStats'];

export type ArchiveFile = S['ArchiveFile'];
export type ArchivePage = S['ArchivePage'];
export type ArchiveStats = S['StatsBody'];
export type ArchiveManifest = S['ArchiveManifest'];
export type ArchivePolicy = S['ArchivePolicy'];
export type PolicyBody = S['PolicyBody'];
export type PodcastArtwork = S['PodcastArtwork'];

export type SchedulerStatus = S['SchedulerStatus'];
export type SearchIndexStatus = S['SearchIndexStatus'];
export type ServiceStatus = S['Status'];
export type KeyDescription = S['KeyDescription'];
export type SettingsReport = S['SettingsReport'];

export type SearchSignal = S['SearchSignal'];
export type RankedPodcast = S['RankedPodcast'];
export type RankedEpisode = S['RankedEpisode'];
export type LibrarySearchResults = S['SearchResults'];

export type RefreshReport = S['RefreshReport'];
export type EnqueueSummary = S['EnqueueSummary'];
export type EnqueueOutcome = S['EnqueueOutcome'];
export type UguisuEvent = S['Event'];

// Discovery (Phase 2). These shapes are documented in docs/DISCOVERY.md.

export type Signal = S['Signal'];
export type Candidate = S['PodcastCandidate'];
export type RankedCandidate = S['RankedCandidate'];
export type ProviderOutcome = S['ProviderOutcome'];
export type DiscoverySearchResponse = S['SearchResponse'];
export type ResolutionStep = S['ResolutionStep'];
export type ResolvedFeed = S['ResolvedFeed'];
export type ProviderStatus = S['ProviderStatus'];

export type AddOutcome = S['AddOutcome'];
export type OpmlImport = S['OpmlImport'];
export type ImportRequest = S['ImportRequest'];
export type ImportBody = S['ImportBody'];
export type ImportItem = S['ImportItem'];
export type RestoreRequest = S['RestoreRequest'];
export type RestoreBody = S['RestoreBody'];
export type RestoreLine = S['RestoreLine'];
export type OpmlItem = S['OpmlItem'];
export type OpmlAction = S['OpmlAction'];
export type VerifiedFile = S['VerifiedFile'];
export type VerifySummary = S['VerifySummary'];

// Authentication (Phase 9).

export type Session = S['SessionBody'];
export type LoginResult = S['LoginBody'];
export type SessionExchange = S['ExchangeBody'];
export type ApiTokenRecord = S['ApiToken'];
export type NewApiToken = S['NewTokenBody'];
export type TokenScope = S['Scope'];
