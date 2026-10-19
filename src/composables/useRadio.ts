import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { usePlayer } from "./usePlayer";
import { useSpectrum } from "./useSpectrum";
import { api, coverUrl, setBase, streamUrl, type AuthInfo, type StreamFormat, type GenresInfo, type LikesInfo, type Queue, type Stats } from "../api";

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

  const notice = ref("");
  const player = usePlayer(volume);
  const { playing, loading } = player;
  watch(player.error, (text) => text && (error.value = text));
  watch(volume, (v) => localStorage.setItem("volume", String(v)));

  const { bands: levels, note: eqNote } = useSpectrum({ playing, token, currentTime: () => player.currentTime() });

  async function start() {
    loading.value = true;
    // Somebody paused the mix for everyone: listening alone would not wake it.
    if (queue.value?.paused && queue.value.pause_reason === "user") await guarded(api.resume);
    token.value = crypto.randomUUID();
    await player.start(streamUrl(format.value, token.value));
  }

  async function toggle() {
    if (loading.value && !playing.value) return; // already connecting
    if (playing.value) await player.stop();
    else await start();
  }

  async function stop() {
    if (playing.value || loading.value) await player.stop();
  }

  // Another stream while listening: connect at once.
  watch(format, (value) => {
    localStorage.setItem("format", value);
    if (playing.value) void start();
  });
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

  /** The button "rebuild the queue" held for a few seconds: the whole queue is dropped and loaded anew. */
  const clearQueue = async () => {
    if (refreshing.value) return;
    refreshing.value = true;
    const started = performance.now();
    const q = await guarded(api.clearQueue);
    if (q) {
      take(q);
      notice.value = "Queue cleared · loading the tracks anew";
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
  /** Likes of the backend: shown as a heart on the track that plays. */
  const likes = ref<LikesInfo>({ supported: false, liked: [] });
  const toggleLike = async (id: string) => {
    const liked = !likes.value.liked.includes(id);
    const before = likes.value;
    likes.value = { ...before, liked: liked ? [...before.liked, id] : before.liked.filter((x) => x !== id) };
    try {
      likes.value = await api.setLike(id, liked);
    } catch (e) {
      likes.value = before;
      notice.value = `Could not ${liked ? "like" : "unlike"} the track: ${e instanceof Error ? e.message : e}`;
    }
  };
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

  // While a choice of the mix (mood, language, genres) is on its way to the radio, what the poll brings
  // would put the old choice back for a moment: the chips would jump back and forth.
  const holding = ref(0);

  /** The service is about to be restarted (the reset): a wait, not an error. */
  function expectRestart() {
    starting.value = true;
    startedAt = performance.now();
  }

  function take(q: Queue) {
    const mine = queue.value;
    if (holding.value > 0 && mine) {
      q = {
        ...q,
        moods: { ...q.moods, selected: mine.moods.selected },
        language: q.language && mine.language ? { ...q.language, selected: mine.language.selected } : q.language,
      };
    }
    queue.value = q;
    fetchedAt.value = performance.now();
  }

  /** The genres as the radio lists them, in the order the chips already have: a chip does not move when it is picked. */
  function steady(prev: GenresInfo | null, next: GenresInfo): GenresInfo {
    if (!prev) return next;
    const known = prev.available.filter((g) => next.available.includes(g));
    return { ...next, available: [...known, ...next.available.filter((g) => !known.includes(g))] };
  }

  async function refresh() {
    try {
      const [a, q, g, st, l] = await Promise.all([api.auth(), api.queue(), api.genres(), api.stats(), api.likes()]);
      auth.value = a;
      likes.value = l;
      if (holding.value === 0) genres.value = steady(genres.value, g);
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

  /**
   * A choice of the mix shows at once, and the radio is told. Then the queue is built again for it
   * (that takes a moment, and the chips must not wait for it).
   */
  async function tune(apply: () => void, call: () => Promise<void>) {
    holding.value++;
    apply();
    try {
      await call();
      take(await api.refreshQueue());
    } catch (e) {
      error.value = String(e instanceof Error ? e.message : e);
      void refresh(); // what the radio really has
    } finally {
      setTimeout(() => holding.value--, 700);
    }
  }
  const setMoods = (moods: string[]) =>
    tune(
      () => queue.value && (queue.value = { ...queue.value, moods: { ...queue.value.moods, selected: moods } }),
      async () => void (await api.setMoods(moods)),
    );
  const setGenres = (list: string[]) =>
    tune(
      () => genres.value && (genres.value = { ...genres.value, selected: list }),
      async () => void (genres.value = steady(genres.value, await api.setGenres(list))),
    );
  const setBlocked = (list: string[]) =>
    tune(
      () => genres.value && (genres.value = { ...genres.value, blocked: list }),
      async () => void (genres.value = steady(genres.value, await api.setBlocked(list))),
    );
  const setLanguage = (language: string | null) =>
    tune(
      () => queue.value?.language && (queue.value = { ...queue.value, language: { ...queue.value.language, selected: language } }),
      async () => void (await api.setLanguage(language)),
    );
  const setBackend = async (id: string, enabled: boolean): Promise<boolean> =>
    (await guarded(async () => {
      auth.value = await (enabled ? api.enableBackend(id) : api.disableBackend(id));
      return true;
    })) === true;
  const setBackendSettings = async (id: string, values: Record<string, string>): Promise<boolean> =>
    (await guarded(async () => {
      auth.value = await api.setBackendSettings(id, values);
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
  // The system's Now Playing (Control Center on macOS, MPRIS on Linux) and the media keys: the native layer
  // (src-tauri/src/media.rs) is told what plays.
  const trackId = () => queue.value?.now?.track_id;
  watch(
    trackId,
    () => {
      const now = queue.value?.now;
      if (now) {
        void invoke("media_metadata", {
          title: now.title,
          artist: now.artists.join(", "),
          cover: coverUrl(now.cover, 400),
          duration: now.duration_seconds,
        });
      }
    },
    { immediate: true },
  );
  watch(
    [playing, () => queue.value?.paused, trackId],
    () => {
      const now = queue.value?.now;
      const state = !now ? "stopped" : playing.value && !queue.value?.paused ? "playing" : "paused";
      void invoke("media_playback", { state, progress: elapsed.value });
    },
    { immediate: true },
  );

  const timers: number[] = [];
  const unlisten: Array<() => void> = [];
  onMounted(async () => {
    // The app is quitting: the radio stops quickly when nobody listens to it.
    unlisten.push(await listen("quit", () => void player.stop()));
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

  return { clearQueue, likes, toggleLike, expectRestart, starting, moveTrack, removeTrack, playTrack, refreshing, notice, skipping, loading, setLanguage, format, eqNote, levels, queue, auth, genres, stats, setBlocked, setBackend, setBackendSettings, signIn, error, playing, volume, elapsed, toggle, stop, next, refreshQueue, setMoods, setGenres };
}
