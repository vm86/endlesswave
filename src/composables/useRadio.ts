import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useSpectrum } from "./useSpectrum";
import { api, coverUrl, setBase, streamUrl, type AuthInfo, type StreamFormat, type GenresInfo, type Queue, type Stats } from "../api";

const POLL_MS = 2000;
const REFRESH_MIN_MS = 700;
// How far the player is behind the mix, about; the longest a skip is waited for.
const STREAM_LAG_MS = 3500;
const SKIP_GIVE_UP_MS = 30000;

/** The radio daemon (polled), the <audio> of its stream and the tray. */
export function useRadio() {
  const queue = ref<Queue | null>(null);
  const auth = ref<AuthInfo | null>(null);
  const genres = ref<GenresInfo | null>(null);
  const stats = ref<Stats | null>(null);
  const error = ref("");
  const playing = ref(false);
  // Between the click on Play and the first sound (and while the stream stalls).
  const loading = ref(false);
  const volume = ref(Number(localStorage.getItem("volume") ?? 80));
  const fetchedAt = ref(performance.now());
  const tick = ref(performance.now());

  const audio = new Audio();
  audio.preload = "none";
  const format = ref<StreamFormat>(localStorage.getItem("format") === "flac" ? "flac" : "mp3");
  // Names this connection to the stream (see the spectrum).
  const token = ref("");

  // Where the player's clock stands when the stream begins: a stream joined in
  // the middle may not start it at 0.
  let base = 0;
  audio.addEventListener("loadedmetadata", () => (base = audio.currentTime));
  const sync = () => (playing.value = !audio.paused);
  audio.addEventListener("playing", () => {
    loading.value = false;
    sync();
  });
  audio.addEventListener("waiting", () => (loading.value = !audio.paused));
  audio.addEventListener("pause", sync);
  audio.addEventListener("error", () => {
    loading.value = false;
    if (!audio.getAttribute("src")) return sync();
    const e = audio.error;
    const flac = audio.canPlayType("audio/flac") || "no";
    error.value = `Stream is not available (${format.value}: code ${e?.code} ${e?.message ?? ""}; can play flac: ${flac})`;
    console.warn("[stream]", format.value, e?.code, e?.message, "canPlayType flac:", flac);
    sync();
  });
  watch(volume, (v) => {
    audio.volume = v / 100;
    localStorage.setItem("volume", String(v));
  }, { immediate: true });

  const { bands: levels, note: eqNote } = useSpectrum({ playing, token, currentTime: () => audio.currentTime - base });

  function drop() {
    loading.value = false;
    audio.pause();
    audio.removeAttribute("src");
    audio.load(); // the stream is live: resume at its edge, not where it stopped
  }

  async function start() {
    loading.value = true;
    // Somebody paused the mix for everyone: listening alone would not wake it.
    if (queue.value?.paused && queue.value.pause_reason === "user") await guarded(api.resume);
    token.value = crypto.randomUUID();
    base = 0;
    audio.src = streamUrl(format.value, token.value);
    try {
      await audio.play();
      error.value = "";
    } catch (e) {
      loading.value = false;
      error.value = (e as Error).message;
    }
    sync();
  }

  async function toggle() {
    if (loading.value && audio.paused) return; // already connecting
    if (!audio.paused) {
      drop();
      sync();
    } else await start();
  }

  function stop() {
    if (audio.paused) return;
    drop();
    sync();
  }

  // Another stream while listening: connect to it at once.
  watch(format, (value) => {
    localStorage.setItem("format", value);
    if (!audio.paused) void start();
  });

  // The button "rebuild the queue": the answer is quick, so the spinner is held
  // a moment to be seen, and the result is told.
  const refreshing = ref(false);
  const notice = ref("");
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
      if (error.value.startsWith("Radio is not running")) error.value = "";
    } catch (e) {
      console.warn("radio request failed", e);
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
  const setBackend = (id: string, enabled: boolean) =>
    guarded(async () => {
      auth.value = await (enabled ? api.enableBackend(id) : api.disableBackend(id));
    });
  const signIn = (id: string, values: Record<string, string>) =>
    guarded(async () => {
      auth.value = await api.signIn(id, values);
    });

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
  // The system's Now Playing (Control Center on macOS, MPRIS on Linux) and the media
  // keys. The webview publishes the <audio> there by itself, so this is the ONE
  // place that says what plays: a second, native player would show up twice.
  const session = "mediaSession" in navigator ? navigator.mediaSession : null;
  watch(
    () => queue.value?.now?.track_id,
    () => {
      const now = queue.value?.now;
      if (!session) return;
      session.metadata = now
        ? new MediaMetadata({
            title: now.title,
            artist: now.artists.join(", "),
            album: "Yamusic Radio",
            artwork: now.cover ? [{ src: coverUrl(now.cover, 400), sizes: "400x400" }] : [],
          })
        : null;
    },
    { immediate: true },
  );
  watch(
    [playing, () => queue.value?.paused, () => queue.value?.now?.track_id],
    () => {
      if (!session) return;
      const now = queue.value?.now;
      session.playbackState = !now ? "none" : playing.value && !queue.value?.paused ? "playing" : "paused";
      try {
        if (now) session.setPositionState({ duration: now.duration_seconds, position: Math.min(elapsed.value, now.duration_seconds), playbackRate: 1 });
      } catch {
        /* a position the system does not take */
      }
    },
    { immediate: true },
  );
  if (session) {
    // A live radio: no previous track, nothing to seek in.
    session.setActionHandler("play", () => playing.value || void toggle());
    session.setActionHandler("pause", () => playing.value && void toggle());
    session.setActionHandler("stop", () => stop());
    session.setActionHandler("nexttrack", () => void next());
  }

  const timers: number[] = [];
  const unlisten: Array<() => void> = [];
  onMounted(async () => {
    unlisten.push(await listen("tray-play-pause", () => void toggle()));
    unlisten.push(await listen("tray-next", () => void next()));
    try {
      setBase(await invoke<string>("radio_url"));
      await invoke("radio_start");
    } catch (e) {
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
    audio.pause();
  });

  return { moveTrack, removeTrack, playTrack, refreshing, notice, skipping, loading, setLanguage, format, eqNote, levels, queue, auth, genres, stats, setBlocked, setBackend, signIn, error, playing, volume, elapsed, toggle, stop, next, refreshQueue, setMoods, setGenres };
}
