import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { usePlayer } from "./usePlayer";
import { useSpectrum } from "./useSpectrum";
import { api, coverUrl, setBase, streamUrls, type AuthInfo, type StreamFormat, type GenresInfo, type Queue, type Stats } from "../api";

const POLL_MS = 2000;
const REFRESH_MIN_MS = 700;
// How far the player is behind the mix, about; the longest a skip is waited for.
const STREAM_LAG_MS = 3500;
const SKIP_GIVE_UP_MS = 30000;

/** The radio daemon (polled), the player of its stream (see usePlayer) and the tray. */
export function useRadio() {
  const queue = ref<Queue | null>(null);
  const auth = ref<AuthInfo | null>(null);
  const genres = ref<GenresInfo | null>(null);
  const stats = ref<Stats | null>(null);
  const error = ref("");
  // The service is starting: its first answer takes a few seconds, and that is not an error.
  const starting = ref(true);
  const STARTUP_MS = 60_000;
  let startedAt = performance.now();
  const volume = ref(Number(localStorage.getItem("volume") ?? 80));
  const fetchedAt = ref(performance.now());
  const tick = ref(performance.now());

  const format = ref<StreamFormat>(localStorage.getItem("format") === "flac" ? "flac" : "mp3");
  // Names this connection to the stream (see the spectrum).
  const token = ref("");

  // Who plays (the native player, or the <audio> of the webview as the fall back).
  const notice = ref("");
  const player = usePlayer(volume, (text) => (notice.value = text));
  const { playing, loading } = player;
  watch(player.error, (text) => text && (error.value = text));
  watch(volume, (v) => localStorage.setItem("volume", String(v)));

  const { bands: levels, note: eqNote } = useSpectrum({ playing, token, currentTime: () => player.currentTime() });

  async function start() {
    loading.value = true;
    // Somebody paused the mix for everyone: listening alone would not wake it.
    if (queue.value?.paused && queue.value.pause_reason === "user") await guarded(api.resume);
    token.value = crypto.randomUUID();
    await player.start(streamUrls(format.value, token.value));
  }

  async function toggle() {
    if (loading.value && !playing.value) return; // already connecting
    if (playing.value) await player.stop();
    else await start();
  }

  async function stop() {
    if (playing.value || loading.value) await player.stop();
  }

  // Another stream, or another player, while listening: connect at once.
  watch(format, (value) => {
    localStorage.setItem("format", value);
    if (playing.value) void start();
  });
  watch(player.engine, () => {
    if (playing.value) void start();
  });
  /** The choice of the player. Choosing the one that is chosen but fell back tries it again. */
  function setEngine(value: "native" | "web") {
    if (value === player.engine.value && player.active.value !== value && (playing.value || loading.value)) void start();
    else player.engine.value = value;
  }

  // The button "rebuild the queue": the answer is quick, so the spinner is held
  // a moment to be seen, and the result is told.
  const refreshing = ref(false);
  const refreshQueue = async () => {
    if (refreshing.value) return;
    refreshing.value = true;
    const started = performance.now();
    const q = await guarded(api.refreshQueue);
    if (q) {
      take(q);
      notice.value = q.next.length ? `Queue rebuilt · ${q.next.length} tracks next` : "Queue rebuilt · the radio is picking tracks";
    }
    setTimeout(() => (refreshing.value = false), Math.max(0, REFRESH_MIN_MS - (performance.now() - started)));
  };

  async function guarded<T>(action: () => Promise<T>): Promise<T | undefined> {
    try {
      return await action();
    } catch (e) {
      error.value = String(e instanceof Error ? e.message : e);
    }
  }

  // The next track button stays pressed until the new track is *heard*: the radio
  // says it changed ~3 s before the player (that far behind the mix) plays it.
  const skipping = ref(false);
  let skipFrom: string | undefined;
  let skipTimer: ReturnType<typeof setTimeout> | undefined;
  const release = (delay = 0) => {
    clearTimeout(skipTimer);
    skipTimer = setTimeout(() => (skipping.value = false), delay);
  };

  // Anything that makes the radio go to another track: the button, or a track asked for.
  async function skip(call: () => Promise<Queue>) {
    if (skipping.value) return;
    skipping.value = true;
    skipFrom = queue.value?.now?.track_id;
    release(SKIP_GIVE_UP_MS); // in case the new track never shows
    const q = await guarded(call);
    if (q) take(q);
    else release(); // refused (nothing plays yet, or a blend is under way)
  }
  const next = () => skip(api.nextTrack);
  const playTrack = (id: string) => skip(() => api.playTrack(id));

  // Changes of the queue by hand show at once, and the radio is told: if it
  // refuses (a blend is under way), the queue is read again and the toast says why.
  async function change(optimistic: (next: Queue["next"]) => Queue["next"], call: () => Promise<Queue>) {
    const q0 = queue.value;
    if (q0) queue.value = { ...q0, next: optimistic(q0.next) };
    const q = await guarded(call);
    if (q) take(q);
    else void refresh();
  }
  const moveTrack = (id: string, position: number) =>
    change(
      (list) => {
        const from = list.findIndex((t) => t.track_id === id);
        if (from < 0) return list;
        const copy = [...list];
        const [moved] = copy.splice(from, 1);
        copy.splice(Math.max(0, Math.min(position, copy.length)), 0, { ...moved!, pinned: true });
        return copy;
      },
      () => api.moveTrack(id, position),
    );
  const removeTrack = (id: string) =>
    change(
      (list) => list.filter((t) => t.track_id !== id),
      () => api.removeTrack(id),
    );

  watch(
    () => queue.value?.now?.track_id,
    (id) => {
      if (skipping.value && id !== skipFrom) release(playing.value ? STREAM_LAG_MS : 0);
    },
  );

  function take(q: Queue) {
    queue.value = q;
    fetchedAt.value = performance.now();
  }

  async function refresh() {
    try {
      const [a, q, g, st] = await Promise.all([api.auth(), api.queue(), api.genres(), api.stats()]);
      auth.value = a;
      genres.value = g;
      stats.value = st;
      take(q);
      starting.value = false;
      if (error.value.startsWith("Radio is not running")) error.value = "";
    } catch (e) {
      console.warn("radio request failed", e);
      if (starting.value && performance.now() - startedAt < STARTUP_MS) return;
      starting.value = false;
      error.value = `Radio is not running: ${e instanceof Error ? e.message : e}`;
    }
  }

  const setMoods = (moods: string[]) =>
    guarded(async () => {
      await api.setMoods(moods);
      take(await api.refreshQueue());
    });
  const setGenres = (list: string[]) =>
    guarded(async () => {
      await api.setGenres(list);
      take(await api.refreshQueue());
    });

  const setBlocked = (list: string[]) =>
    guarded(async () => {
      await api.setBlocked(list);
      take(await api.refreshQueue());
    });
  const setLanguage = (language: string | null) =>
    guarded(async () => {
      await api.setLanguage(language);
      take(await api.refreshQueue());
    });
  const setBackend = async (id: string, enabled: boolean): Promise<boolean> =>
    (await guarded(async () => {
      auth.value = await (enabled ? api.enableBackend(id) : api.disableBackend(id));
      return true;
    })) === true;
  const signIn = async (id: string, values: Record<string, string>): Promise<boolean> =>
    (await guarded(async () => {
      auth.value = await api.signIn(id, values);
      return true;
    })) === true;

  const elapsed = computed(() => {
    const now = queue.value?.now;
    if (!now) return 0;
    if (queue.value?.paused) return now.elapsed_seconds;
    return Math.min(now.duration_seconds, now.elapsed_seconds + (tick.value - fetchedAt.value) / 1000);
  });

  const title = computed(() => {
    const now = queue.value?.now;
    return now ? `${now.artists.join(", ")} — ${now.title}` : "";
  });

  // Tray label and the media keys / lock screen follow the track.
  watch([title, playing], () => {
    void invoke("update_tray", { title: title.value, playing: playing.value });
  });
  // The system's Now Playing (Control Center on macOS, MPRIS on Linux) and the media keys. ONE
  // of two says what plays, or a second player would show up in the system: the native layer
  // (src-tauri/src/media.rs) while the native player plays, and the webview (navigator.mediaSession,
  // which publishes the <audio> by itself) when the <audio> plays.
  const session = "mediaSession" in navigator ? navigator.mediaSession : null;
  const trackId = () => queue.value?.now?.track_id;
  watch(
    [trackId, player.active],
    () => {
      const now = queue.value?.now;
      if (player.active.value === "native") {
        if (session) {
          session.metadata = null;
          session.playbackState = "none";
        }
        if (now) {
          void invoke("media_metadata", {
            title: now.title,
            artist: now.artists.join(", "),
            cover: coverUrl(now.cover, 400),
            duration: now.duration_seconds,
          });
        }
      } else if (session) {
        session.metadata = now
          ? new MediaMetadata({
              title: now.title,
              artist: now.artists.join(", "),
              album: "EndlessWave",
              artwork: now.cover ? [{ src: coverUrl(now.cover, 400), sizes: "400x400" }] : [],
            })
          : null;
      }
    },
    { immediate: true },
  );
  watch(
    [playing, () => queue.value?.paused, trackId, player.active],
    () => {
      const now = queue.value?.now;
      const state = !now ? "none" : playing.value && !queue.value?.paused ? "playing" : "paused";
      if (player.active.value === "native") {
        void invoke("media_playback", { state: state === "none" ? "stopped" : state, progress: elapsed.value });
      } else if (session) {
        session.playbackState = state;
        try {
          if (now) session.setPositionState({ duration: now.duration_seconds, position: Math.min(elapsed.value, now.duration_seconds), playbackRate: 1 });
        } catch {
          /* a position the system does not take */
        }
      }
    },
    { immediate: true },
  );
  if (session) {
    // A live radio: no previous track, nothing to seek in.
    session.setActionHandler("play", () => playing.value || void toggle());
    session.setActionHandler("pause", () => playing.value && void toggle());
    session.setActionHandler("stop", () => void stop());
    session.setActionHandler("nexttrack", () => void next());
  }

  const timers: number[] = [];
  const unlisten: Array<() => void> = [];
  onMounted(async () => {
    unlisten.push(await listen("tray-play-pause", () => void toggle()));
    unlisten.push(await listen("tray-next", () => void next()));
    // The system's controls of the native layer: the media keys, Control Center, MPRIS (`playerctl`).
    unlisten.push(await listen("media-toggle", () => void toggle()));
    unlisten.push(await listen("media-play", () => playing.value || void toggle()));
    unlisten.push(await listen("media-pause", () => playing.value && void toggle()));
    unlisten.push(await listen("media-stop", () => void stop()));
    unlisten.push(await listen("media-next", () => void next()));
    // The app starts the service again when it dies: that is a wait, not an error.
    unlisten.push(
      await listen<string>("radio", (event) => {
        if (event.payload === "restarting") {
          starting.value = true;
          startedAt = performance.now();
        } else {
          starting.value = false;
          error.value = "The radio service stopped and could not be started again. Restart the app.";
        }
      }),
    );
    try {
      setBase(await invoke<string>("radio_url"));
      await invoke("radio_start");
    } catch (e) {
      starting.value = false;
      error.value = String(e);
    }
    void refresh();
    timers.push(window.setInterval(() => void refresh(), POLL_MS));
    timers.push(window.setInterval(() => (tick.value = performance.now()), 500));
    // A skip is on its way: look at the radio more often.
    timers.push(window.setInterval(() => skipping.value && void refresh(), 600));
  });
  onUnmounted(() => {
    timers.forEach(clearInterval);
    unlisten.forEach((f) => f());
  });

  return { setEngine, starting, engine: player.engine, activeEngine: player.active, moveTrack, removeTrack, playTrack, refreshing, notice, skipping, loading, setLanguage, format, eqNote, levels, queue, auth, genres, stats, setBlocked, setBackend, signIn, error, playing, volume, elapsed, toggle, stop, next, refreshQueue, setMoods, setGenres };
}
