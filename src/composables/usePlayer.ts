import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { onUnmounted, ref, watch, type Ref } from "vue";

/**
 * The player: src-tauri/src/audio.rs reads the stream and plays it through the system's audio output;
 * the page starts and stops it, sets the volume and asks where it stands.
 */

// The first start of the radio (no track analyzed yet) can take a minute or more; the player
// is given time for it, and gives up only on a real failure or after this long.
const START_TIMEOUT_MS = 300000;
const POSITION_POLL_MS = 200;

interface NativeStatus {
  state: "loading" | "playing" | "error" | "stopped";
  position: number;
  underruns: number;
  message: string;
}

export function usePlayer(volume: Ref<number>) {
  const playing = ref(false);
  // Between the click on Play and the first sound (and while the stream stalls).
  const loading = ref(false);
  const error = ref("");

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
  async function start(url: string) {
    loading.value = true;
    error.value = "";
    await stopAll();
    try {
      await startNative(url);
    } catch (reason) {
      console.warn("[player] failed:", reason);
      await invoke("player_stop").catch(() => undefined);
      loading.value = false;
      playing.value = false;
      if (reason !== "stopped") error.value = `Cannot play: ${reason}`;
    }
  }

  async function stopAll() {
    stopPolling();
    pending?.reject("stopped");
    pending = null;
    await invoke("player_stop").catch(() => undefined);
  }

  async function stop() {
    loading.value = false;
    playing.value = false;
    await stopAll();
  }

  /** Seconds of the stream that were played since it was connected. */
  function currentTime(): number {
    return position + (playing.value ? (performance.now() - positionAt) / 1000 : 0);
  }

  watch(volume, (v) => void invoke("player_volume", { volume: v / 100 }), { immediate: true });

  onUnmounted(() => {
    void stopAll();
    void unlisten.then((f) => f());
  });

  return { playing, loading, error, start, stop, currentTime };
}
