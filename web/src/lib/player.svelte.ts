// What the player is playing, for the whole application.
//
// The audio element lives in the shell so playback survives navigation; this
// module only records which episode it was told to play. Nothing here talks
// to the API beyond asking for the episode's media URL.

import { mediaUrl } from './api';

export interface NowPlaying {
  episodeId: string;
  title: string;
  podcastTitle: string;
  src: string;
}

class Player {
  #current = $state<NowPlaying | null>(null);

  get current(): NowPlaying | null {
    return this.#current;
  }

  /** Points the shell's audio element at an archived episode. */
  play(episode: { id: string; title: string }, podcastTitle: string): void {
    this.#current = {
      episodeId: episode.id,
      title: episode.title,
      podcastTitle,
      src: mediaUrl(episode.id),
    };
  }

  /** Clears the player; the element stops and the bar disappears. */
  close(): void {
    this.#current = null;
  }
}

/** The application's single player. */
export const player = new Player();
