/**
 * Turns the three events worth interrupting someone for into native toasts.
 *
 * Reads the event stream the app already has rather than opening a second
 * one, and asks the server for the title the same way any view would. Outside
 * the desktop shell `start` does nothing at all.
 */
import { events } from "./events.svelte";
import { notify, inDesktopShell } from "./api/desktop";
import { getDownload, getEpisode } from "./api";
import { m } from "./i18n";
import type { UguisuEvent } from "./api";

/** Only terminal outcomes: progress and discovery would be a torrent. */
const WATCHED = new Set([
  "download.completed",
  "download.failed",
  "podcast.feed.refresh.failed",
]);

/** What a finished job was for. A failure to find out is not worth surfacing. */
async function subject(
  jobId: unknown,
): Promise<{ title: string; podcast: string }> {
  const fallback = { title: m.notifications.someEpisode, podcast: m.app.name };
  if (typeof jobId !== "string") {
    return fallback;
  }
  try {
    const detail = await getDownload(jobId);
    const episode = await getEpisode(detail.job.episode_id);
    return { title: episode.episode.title, podcast: episode.podcast_title };
  } catch {
    return fallback;
  }
}

/** Raises the toast for one watched event. */
export async function announce(event: UguisuEvent): Promise<void> {
  const payload = event as unknown as Record<string, unknown>;
  switch (event.kind) {
    case "download.completed": {
      const { title, podcast } = await subject(payload.job_id);
      notify(podcast, m.notifications.archived(title));
      break;
    }
    case "download.failed": {
      const { title, podcast } = await subject(payload.job_id);
      // A failure that cannot be retried ends after one attempt.
      const attempts =
        typeof payload.attempts === "number" ? payload.attempts : 1;
      const outcome =
        attempts > 1
          ? m.notifications.gaveUp(attempts)
          : m.notifications.notRetried;
      notify(
        m.notifications.failedTitle(podcast),
        m.notifications.failed(title, outcome),
      );
      break;
    }
    case "podcast.feed.refresh.failed":
      notify(
        m.notifications.refreshFailedTitle,
        m.notifications.refreshFailed,
      );
      break;
    default:
      break;
  }
}

/** Subscribes for as long as the app is up. Returns an unsubscribe. */
export function start(): () => void {
  if (!inDesktopShell()) {
    return () => {};
  }
  return events.subscribe((event) => {
    if (WATCHED.has(event.kind)) {
      void announce(event);
    }
  });
}
