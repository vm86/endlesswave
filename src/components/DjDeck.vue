<script setup lang="ts">
import { mdiFlagVariant, mdiPause, mdiPlay, mdiSkipPrevious } from "@mdi/js";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { clock, coverUrl, type TrackInfo } from "../api";

/** The places of a track that are good to mix at: where it begins, fills out, thins out. */
export interface Marker {
  at: number;
  kind: "start" | "intro" | "breakdown" | "drop" | "outro";
  bar: number;
}
export interface Overview {
  /** The low, the mid and the high band of each column, three numbers a column. */
  bins: number[];
  duration: number;
  bpm: number;
  beat_offset: number;
  markers: Marker[];
}

export interface DeckState {
  track: TrackInfo | null;
  overview_ready: boolean;
  low: number;
  mid: number;
  high: number;
  fader: number;
  state: "empty" | "loading" | "ready" | "error";
  message: string;
  playing: boolean;
  position: number;
  duration: number;
  cue: number;
  tempo: number;
  peak: number;
}

const props = defineProps<{
  id: "a" | "b";
  track: TrackInfo | null;
  deck: DeckState | null;
  /** The tempo of the other deck, to match to; null when there is nothing to match. */
  syncable: boolean;
  overview: Overview | null;
  eq: { low: number; mid: number; high: number };
  fader: number;
}>();
const emit = defineEmits<{
  set: [control: string, value: number];
  sync: [];
}>();

const ready = computed(() => props.deck?.state === "ready");
const percent = computed(() => ((props.deck?.tempo ?? 1) - 1) * 100);
const bpm = computed(() => (props.track?.bpm ? props.track.bpm * (props.deck?.tempo ?? 1) : null));
const progress = computed(() => (props.deck && props.deck.duration ? Math.min(1, props.deck.position / props.deck.duration) : 0));
const cuePlace = computed(() => (props.deck && props.deck.duration ? props.deck.cue / props.deck.duration : 0));

function seek(event: PointerEvent) {
  if (!ready.value || !props.deck) return;
  const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
  emit("set", `${props.id}.seek`, Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)) * props.deck.duration);
}
const MARKER_WORDS: Record<Marker["kind"], string> = {
  start: "The first beat of the bar grid: a clean place to start the track",
  intro: "The intro ends and the track fills out: bring it in here",
  drop: "The track fills out again after a breakdown: a strong place to bring it in",
  breakdown: "The track thins out: a good place to leave it, or to bring the other one in",
  outro: "The track thins out for good: the place to leave it",
};
/** The ways in and the ways out: the first begin a track, the others end it. */
const isExit = (kind: Marker["kind"]) => kind === "breakdown" || kind === "outro";
const markers = computed(() => props.overview?.markers ?? []);
/** The next way out, ahead of the playhead, and how long until it. */
const nextExit = computed(() => {
  const now = props.deck?.position ?? 0;
  const tempo = props.deck?.tempo || 1;
  const found = markers.value.find((m) => isExit(m.kind) && m.at > now + 0.5);
  return found ? { marker: found, in: (found.at - now) / tempo } : null;
});
const jump = (marker: Marker) => emit("set", `${props.id}.seek`, marker.at);

