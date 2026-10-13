import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { onUnmounted, ref, watch, type Ref } from "vue";

/**
 * Who plays the stream: the native player (src-tauri/src/audio.rs: the system's audio output),
 * or the `<audio>` of the webview. The native one is the default; when it cannot start (no audio
 * device, a stream it does not understand) the `<audio>` plays instead, for that listening.
 */
export type Engine = "native" | "web";

// The first start of the radio (no track analyzed yet) can take a minute or more; the native player
// is given time for it, and falls back only on a real failure or after this long.
const START_TIMEOUT_MS = 300000;
const POSITION_POLL_MS = 200;

interface NativeStatus {
  state: "loading" | "playing" | "error" | "stopped";
  position: number;
  underruns: number;
  message: string;
}

export function usePlayer(volume: Ref<number>, notice: (text: string) => void) {
  /** What is chosen; `active` is what plays now (it differs after a fall back). */
  const engine = ref<Engine>(localStorage.getItem("engine") === "web" ? "web" : "native");
  const active = ref<Engine>(engine.value);
  const playing = ref(false);
  // Between the click on Play and the first sound (and while the stream stalls).
  const loading = ref(false);
  const error = ref("");

  // ---- The <audio> of the webview
  const audio = new Audio();
  audio.preload = "none";
  // Where the clock of <audio> stands when the stream begins: a stream joined in the middle may not
  // start it at 0.
  let base = 0;
  audio.addEventListener("loadedmetadata", () => (base = audio.currentTime));
  const syncWeb = () => {
    if (active.value === "web") playing.value = !audio.paused;
  };
  audio.addEventListener("playing", () => {
    loading.value = false;
    syncWeb();
  });
  audio.addEventListener("waiting", () => (loading.value = !audio.paused));
  audio.addEventListener("pause", syncWeb);
  audio.addEventListener("error", () => {
    loading.value = false;
    if (!audio.getAttribute("src")) return syncWeb();
    const e = audio.error;
    error.value = `Stream is not available (code ${e?.code} ${e?.message ?? ""})`;
    syncWeb();
  });

  function dropWeb() {
    audio.pause();
    audio.removeAttribute("src");
    audio.load(); // the stream is live: resume at its edge, not where it stopped
  }

  async function startWeb(url: string) {
    base = 0;
    audio.volume = volume.value / 100;
    audio.src = url;
    await audio.play();
  }

  // ---- The native player
  let position = 0;
  let positionAt = performance.now();
  let underruns = 0;
  let pending: { resolve: () => void; reject: (reason: string) => void } | null = null;
  let poll: ReturnType<typeof setInterval> | undefined;

  const stopPolling = () => {
    clearInterval(poll);
    poll = undefined;
  };
  function startPolling() {
    stopPolling();
    poll = setInterval(async () => {
      try {
        const status = await invoke<NativeStatus>("player_status");
        position = status.position;
        positionAt = performance.now();
        if (status.underruns > underruns) console.warn(`[player] the output ran dry (${status.underruns} times), at ${status.position.toFixed(1)} s`);
        underruns = status.underruns;
      } catch {
        /* the next one */
      }
    }, POSITION_POLL_MS);
  }

  const unlisten: Promise<UnlistenFn> = listen<{ state: NativeStatus["state"]; message: string }>("player", (event) => {
    if (active.value !== "native") return;
    const { state, message } = event.payload;
    if (state === "playing") {
      loading.value = false;
      playing.value = true;
      underruns = 0;
      position = 0;
      positionAt = performance.now();
      startPolling();
      pending?.resolve();
      pending = null;
    } else if (state === "error") {
      loading.value = false;
      playing.value = false;
      stopPolling();
      if (pending) {
        pending.reject(message);
        pending = null;
      } else {
        error.value = message; // it played, and then the stream was lost
      }
    } else if (state === "stopped") {
      loading.value = false;
      playing.value = false;
      stopPolling();
    }
  });

  function startNative(url: string): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        pending = null;
        reject(`no sound in ${START_TIMEOUT_MS / 1000} s`);
      }, START_TIMEOUT_MS);
      pending = {
        resolve: () => {
          clearTimeout(timer);
          resolve();
        },
        reject: (reason) => {
          clearTimeout(timer);
          reject(reason);
        },
      };
      invoke("player_start", { url, volume: volume.value / 100 }).catch((e) => {
        clearTimeout(timer);
        pending = null;
        reject(String(e));
      });
    });
  }

  // ---- What the page uses
  /** Starts the stream: `urls` has the address for each engine. */
  async function start(urls: Record<Engine, string>) {
    loading.value = true;
    error.value = "";
    await stopAll();
    active.value = engine.value;
    if (active.value === "native") {
      try {
        await startNative(urls.native);
        return;
      } catch (reason) {
        console.warn("[player] the native player failed:", reason);
        await invoke("player_stop").catch(() => undefined);
        notice(`Native audio is not available (${reason}): playing with the browser`);
        active.value = "web";
        void invoke("media_enabled", { enabled: false });
      }
    }
    try {
      await startWeb(urls.web);
    } catch (e) {
      loading.value = false;
      error.value = (e as Error).message;
    }
    syncWeb();
  }

  async function stopAll() {
    stopPolling();
    pending?.reject("stopped");
    pending = null;
    dropWeb();
    await invoke("player_stop").catch(() => undefined);
  }

  async function stop() {
    loading.value = false;
    playing.value = false;
    await stopAll();
  }

  /** Seconds of the stream that were played since it was connected. */
  function currentTime(): number {
    if (active.value === "native") return position + (playing.value ? (performance.now() - positionAt) / 1000 : 0);
    return audio.currentTime - base;
  }

  watch(volume, (v) => {
    audio.volume = v / 100;
    void invoke("player_volume", { volume: v / 100 });
  }, { immediate: true });

  // The system's own controls (Now Playing) belong to the native player; the webview shows itself
  // when it plays, and a second one would show up.
  watch(active, (value) => void invoke("media_enabled", { enabled: value === "native" }), { immediate: true });

  watch(engine, (value) => localStorage.setItem("engine", value));

  onUnmounted(() => {
    void stopAll();
    void unlisten.then((f) => f());
  });

  return { engine, active, playing, loading, error, start, stop, currentTime };
}
