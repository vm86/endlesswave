import { invoke } from "@tauri-apps/api/core";
import { ref, watch } from "vue";

/** The equalizer of the native player (src-tauri/src/equalizer.rs): ten bands, the gain of each in dB. */
export const BANDS = ["32", "64", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
export const MAX_GAIN = 12;

export const PRESETS: Record<string, number[]> = {
  Flat: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  "Bass boost": [6, 5, 4, 2, 0, 0, 0, 0, 0, 0],
  "Treble boost": [0, 0, 0, 0, 0, 1, 2, 4, 5, 6],
  Vocal: [-2, -2, -1, 1, 3, 4, 3, 1, 0, -1],
  Loudness: [5, 4, 1, 0, -1, 0, -1, 0, 3, 4],
  Rock: [4, 3, 2, -1, -2, -1, 2, 3, 4, 4],
  Electronic: [5, 4, 1, 0, -2, 2, 1, 1, 4, 5],
};

const KEY = "equalizer";
const clean = (gains: unknown): number[] =>
  BANDS.map((_, i) => {
    const g = Array.isArray(gains) ? Number(gains[i]) : 0;
    return Number.isFinite(g) ? Math.max(-MAX_GAIN, Math.min(MAX_GAIN, Math.round(g))) : 0;
  });

function load(): number[] {
  try {
    return clean(JSON.parse(localStorage.getItem(KEY) ?? "null"));
  } catch {
    return clean(null);
  }
}

// One state for the whole page: the gains are kept, and handed to the player whenever they change.
const gains = ref<number[]>(load());
let started = false;

function push(value: number[]) {
  void invoke("player_eq", { gains: value }).catch((e) => console.warn("equalizer", e));
}

export function useEqualizer() {
  if (!started) {
    started = true;
    push(gains.value);
    watch(
      gains,
      (value) => {
        try {
          localStorage.setItem(KEY, JSON.stringify(value));
        } catch {
          /* the gains are not kept; they still work */
        }
        push(value);
      },
      { deep: true },
    );
  }
  const preset = () => Object.entries(PRESETS).find(([, p]) => p.every((g, i) => g === gains.value[i]))?.[0] ?? "Custom";
  return {
    gains,
    preset,
    setBand: (index: number, gain: number) => {
      const next = [...gains.value];
      next[index] = Math.max(-MAX_GAIN, Math.min(MAX_GAIN, Math.round(gain)));
      gains.value = next;
    },
    apply: (name: string) => {
      if (PRESETS[name]) gains.value = [...PRESETS[name]!];
    },
  };
}