// The waveform: three bands, one over the other (the kick is the widest, the hats the narrowest).
const canvas = ref<HTMLCanvasElement>();
const COLORS = ["rgba(10, 132, 255, 0.95)", "rgba(255, 159, 10, 0.9)", "rgba(245, 245, 247, 0.95)"];
function draw() {
  const el = canvas.value;
  if (!el) return;
  const ratio = window.devicePixelRatio || 1;
  const [w, h] = [el.clientWidth, el.clientHeight];
  el.width = Math.round(w * ratio);
  el.height = Math.round(h * ratio);
  const ctx = el.getContext("2d");
  if (!ctx) return;
  ctx.scale(ratio, ratio);
  ctx.clearRect(0, 0, w, h);
  const overview = props.overview;
  if (!overview) return;
  const columns = overview.bins.length / 3;
  const mid = h / 2;
  const widths = [1, 0.72, 0.45];
  for (let x = 0; x < w; x++) {
    const column = Math.min(columns - 1, Math.floor((x / w) * columns));
    for (const band of [0, 1, 2]) {
      const height = Math.max(1, overview.bins[column * 3 + band]! * h * widths[band]!);
      ctx.fillStyle = COLORS[band]!;
      ctx.fillRect(x, mid - height / 2, 1, height);
    }
  }
  // The grid: a line at each phrase (8 bars), the places where tracks are laid one over the other.
  if (overview.bpm > 0 && overview.markers.length) {
    const phrase = (8 * 4 * 60) / overview.bpm;
    ctx.fillStyle = "rgba(255, 255, 255, 0.28)";
    for (let t = overview.beat_offset; t < overview.duration; t += phrase) ctx.fillRect(Math.round((t / overview.duration) * w), h - 7, 1, 7);
  }
}
let resize: ResizeObserver | undefined;
onMounted(() => {
  draw();
  if (canvas.value) {
    resize = new ResizeObserver(draw);
    resize.observe(canvas.value);
  }
});
onBeforeUnmount(() => resize?.disconnect());
watch(() => props.overview, draw);

const num = (event: Event) => Number((event.target as HTMLInputElement).value);
const bands = [
  ["high", "High"],
  ["mid", "Mid"],
  ["low", "Low"],
] as const;
</script>

