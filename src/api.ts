import { fetch } from "@tauri-apps/plugin-http";

export interface TrackInfo {
  track_id: string;
  title: string;
  artists: string[];
  bpm: number;
  duration_seconds: number;
  mood: string;
  camelot: string;
  /** The cover's URL with %% where the size goes, "" if there is none. */
  cover: string;
  /** Put where it is by hand: the radio does not move it. */
  pinned?: boolean;
  /** The backend the track is from: "yandex", "vk", "local". */
  source?: string;
}
export interface NowTrack extends TrackInfo {
  elapsed_seconds: number;
}
export interface MoodsInfo {
  available: string[];
  selected: string[];
  station: string;
  /** Whether the backends steer their stations by the mood. */
  steered?: boolean;
}
export interface GenresInfo {
  available: string[];
  selected: string[];
  applied: string[];
  blocked: string[];
  blocked_applied: string[];
}
export interface LanguageInfo {
  /** `any`, `russian`, `not-russian`, `without-words`. */
  available: string[];
  /** Picked here, null if none. */
  selected: string | null;
  station: string;
  /** Whether the backend can change it. */
  steered: boolean;
}
export interface Queue {
  previous: TrackInfo[];
  now: NowTrack | null;
  next: TrackInfo[];
  moods: MoodsInfo;
  language?: LanguageInfo;
  skip_pending: boolean;
  /** The mix stands (e.g. "no_listeners"): the time of the track does not run. */
  blending: boolean;
  can_skip: boolean;
  paused: boolean;
  pause_reason: string | null;
}
export interface AuthField {
  name: string;
  label: string;
  secret: boolean;
  /** The values to choose from, when the field is a choice and not a text. */
  choices?: { value: string; label: string }[];
}
export interface Backend {
  id: string;
  name: string;
  enabled: boolean;
  authorized: boolean;
  /** Signed in and not resting after failures. */
  available: boolean;
  url: string | null;
  code: string | null;
  /** Values to type to sign in; [] for a device flow. */
  fields: AuthField[];
  /** The settings it has (the folder of the local one) and what they are called. */
  settings?: Record<string, string>;
  setting_fields?: AuthField[];
}
export interface AuthInfo {
  authorized: boolean;
  url: string | null;
  code: string | null;
  fields: AuthField[];
  backends: Backend[];
}
export interface TransitionData {
  total: number;
  beat_matched: number;
  by_type: Record<string, number>;
  by_effect: Record<string, number>;
  by_key: Record<string, number>;
  by_quality: { good: number; fair: number; poor: number; unknown: number };
}
export interface Stats {
  session: TransitionData;
  all_time: TransitionData | null;
}

let base = "";
export const setBase = (url: string) => (base = url);
export const baseUrl = () => base;

/** The radio's streams: MP3 at 320 kbit/s, and lossless FLAC. */
export type StreamFormat = "mp3" | "flac";
/**
 * The address of the stream of a format. `token` names this listener: the radio says where in the mix its
 * stream starts (for the spectrum).
 */
export function streamUrl(format: StreamFormat, token: string): string {
  return `${base}${format === "flac" ? "/stream.flac" : "/stream"}?l=${encodeURIComponent(token)}&${Date.now()}`;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(base + path, init);
  if (!response.ok) throw new Error((await response.text()) || response.statusText);
  return (await response.json()) as T;
}
const json = (method: string, body?: unknown): RequestInit => ({
  method,
  headers: { "Content-Type": "application/json" },
  body: body === undefined ? undefined : JSON.stringify(body),
});

export interface LikesInfo {
  supported: boolean;
  liked: string[];
}

export const api = {
  auth: () => request<AuthInfo>("/api/auth"),
  queue: () => request<Queue>("/api/queue?previous=3&next=15"),
  genres: () => request<GenresInfo>("/api/genres"),
  stats: () => request<Stats>("/api/stats"),
  setLanguage: (language: string | null) =>
    request<LanguageInfo>("/api/language", language ? json("PUT", { language }) : json("DELETE")),
  pause: () => request<unknown>("/api/pause", { method: "POST" }),
  resume: () => request<unknown>("/api/resume", { method: "POST" }),
  enableBackend: (backend: string) => request<AuthInfo>("/api/auth/backend", json("PUT", { backend })),
  disableBackend: (id: string) =>
    request<AuthInfo>(`/api/auth/backend/${encodeURIComponent(id)}`, { method: "DELETE" }),
  setBackendSettings: (backend: string, values: Record<string, string>) =>
    request<AuthInfo>(`/api/auth/backend/${encodeURIComponent(backend)}/settings`, json("PUT", { values })),
  signIn: (backend: string, values: Record<string, string>) =>
    request<AuthInfo>("/api/auth", json("POST", { backend, values })),
  setBlocked: (genres: string[]) =>
    request<GenresInfo>("/api/genres/blocked", genres.length ? json("PUT", { genres }) : json("DELETE")),
  moveTrack: (track_id: string, position: number) =>
    request<Queue>("/api/queue/move", json("POST", { track_id, position })),
  removeTrack: (trackId: string) =>
    request<Queue>(`/api/queue/tracks/${encodeURIComponent(trackId)}`, { method: "DELETE" }),
  playTrack: (track_id: string) => request<Queue>("/api/queue/play", json("POST", { track_id })),
  likes: () => request<LikesInfo>("/api/likes"),
  setLike: (track_id: string, liked: boolean) => request<LikesInfo>("/api/likes", json("PUT", { track_id, liked })),
  nextTrack: () => request<Queue>("/api/next", { method: "POST" }),
  refreshQueue: () => request<Queue>("/api/queue/refresh", { method: "POST" }),
  setMoods: (moods: string[]) =>
    request<MoodsInfo>("/api/moods", moods.length ? json("PUT", { moods }) : json("DELETE")),
  setGenres: (genres: string[]) =>
    request<GenresInfo>("/api/genres", genres.length ? json("PUT", { genres }) : json("DELETE")),
};

export const coverUrl = (template: string, size: number) =>
  template ? template.replace("%%", `${size}x${size}`) : "";

export const clock = (seconds: number) => {
  const total = Math.max(0, Math.round(seconds));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
};
