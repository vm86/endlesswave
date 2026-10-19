<script setup lang="ts">
import { computed, ref } from "vue";
import type { ArcInfo } from "../api";

const props = defineProps<{ arc: ArcInfo }>();
const emit = defineEmits<{ change: [change: { enabled?: boolean; stages?: string[]; stage_tracks?: number }] }>();

// The line: a point for each stage, from the sad (bottom) to the lively (top).
const W = 300;
const H = 140;
const PAD_X = 48;
const PAD_Y = 16;
const levels = computed(() => props.arc.moods.length);
const stepY = computed(() => (H - 2 * PAD_Y) / (levels.value - 1));
const yOf = (mood: string) => PAD_Y + (levels.value - 1 - props.arc.moods.indexOf(mood)) * stepY.value;
const xOf = (i: number, n: number) => (n < 2 ? W / 2 : PAD_X + (i * (W - PAD_X - 14)) / (n - 1));

// While a point is dragged it shows where it will land; the change goes out when it is let go.
const dragging = ref<{ index: number; mood: string } | null>(null);
const shown = computed(() => props.arc.stages.map((m, i) => (dragging.value?.index === i ? dragging.value.mood : m)));
const points = computed(() => shown.value.map((m, i) => ({ x: xOf(i, shown.value.length), y: yOf(m), mood: m })));
const path = computed(() => points.value.map((p) => `${p.x},${p.y}`).join(" "));

const svg = ref<SVGSVGElement>();
function moodAt(event: PointerEvent): string {
  const box = svg.value!.getBoundingClientRect();
  const y = ((event.clientY - box.top) / box.height) * H;
  const level = Math.round((H - PAD_Y - y) / stepY.value);
  return props.arc.moods[Math.max(0, Math.min(levels.value - 1, level))]!;
}
// A point can also be moved with the arrow keys (up: livelier, down: sadder), as a slider.
function key(event: KeyboardEvent, index: number) {
  const step = event.key === "ArrowUp" || event.key === "ArrowRight" ? 1 : event.key === "ArrowDown" || event.key === "ArrowLeft" ? -1 : 0;
  if (!step) return;
  event.preventDefault();
  const at = props.arc.moods.indexOf(props.arc.stages[index]!) + step;
  const mood = props.arc.moods[Math.max(0, Math.min(levels.value - 1, at))]!;
  if (mood === props.arc.stages[index]) return;
  const stages = [...props.arc.stages];
  stages[index] = mood;
  emit("change", { stages, stage_tracks: props.arc.stage_tracks });
}
function down(event: PointerEvent, index: number) {
  (event.target as Element).setPointerCapture(event.pointerId);
  dragging.value = { index, mood: props.arc.stages[index]! };
}
function move(event: PointerEvent) {
  if (dragging.value) dragging.value = { ...dragging.value, mood: moodAt(event) };
}
function up() {
  const drag = dragging.value;
  dragging.value = null;
  if (drag && drag.mood !== props.arc.stages[drag.index]) {
    const stages = [...props.arc.stages];
    stages[drag.index] = drag.mood;
    emit("change", { stages, stage_tracks: props.arc.stage_tracks });
  }
}

const addStage = () =>
  emit("change", { stages: [...props.arc.stages, props.arc.stages[props.arc.stages.length - 1]!], stage_tracks: props.arc.stage_tracks });
const removeStage = () => emit("change", { stages: props.arc.stages.slice(0, -1), stage_tracks: props.arc.stage_tracks });
const setLength = (tracks: number) => emit("change", { stages: props.arc.stages, stage_tracks: tracks });
</script>