<template>
  <section class="djdeck" :class="[id, { 'dj-empty': !track }]" :aria-label="`Deck ${id.toUpperCase()}`">
    <header>
      <span class="letter">{{ id.toUpperCase() }}</span>
      <img v-if="track?.cover" class="dj-art" :src="coverUrl(track.cover, 100)" alt="" />
      <div v-else class="dj-art"></div>
      <div class="dj-meta">
        <div class="title">{{ track?.title ?? "Empty" }}</div>
        <div class="dj-artist">{{ track ? track.artists.join(", ") : "Load a track from the list below" }}</div>
        <div class="chips" v-if="track">
          <span class="dj-chip bpm">{{ bpm ? bpm.toFixed(1) : "?" }} BPM</span>
          <span class="dj-chip">{{ track.camelot || "—" }}</span>
          <span v-if="track.mood" class="dj-chip">{{ track.mood }}</span>
        </div>
      </div>
    </header>

    <div class="time" :class="{ off: !ready }">
      <template v-if="deck?.state === 'loading'">Loading…</template>
      <template v-else-if="deck?.state === 'error'">{{ deck.message }}</template>
      <template v-else-if="ready && deck">
        <span class="dj-now">{{ clock(deck.position) }}</span>
        <span class="left">−{{ clock(Math.max(0, deck.duration - deck.position)) }}</span>
      </template>
      <template v-else>0:00</template>
    </div>
    <div class="bar" :class="{ off: !ready }" role="slider" aria-label="Position" :aria-valuenow="Math.round(progress * 100)" aria-valuemin="0" aria-valuemax="100" @pointerdown="seek">
      <canvas ref="canvas" class="wave"></canvas>
      <i class="played" :style="{ width: `${progress * 100}%` }"></i>
      <i class="cue" :style="{ left: `${cuePlace * 100}%` }"></i>
      <i class="head" :style="{ left: `${progress * 100}%` }"></i>
      <button v-for="m in markers" :key="m.kind + m.bar" type="button" class="flag" :class="[isExit(m.kind) ? 'out' : 'in', { next: nextExit?.marker === m }]"
        :style="{ left: `${(m.at / (overview?.duration || 1)) * 100}%` }" :title="`${m.kind} ${clock(m.at)}\n${MARKER_WORDS[m.kind]}`" :aria-label="`${m.kind} at ${clock(m.at)}`"
        @pointerdown.stop @click.stop="jump(m)"></button>
    </div>
    <div v-if="markers.length" class="marks">
      <button v-for="m in markers" :key="m.kind + m.bar" type="button" class="mark" :class="isExit(m.kind) ? 'out' : 'in'" :title="MARKER_WORDS[m.kind]" @click="jump(m)">
        {{ m.kind }} <b>{{ clock(m.at) }}</b>
      </button>
      <span v-if="deck?.playing && nextExit" class="until">{{ nextExit.marker.kind }} in <b>{{ clock(nextExit.in) }}</b></span>
    </div>

    <div class="dj-transport">
      <button type="button" class="pad" :disabled="!ready" title="Back to the cue point" @click="emit('set', `${id}.cue`, 1)">
        <v-icon :icon="mdiSkipPrevious" size="18" /><span>Cue</span>
      </button>
      <button type="button" class="pad play" :class="{ on: deck?.playing }" :disabled="!ready" :title="deck?.playing ? 'Pause' : 'Play'" @click="emit('set', `${id}.play`, deck?.playing ? 0 : 1)">
        <v-icon :icon="deck?.playing ? mdiPause : mdiPlay" size="22" />
      </button>
      <button type="button" class="pad" :disabled="!ready" title="Put the cue point here" @click="emit('set', `${id}.setcue`, 1)">
        <v-icon :icon="mdiFlagVariant" size="16" /><span>Set cue</span>
      </button>
    </div>

    <div class="controls">
      <div class="col dj-eq" v-for="[band, label] in bands" :key="band">
        <input type="range" class="vert" min="-40" max="9" step="0.5" :value="eq[band]" :aria-label="`${label} of deck ${id.toUpperCase()}`"
          @input="emit('set', `${id}.${band}`, num($event))" @dblclick="emit('set', `${id}.${band}`, 0)" />
        <label>{{ label }}</label>
        <small>{{ eq[band] <= -39 ? "kill" : `${eq[band] > 0 ? "+" : ""}${eq[band].toFixed(0)}` }}</small>
      </div>
      <div class="col">
        <input type="range" class="vert" min="0" max="1" step="0.01" :value="fader" :aria-label="`Level of deck ${id.toUpperCase()}`" @input="emit('set', `${id}.fader`, num($event))" />
        <label>Level</label>
        <div class="meter"><i :style="{ height: `${Math.min(100, (deck?.peak ?? 0) * 100)}%` }"></i></div>
      </div>
      <div class="col tempo">
        <input type="range" class="vert" min="-16" max="16" step="0.1" :value="percent" :aria-label="`Tempo of deck ${id.toUpperCase()}`"
          @input="emit('set', `${id}.tempo`, 1 + num($event) / 100)" @dblclick="emit('set', `${id}.tempo`, 1)" />
        <label>Tempo</label>
        <small>{{ percent > 0 ? "+" : "" }}{{ percent.toFixed(1) }}%</small>
        <button type="button" class="pad sync" :disabled="!syncable || !track?.bpm" title="Match the tempo of the other deck" @click="emit('sync')">Sync</button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.djdeck { display: flex; flex-direction: column; gap: 10px; padding: 14px; border-radius: 12px; background: var(--panel); box-shadow: 0 0 0 0.5px var(--panel-border); min-width: 0; }
