<script setup lang="ts">
import { computed, ref } from "vue";
import type { CycleInfo, Ramp } from "../api";

// A plan of the tempo: a point for each stage (its BPM), joined by a line, with a band around it that is as wide as the
// tolerance. Drag a point up (faster) or down (slower), or use the arrow keys.
const props = defineProps<{ ramp: Ramp; limits: CycleInfo; stage: number | null; on: boolean }>();
const emit = defineEmits<{ change: [ramp: Ramp] }>();

const STEP = 5;
const W = 300;
const H = 175;
const PAD_L = 34;
const PAD_R = 12;
const PAD_T = 12;
const PAD_B = 12;
const LOW = 30;
const HIGH = 180;
const plotH = H - PAD_T - PAD_B;
const yOf = (bpm: number) => PAD_T + ((HIGH - bpm) / (HIGH - LOW)) * plotH;
const xOf = (i: number, n: number) => (n < 2 ? W / 2 : PAD_L + (i * (W - PAD_L - PAD_R)) / (n - 1));
const GRID = [40, 80, 120, 160];

const dragging = ref<{ index: number; bpm: number } | null>(null);
const shown = computed(() => props.ramp.stages.map((b, i) => (dragging.value?.index === i ? dragging.value.bpm : b)));
const points = computed(() => shown.value.map((bpm, i) => ({ x: xOf(i, shown.value.length), y: yOf(bpm), bpm })));
const line = computed(() => points.value.map((p) => `${p.x},${p.y}`).join(" "));
const band = computed(() => {
  const t = props.ramp.tolerance;
  const upper = shown.value.map((b, i) => `${xOf(i, shown.value.length)},${yOf(Math.min(HIGH, b + t))}`);
  const lower = shown.value.map((b, i) => `${xOf(i, shown.value.length)},${yOf(Math.max(LOW, b - t))}`).reverse();
  return [...upper, ...lower].join(" ");
});

const svg = ref<SVGSVGElement>();
const clamp = (bpm: number) => Math.max(props.limits.min_bpm, Math.min(props.limits.max_bpm, Math.round(bpm / STEP) * STEP));
function bpmAt(event: PointerEvent): number {
  const box = svg.value!.getBoundingClientRect();
  const y = ((event.clientY - box.top) / box.height) * H;
  return clamp(HIGH - ((y - PAD_T) / plotH) * (HIGH - LOW));
}
const send = (stages: number[]) => emit("change", { ...props.ramp, stages });
function down(event: PointerEvent, index: number) {
  (event.target as Element).setPointerCapture(event.pointerId);
  dragging.value = { index, bpm: props.ramp.stages[index]! };
}
function move(event: PointerEvent) {
  if (dragging.value) dragging.value = { ...dragging.value, bpm: bpmAt(event) };
}
function up() {
  const drag = dragging.value;
  dragging.value = null;
  if (drag && drag.bpm !== props.ramp.stages[drag.index]) {
    const stages = [...props.ramp.stages];
    stages[drag.index] = drag.bpm;
    send(stages);
  }
}
function key(event: KeyboardEvent, index: number) {
  const step = event.shiftKey ? 3 * STEP : STEP;
  const by = event.key === "ArrowUp" || event.key === "ArrowRight" ? step : event.key === "ArrowDown" || event.key === "ArrowLeft" ? -step : 0;
  if (!by) return;
  event.preventDefault();
  const stages = [...props.ramp.stages];
  stages[index] = clamp(stages[index]! + by);
  send(stages);
}

const n = computed(() => props.ramp.stages.length);
const addStage = () => send([...props.ramp.stages, props.ramp.stages[n.value - 1]!]);
const removeStage = () => send(props.ramp.stages.slice(0, -1));
const setLength = (tracks: number) => emit("change", { ...props.ramp, stage_tracks: tracks });
const setTolerance = (tolerance: number) => emit("change", { ...props.ramp, tolerance });
</script>

