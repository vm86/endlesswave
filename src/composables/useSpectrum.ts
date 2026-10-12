import { onScopeDispose, ref, watch, type Ref } from "vue";
import { baseUrl } from "../api";

export const BANDS = 12;
const EMPTY: number[] = Array.from({ length: BANDS }, () => 0);
// What is kept of what the radio sends ahead: ~30 values a second.
const MAX_FRAMES = 600;
// No value for this long while playing: the spectrum is not coming.
const SILENT_MS = 4000;

/** One value of the spectrum: its position in the mix, and its bands (0..1). */
interface Frame {
  t: number;
  b: number[];
}

/** The bands of the last frame that is not after `target`; older frames are dropped. */
export function pickFrame(frames: Frame[], target: number): number[] | null {
  let last = -1;
  while (last + 1 < frames.length && (frames[last + 1] as Frame).t <= target) last++;
  if (last < 0) return null;
  frames.splice(0, last);
  return (frames[0] as Frame).b;
}

/**
 * The spectrum of the radio's mix, in time with what plays. The radio counts
 * it from the mix and says where in the mix the stream of this listener
 * (named by `token`) starts; the player is a few seconds behind the mix, so
 * the frames wait until `audio.currentTime` gets to them.
 */
export function useSpectrum(player: { playing: Ref<boolean>; token: Ref<string>; currentTime: () => number }) {
  const bands = ref<number[]>(EMPTY);
  const note = ref("");
  let source: EventSource | null = null;
  let frames: Frame[] = [];
  let lastT = -Infinity;
  let start: number | null = null;
  let raf = 0;
  let lastFrameAt = 0;

  function close() {
    source?.close();
    source = null;
    cancelAnimationFrame(raf);
    frames = [];
    lastT = -Infinity;
    start = null;
    bands.value = EMPTY;
    note.value = "";
  }

  function show() {
    if (start !== null) {
      const picked = pickFrame(frames, start + player.currentTime());
      if (picked && picked !== bands.value) bands.value = picked;
    }
    note.value = performance.now() - lastFrameAt > SILENT_MS ? "no spectrum from the radio" : "";
    raf = requestAnimationFrame(show);
  }

  function open(token: string) {
    lastFrameAt = performance.now();
    source = new EventSource(`${baseUrl()}/api/spectrum?l=${encodeURIComponent(token)}`);
    source.addEventListener("start", (event) => {
      start = (JSON.parse((event as MessageEvent<string>).data) as { position: number | null }).position;
    });
    source.onmessage = (event: MessageEvent<string>) => {
      const frame = JSON.parse(event.data) as Frame;
      lastFrameAt = performance.now();
      // A connection made again begins with what was already seen.
      if (frame.t <= lastT) return;
      lastT = frame.t;
      frames.push(frame);
      if (frames.length > MAX_FRAMES) frames.splice(0, frames.length - MAX_FRAMES);
    };
    raf = requestAnimationFrame(show);
  }

  watch(
    () => [player.playing.value, player.token.value] as const,
    ([playing, token]) => {
      close();
      if (playing && token) open(token);
    },
    { immediate: true },
  );
  onScopeDispose(close);

  return { bands, note };
}