header { display: flex; gap: 10px; align-items: center; min-width: 0; }
.letter { flex: none; width: 24px; height: 24px; border-radius: 6px; display: grid; place-items: center; font-weight: 700; color: #fff; background: var(--accent); }
.b .letter { background: #ff9f0a; }
.dj-art { flex: none; width: 56px; height: 56px; border-radius: 6px; background: var(--control); object-fit: cover; }
.dj-meta { min-width: 0; flex: 1; }
.title { font-weight: 600; font-size: 14px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.dj-artist { color: var(--text2); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.dj-empty .title { color: var(--text2); }
.chips { display: flex; gap: 5px; margin-top: 4px; }
.dj-chip { padding: 0 6px; border-radius: 8px; font-size: 11px; line-height: 16px; background: var(--control); color: var(--text2); }
.dj-chip.bpm { color: var(--text); font-variant-numeric: tabular-nums; font-weight: 600; }
.time { display: flex; justify-content: space-between; align-items: baseline; font-family: var(--mono); font-variant-numeric: tabular-nums; font-size: 22px; min-height: 28px; }
.time.off { font-size: 13px; color: var(--text2); font-family: var(--font); }
.time .left { font-size: 13px; color: var(--text2); }
.bar { position: relative; height: 64px; border-radius: 6px; background: rgba(0, 0, 0, 0.35); cursor: pointer; overflow: hidden; touch-action: none; }
.wave { position: absolute; inset: 0; width: 100%; height: 100%; }
.played { position: absolute; inset: 0 auto 0 0; background: rgba(0, 0, 0, 0.45); pointer-events: none; }
.head { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: #fff; box-shadow: 0 0 4px rgba(255, 255, 255, 0.8); pointer-events: none; }
.flag { position: absolute; top: 0; width: 13px; height: 13px; margin-left: -1px; padding: 0; border: 0; cursor: pointer; clip-path: polygon(0 0, 100% 0, 0 100%); }
.flag.in { background: #30d158; }
.flag.out { background: #ff9f0a; }
.flag.next { width: 17px; height: 17px; background: #ff453a; }
.marks { display: flex; flex-wrap: wrap; gap: 4px; align-items: center; margin-top: -4px; }
.mark { padding: 0 7px; height: 20px; border-radius: 10px; font-size: 11px; cursor: default; color: var(--text); background: var(--control); }
.mark b { font-variant-numeric: tabular-nums; font-weight: 600; }
.mark.in { box-shadow: inset 0 0 0 1px #30d158; }
.mark.out { box-shadow: inset 0 0 0 1px #ff9f0a; }
.mark:hover { background: var(--control-hover); }
.until { margin-left: auto; font-size: 11px; color: #ff9f0a; font-variant-numeric: tabular-nums; }
.bar.off { opacity: 0.5; cursor: default; }
.bar .cue { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--red); }
.dj-transport { display: flex; gap: 8px; justify-content: center; }
.pad { min-width: 64px; height: 38px; padding: 0 12px; border-radius: 8px; display: inline-flex; gap: 5px; align-items: center; justify-content: center; font-size: 12px; font-weight: 500;
  background: var(--control); color: var(--text); cursor: default; transition: background 120ms var(--ease); }
.pad:hover:not(:disabled) { background: var(--control-hover); }
.pad:active:not(:disabled) { transform: scale(0.97); }
.pad:disabled { opacity: 0.4; }
.pad.play { min-width: 84px; }
.pad.play.on { background: var(--accent); color: #fff; }
.b .pad.play.on { background: #ff9f0a; }
.controls { display: flex; gap: 6px; justify-content: space-between; align-items: stretch; margin-top: 2px; }
.col { flex: 1; display: flex; flex-direction: column; align-items: center; gap: 4px; min-width: 0; position: relative; }
.col label { font-size: 11px; color: var(--text2); }
.col small { font-size: 11px; color: var(--text2); font-variant-numeric: tabular-nums; min-height: 14px; }
.vert { writing-mode: vertical-lr; direction: rtl; width: 22px; height: 130px; accent-color: var(--accent); cursor: default; margin: 0; }
.b .vert { accent-color: #ff9f0a; }
.meter { position: absolute; right: 4px; top: 4px; bottom: 36px; width: 4px; border-radius: 2px; background: var(--control); display: flex; align-items: flex-end; overflow: hidden; }
.meter i { width: 100%; background: #30d158; transition: height 60ms linear; }
.sync { min-width: 0; width: 100%; height: 26px; margin-top: 2px; }
</style>