<template>
  <div class="arc" :class="{ off: !arc.enabled }">
    <div class="arc-head">
      <span class="chip" :class="{ sel: arc.enabled }" :title="arc.enabled ? 'Follow the mood arc' : 'The radio picks the energy by itself'"
        @click="emit('change', { enabled: !arc.enabled })">{{ arc.enabled ? "on" : "off" }}</span>
      <span class="dim">{{ arc.enabled ? `now: ${arc.mood}` : "the built-in arc" }}</span>
    </div>
    <svg ref="svg" :viewBox="`0 0 ${W} ${H}`" class="arc-svg" @pointermove="move" @pointerup="up" @pointercancel="up">
      <g v-for="m in arc.moods" :key="m">
        <line :x1="PAD_X - 8" :x2="W - 6" :y1="yOf(m)" :y2="yOf(m)" class="grid" />
        <text :x="PAD_X - 12" :y="yOf(m) + 3" class="label" text-anchor="end">{{ m }}</text>
      </g>
      <polyline :points="path" class="line" />
      <g v-for="(p, i) in points" :key="i" class="pt" tabindex="0" role="slider" :aria-label="`Stage ${i + 1}`" aria-orientation="vertical"
        :aria-valuemin="0" :aria-valuemax="levels - 1" :aria-valuenow="arc.moods.indexOf(p.mood)" :aria-valuetext="p.mood"
        @keydown="key($event, i)" @pointerdown.prevent="down($event, i)">
        <circle :cx="p.x" :cy="p.y" r="15" class="hit" />
        <circle :cx="p.x" :cy="p.y" r="7" class="dot" :class="{ now: arc.enabled && i === arc.stage, drag: dragging?.index === i }" />
        <text v-if="dragging?.index === i" :x="p.x" :y="p.y - 12" class="tip" text-anchor="middle">{{ p.mood }}</text>
        <title>stage {{ i + 1 }}: {{ p.mood }}</title>
      </g>
    </svg>
    <div class="arc-row">
      <span>Stages</span>
      <div class="stepper">
        <button :disabled="arc.stages.length <= arc.min_stages" title="Remove the last stage" @click="removeStage">−</button>
        <b>{{ arc.stages.length }}</b>
        <button :disabled="arc.stages.length >= arc.max_stages" title="Add a stage" @click="addStage">+</button>
      </div>
    </div>
    <div class="arc-row">
      <span>Stage length</span>
      <div class="stepper">
        <button :disabled="arc.stage_tracks <= 1" @click="setLength(arc.stage_tracks - 1)">−</button>
        <b>{{ arc.stage_tracks }} {{ arc.stage_tracks === 1 ? "track" : "tracks" }}</b>
        <button :disabled="arc.stage_tracks >= arc.max_stage_tracks" @click="setLength(arc.stage_tracks + 1)">+</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* The plot has a size of its own: in a wide window it does not grow with it. */
.arc { margin-top: 4px; max-width: 360px; }
.arc.off .line, .arc.off .dot { opacity: 0.45; }
.arc-head { display: flex; align-items: center; gap: 8px; margin-bottom: 4px; }
.arc-svg { width: 100%; height: auto; touch-action: none; user-select: none; display: block; }
.grid { stroke: var(--sep); stroke-width: 0.5; }
.label { font-size: 10px; fill: var(--text2); }
.tip { font-size: 10px; font-weight: 600; fill: var(--text); pointer-events: none; }
.hit { fill: transparent; cursor: ns-resize; }
.pt { outline: none; }
.pt:focus-visible .dot { stroke-width: 3; filter: drop-shadow(0 0 3px var(--accent)); }
.line { fill: none; stroke: var(--accent); stroke-width: 2; stroke-linejoin: round; stroke-linecap: round; }
.dot { fill: var(--panel, #1c1c1e); stroke: var(--accent); stroke-width: 2; cursor: ns-resize; transition: r 120ms, cx 150ms, cy 150ms; }
.dot:hover, .dot.drag { r: 8; }
.dot.now { fill: var(--accent); }
.arc-row { display: flex; align-items: center; justify-content: space-between; font-size: 12px; margin-top: 8px; }
.stepper { display: flex; align-items: center; gap: 8px; }
.stepper b { min-width: 64px; text-align: center; font-weight: 500; font-variant-numeric: tabular-nums; }
.stepper button { width: 28px; height: 28px; border-radius: 6px; font-size: 14px; background: var(--control); color: inherit; border: 0; cursor: pointer; }
.stepper button:hover:not(:disabled) { background: var(--control-hover); }
.stepper button:disabled { opacity: 0.35; cursor: default; }
</style>
