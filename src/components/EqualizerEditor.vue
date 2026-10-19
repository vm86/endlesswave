<script setup lang="ts">
import { computed, ref } from "vue";
import { BANDS, MAX_GAIN, PRESETS, useEqualizer } from "../composables/useEqualizer";

const { gains, preset, setBand, apply } = useEqualizer();

// The curve: a point for each band, its height is the gain; drag it, or use the arrow keys, double-click for 0 dB.
const W = 320;
const H = 150;
const PAD_L = 30;
const PAD_R = 10;
const PAD_T = 12;
const PAD_B = 22;
const plotH = H - PAD_T - PAD_B;
const xOf = (i: number) => PAD_L + (i * (W - PAD_L - PAD_R)) / (BANDS.length - 1);
const yOf = (gain: number) => PAD_T + ((MAX_GAIN - gain) / (2 * MAX_GAIN)) * plotH;
const GRID = [MAX_GAIN, MAX_GAIN / 2, 0, -MAX_GAIN / 2, -MAX_GAIN];

const dragging = ref<{ index: number; gain: number } | null>(null);
const shown = computed(() => gains.value.map((g, i) => (dragging.value?.index === i ? dragging.value.gain : g)));
const points = computed(() => shown.value.map((g, i) => ({ x: xOf(i), y: yOf(g), gain: g })));
const path = computed(() => points.value.map((p) => `${p.x},${p.y}`).join(" "));

const svg = ref<SVGSVGElement>();
function gainAt(event: PointerEvent): number {
  const box = svg.value!.getBoundingClientRect();
  const y = ((event.clientY - box.top) / box.height) * H;
  return Math.max(-MAX_GAIN, Math.min(MAX_GAIN, Math.round(MAX_GAIN - ((y - PAD_T) / plotH) * 2 * MAX_GAIN)));
}
function down(event: PointerEvent, index: number) {
  (event.target as Element).setPointerCapture(event.pointerId);
  dragging.value = { index, gain: gains.value[index]! };
}
function move(event: PointerEvent) {
  if (dragging.value) dragging.value = { ...dragging.value, gain: gainAt(event) };
}
function up() {
  const drag = dragging.value;
  dragging.value = null;
  if (drag) setBand(drag.index, drag.gain);
}
function key(event: KeyboardEvent, index: number) {
  const step = event.shiftKey ? 3 : 1;
  const by = event.key === "ArrowUp" || event.key === "ArrowRight" ? step : event.key === "ArrowDown" || event.key === "ArrowLeft" ? -step : 0;
  if (event.key === "Home" || event.key === "0") {
    event.preventDefault();
    return setBand(index, 0);
  }
  if (!by) return;
  event.preventDefault();
  setBand(index, gains.value[index]! + by);
}
const label = (gain: number) => `${gain > 0 ? "+" : ""}${gain} dB`;
</script>

<template>
  <div class="eq-editor">
    <div class="eq-presets">
      <span v-for="name in Object.keys(PRESETS)" :key="name" class="chip" :class="{ sel: preset() === name }" @click="apply(name)">{{ name }}</span>
      <span v-if="preset() === 'Custom'" class="chip sel">Custom</span>
    </div>
    <svg ref="svg" :viewBox="`0 0 ${W} ${H}`" class="eq-svg" @pointermove="move" @pointerup="up" @pointercancel="up">
      <g v-for="g in GRID" :key="g">
        <line :x1="PAD_L - 4" :x2="W - PAD_R + 4" :y1="yOf(g)" :y2="yOf(g)" :class="g === 0 ? 'zero' : 'grid'" />
        <text :x="PAD_L - 8" :y="yOf(g) + 3" class="label" text-anchor="end">{{ g > 0 ? "+" : "" }}{{ g }}</text>
      </g>
      <text v-for="(name, i) in BANDS" :key="name" :x="xOf(i)" :y="H - 6" class="label" text-anchor="middle">{{ name }}</text>
      <polyline :points="path" class="line" />
      <g v-for="(p, i) in points" :key="i" class="pt" tabindex="0" role="slider" :aria-label="`${BANDS[i]} Hz`" aria-orientation="vertical"
        :aria-valuemin="-MAX_GAIN" :aria-valuemax="MAX_GAIN" :aria-valuenow="p.gain" :aria-valuetext="label(p.gain)" @keydown="key($event, i)"
        @pointerdown.prevent="down($event, i)" @dblclick="setBand(i, 0)">
        <circle :cx="p.x" :cy="p.y" r="15" class="hit" />
        <circle :cx="p.x" :cy="p.y" r="6" class="dot" :class="{ drag: dragging?.index === i, moved: p.gain !== 0 }" />
        <text v-if="dragging?.index === i" :x="p.x" :y="p.y - 11" class="tip" text-anchor="middle">{{ label(p.gain) }}</text>
        <title>{{ BANDS[i] }} Hz: {{ label(p.gain) }}</title>
      </g>
    </svg>
  </div>
</template>

<style scoped>
.eq-editor { max-width: 520px; margin: 0 auto; }
.eq-presets { display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 6px; }
.eq-svg { width: 100%; height: auto; touch-action: none; user-select: none; display: block; }
.grid { stroke: var(--sep); stroke-width: 0.5; }
.zero { stroke: var(--text2); stroke-width: 0.5; opacity: 0.6; }
.label { font-size: 10px; fill: var(--text2); }
.tip { font-size: 10px; font-weight: 600; fill: var(--text); pointer-events: none; }
.line { fill: none; stroke: var(--accent); stroke-width: 2; stroke-linejoin: round; stroke-linecap: round; }
.hit { fill: transparent; cursor: ns-resize; }
.pt { outline: none; }
.dot { fill: var(--panel, #1c1c1e); stroke: var(--accent); stroke-width: 2; cursor: ns-resize; transition: r 120ms; }
.dot.moved { fill: var(--accent); }
.dot:hover, .dot.drag { r: 7.5; }
.pt:focus-visible .dot { stroke-width: 3; filter: drop-shadow(0 0 3px var(--accent)); }
</style>
