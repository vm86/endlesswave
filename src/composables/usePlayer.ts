import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { onUnmounted, ref, watch, type Ref } from "vue";
import type { NowTrack } from "../api";

const START_TIMEOUT_MS = 300000;
const POSITION_POLL_MS = 200;

interface NativeStatus {
  state: "loading" | "playing" | "error" | "stopped";
  mode: "radio" | "dj";
  position: number;
  mix_position: number | null;
  stream_epoch: number;
  paused: boolean;
  underruns: number;
  message: string;
  connection: string;
  volume: number;
  track: (NowTrack & { tempo?: number }) | null;
}

/** One native output serves both the stream and the console; external deck controls are observed even while paused. */
export function usePlayer(volume: Ref<number>) {
  const playing = ref(false);
  const loading = ref(false);
  const error = ref("");
  const mode = ref<NativeStatus["mode"]>("radio");
  const track = ref<NativeStatus["track"]>(null);
  const connection = ref("");
  const sourcePaused = ref(false);
  const streamEpoch = ref(0);
  let position = 0;
  let mixPosition: number | null = null;
  let positionAt = performance.now();
  let underruns = 0;
  let pending: { resolve: () => void; reject: (reason: string) => void } | null = null;
  let poll: ReturnType<typeof setInterval> | undefined;
  let disposed = false;
  let operation = 0;

  function take(status: NativeStatus, syncVolume = false) {
    if (disposed) return;
    const enteredConsole = mode.value !== "dj" && status.mode === "dj";
    mode.value = status.mode;
    track.value = status.track;
    position = status.position;
    mixPosition = status.mix_position;
    streamEpoch.value = status.stream_epoch;
    sourcePaused.value = status.paused;
    positionAt = performance.now();
    connection.value = status.connection;
    if (syncVolume && Number.isFinite(status.volume)) volume.value = Math.round(status.volume * 100);
    if (status.underruns > underruns) console.warn(`[player] the output ran dry (${status.underruns} times), at ${position.toFixed(1)} s`);
    underruns = status.underruns;
    playing.value = status.state === "playing";
    loading.value = status.state === "loading";
    if (status.state === "playing" || (enteredConsole && status.state === "stopped")) {
      pending?.resolve();
      pending = null;
    } else if (status.state === "error") {
      if (pending) { pending.reject(status.message); pending = null; }
      else error.value = status.message;
    }
  }

  const listeners: Array<Promise<UnlistenFn>> = [
    listen<NativeStatus>("player", (event) => take(event.payload, true)),
    listen<number>("player-volume", (event) => { volume.value = Math.round(event.payload * 100); }),
  ];
  poll = setInterval(() => {
    void invoke<NativeStatus>("player_status").then((status) => take(status)).catch(() => {});
  }, POSITION_POLL_MS);

  function startNative(url: string): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => { pending = null; reject(`no sound in ${START_TIMEOUT_MS / 1000} s`); }, START_TIMEOUT_MS);
      pending = {
        resolve: () => { clearTimeout(timer); resolve(); },
        reject: (reason) => { clearTimeout(timer); reject(reason); },
      };
      invoke<string>("player_start", { url, volume: volume.value / 100 }).catch((e) => {
        clearTimeout(timer); pending = null; reject(String(e));
      });
    });
  }

  async function start(url: string) {
    const mine = ++operation;
    error.value = "";
    pending?.reject("stopped"); pending = null;
    loading.value = true;
    try {
      await startNative(url);
    } catch (reason) {
      if (mine !== operation) return;
      console.warn("[player] failed:", reason);
      loading.value = false;
      if (reason !== "stopped") error.value = `Cannot play: ${reason}`;
    }
  }

  async function stop() {
    operation++;
    pending?.reject("stopped"); pending = null;
    loading.value = false;
    playing.value = false;
    await invoke("player_stop").catch(() => undefined);
  }

  function currentTime(): number {
    return position + (playing.value && mode.value === "radio" ? (performance.now() - positionAt) / 1000 : 0);
  }
  function mixTime(): number | null {
    return mixPosition === null ? null : mixPosition + (playing.value && mode.value === "radio" ? (performance.now() - positionAt) / 1000 : 0);
  }
  function trackTime(): number {
    return (track.value?.elapsed_seconds ?? 0) + (playing.value ? (performance.now() - positionAt) / 1000 * (track.value?.tempo ?? 1) : 0);
  }

  watch(volume, (v) => void invoke("player_volume", { volume: v / 100 }), { immediate: true });
  onUnmounted(() => {
    disposed = true;
    clearInterval(poll);
    pending?.reject("stopped"); pending = null;
    void invoke("player_stop").catch(() => undefined);
    for (const unlisten of listeners) void unlisten.then((f) => f());
  });
  return { playing, loading, error, mode, track, connection, sourcePaused, streamEpoch, start, stop, currentTime, mixTime, trackTime };
}
