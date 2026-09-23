import { invoke } from "@tauri-apps/api/core";

export interface Account {
  uid: string;
  displayName: string;
  hasPlus: boolean;
}

export interface Track {
  id: string;
  title: string;
  artist: string;
  album: string;
  coverUrl: string | null;
  coverThumbUrl: string | null;
  durationMs: number;
  available: boolean;
  /** Podcast episodes only: ISO release date. */
  pubDate?: string | null;
}

export interface Podcast {
  id: string;
  title: string;
  episodeCount: number;
  coverThumbUrl: string | null;
}

export interface Playable {
  track: Track;
  url: string;
}

export interface Settings {
  menuBarMode: boolean;
  alwaysOnTop: boolean;
  miniPlayer: boolean;
  miniFade: boolean;
  island: boolean;
  stationId: string | null;
  stationName: string | null;
}

/** Where the island sits; `notch` is false for the free-standing pill. */
export interface IslandGeometry {
  notch: boolean;
  notchWidth: number;
  notchHeight: number;
  width: number;
  height: number;
}

export interface Station {
  id: string;
  kind: string;
  name: string;
  iconUrl: string | null;
  color: string | null;
  /** From the personalised dashboard rather than the full catalogue. */
  featured: boolean;
}

export interface DeviceCode {
  device_code: string;
  user_code: string;
  verification_url: string;
  interval: number;
  expires_in: number;
}

/** Feedback kinds the rotor understands. */
export type Feedback = "trackStarted" | "trackFinished" | "skip";

export const api = {
  authRestore: () => invoke<Account | null>("auth_restore"),
  authBegin: () => invoke<DeviceCode>("auth_begin"),
  authPoll: () => invoke<Account | null>("auth_poll"),
  authLogout: () => invoke<void>("auth_logout"),

  stations: () => invoke<Station[]>("stations"),
  waveSetStation: (id: string, name: string) =>
    invoke<Settings>("wave_set_station", { id, name }),

  waveNext: () => invoke<Playable | null>("wave_next"),
  waveQueue: () => invoke<Track[]>("wave_queue"),
  waveSkipTo: (skipAhead: number) =>
    invoke<Playable | null>("wave_skip_to", { skipAhead }),
  waveRestart: () => invoke<void>("wave_restart"),
  searchTracks: (text: string) => invoke<Track[]>("search_tracks", { text }),
  trackPlay: (track: Track) => invoke<Playable>("track_play", { track }),
  searchPodcasts: (text: string) => invoke<Podcast[]>("search_podcasts", { text }),
  podcastEpisodes: (id: string) => invoke<Track[]>("podcast_episodes", { id }),
  waveFeedback: (kind: Feedback, trackId?: string, playedSeconds?: number) =>
    invoke<void>("wave_feedback", { kind, trackId, playedSeconds }),

  likeTrack: (trackId: string, like: boolean) =>
    invoke<void>("like_track", { trackId, like }),

  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (settings: Settings) => invoke<Settings>("settings_set", { settings }),
  showWindow: () => invoke<void>("show_window"),
  miniMenu: () => invoke<void>("mini_menu"),
  mainMenu: (x: number, y: number) => invoke<void>("main_menu", { x, y }),
  islandResize: (expanded: boolean) =>
    invoke<IslandGeometry | null>("island_resize", { expanded }),
};