<template>
  <div class="bpm" :class="{ off: !on }">
    <svg ref="svg" :viewBox="`0 0 ${W} ${H}`" class="bpm-svg" @pointermove="move" @pointerup="up" @pointercancel="up">
      <g v-for="g in GRID" :key="g">
        <line :x1="PAD_L - 6" :x2="W - PAD_R + 4" :y1="yOf(g)" :y2="yOf(g)" class="grid" />
        <text :x="PAD_L - 10" :y="yOf(g) + 3" class="label" text-anchor="end">{{ g }}</text>
      </g>
      <polygon :points="band" class="band" />
      <polyline :points="line" class="line" />
      <g v-for="(p, i) in points" :key="i" class="pt" tabindex="0" role="slider" :aria-label="`Stage ${i + 1}`" aria-orientation="vertical"
        :aria-valuemin="limits.min_bpm" :aria-valuemax="limits.max_bpm" :aria-valuenow="p.bpm" :aria-valuetext="`${p.bpm} BPM`"
        @keydown="key($event, i)" @pointerdown.prevent="down($event, i)">
        <circle :cx="p.x" :cy="p.y" r="15" class="hit" />
        <circle :cx="p.x" :cy="p.y" r="6.5" class="dot" :class="{ now: on && i === stage, drag: dragging?.index === i }" />
        <text v-if="dragging?.index === i" :x="p.x" :y="p.y - 11" class="tip" text-anchor="middle">{{ p.bpm }} BPM</text>
        <title>stage {{ i + 1 }}: {{ p.bpm }} BPM</title>
      </g>
    </svg>
    <div class="row">
      <span>Stages</span>
      <div class="stepper">
        <button type="button" :disabled="n <= limits.min_stages" aria-label="Remove the last stage" @click="removeStage">−</button>
        <b>{{ n }}</b>
        <button type="button" :disabled="n >= limits.max_stages" aria-label="Add a stage" @click="addStage">+</button>
      </div>
    </div>
    <div class="row">
      <span>Stage length</span>
      <div class="stepper">
        <button type="button" :disabled="ramp.stage_tracks <= 1" aria-label="Shorter" @click="setLength(ramp.stage_tracks - 1)">−</button>
        <b>{{ ramp.stage_tracks }} {{ ramp.stage_tracks === 1 ? "track" : "tracks" }}</b>
        <button type="button" :disabled="ramp.stage_tracks >= limits.max_stage_tracks" aria-label="Longer" @click="setLength(ramp.stage_tracks + 1)">+</button>
      </div>
    </div>
    <div class="row">
      <span>Range around the line</span>
      <div class="stepper">
        <button type="button" :disabled="ramp.tolerance <= limits.min_tolerance" aria-label="Narrower" @click="setTolerance(Math.max(limits.min_tolerance, ramp.tolerance - 1))">−</button>
        <b>± {{ ramp.tolerance }} BPM</b>
        <button type="button" :disabled="ramp.tolerance >= limits.max_tolerance" aria-label="Wider" @click="setTolerance(Math.min(limits.max_tolerance, ramp.tolerance + 1))">+</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.bpm { max-width: 360px; }
.bpm.off .line, .bpm.off .dot, .bpm.off .band { opacity: 0.45; }
.bpm-svg { width: 100%; height: auto; touch-action: none; user-select: none; display: block; }
.grid { stroke: var(--sep); stroke-width: 0.5; }
.label { font-size: 10px; fill: var(--text2); }
.tip { font-size: 10px; font-weight: 600; fill: var(--text); pointer-events: none; }
.band { fill: var(--accent); opacity: 0.14; }
.line { fill: none; stroke: var(--accent); stroke-width: 2; stroke-linejoin: round; stroke-linecap: round; }
.hit { fill: transparent; cursor: ns-resize; }
.pt { outline: none; }
.dot { fill: var(--panel, #1c1c1e); stroke: var(--accent); stroke-width: 2; cursor: ns-resize; transition: r 120ms; }
.dot.now { fill: var(--accent); }
.dot:hover, .dot.drag { r: 8; }
.pt:focus-visible .dot { stroke-width: 3; filter: drop-shadow(0 0 3px var(--accent)); }
.row { display: flex; align-items: center; justify-content: space-between; font-size: 12px; margin-top: 8px; }
.stepper { display: flex; align-items: center; gap: 8px; }
.stepper b { min-width: 84px; text-align: center; font-weight: 500; font-variant-numeric: tabular-nums; }
.stepper button { width: 28px; height: 28px; border-radius: 6px; font-size: 14px; background: var(--control); color: inherit; border: 0; cursor: pointer; }
.stepper button:hover:not(:disabled) { background: var(--control-hover); }
.stepper button:disabled { opacity: 0.35; cursor: default; }
</style>
