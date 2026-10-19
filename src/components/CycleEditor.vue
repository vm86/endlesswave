<script setup lang="ts">
import { computed, ref } from "vue";
import type { CycleInfo, CycleName, Ramp } from "../api";
import BpmEditor from "./BpmEditor.vue";
import Disclosure from "./Disclosure.vue";

const props = defineProps<{ cycle: CycleInfo }>();
const emit = defineEmits<{ change: [change: Record<string, unknown>] }>();

type Part = "morning" | "day" | "evening";
const PARTS: Array<[Part, string, string]> = [
  ["morning", "Morning", "quiet at first, then livelier"],
  ["day", "Day", "the mood arc, in a range of tempo"],
  ["evening", "Evening", "quiet music, slowing down"],
];
const NAMES: Array<[CycleName, string]> = [["off", "Off"], ...PARTS.map(([id, label]) => [id, label] as [CycleName, string])];

const active = computed(() => props.cycle.active);
const schedule = computed(() => props.cycle.schedule);
const byClock = computed(() => schedule.value.enabled);
const label = (id: CycleName) => NAMES.find(([n]) => n === id)?.[1] ?? id;

// What the clock says is next, for the line under the times.
const NEXT: Record<Part, Part> = { morning: "day", day: "evening", evening: "morning" };
const until = computed(() => schedule.value[NEXT[props.cycle.scheduled as Part]]);

// Which plan is being edited: a choice of the editor, apart from the cycle that is on.
const editing = ref<Part>(active.value === "off" ? "morning" : (active.value as Part));
const ramp = computed<Ramp | null>(() => (editing.value === "day" ? null : props.cycle[editing.value]));
const setRamp = (value: Ramp) => emit("change", { [editing.value]: value });
const showsNow = computed(() => editing.value === active.value);

const summary = (id: Part) => {
  if (id === "day") return `${props.cycle.day.bpm_min}–${props.cycle.day.bpm_max} BPM`;
  const r = props.cycle[id];
  return `${r.stages[0]} → ${r.stages[r.stages.length - 1]} BPM`;
};

function setTime(key: Part, value: string) {
  if (/^\d\d:\d\d$/.test(value)) emit("change", { schedule: { [key]: value } });
}

const day = computed(() => props.cycle.day);
const STEP = 5;
const MIN_SPAN = 10;
function setDay(low: number, high: number) {
  const lo = Math.max(props.cycle.min_bpm, Math.min(low, high - MIN_SPAN));
  const hi = Math.min(props.cycle.max_bpm, Math.max(high, lo + MIN_SPAN));
  emit("change", { day: { bpm_min: lo, bpm_max: hi } });
}
</script>

<template>
  <div class="cycle">
    <!-- 1. Who decides: you, or the clock. -->
    <div class="c-row first">
      <span>Follow the clock</span>
      <span class="chip" :class="{ sel: byClock }" role="switch" :aria-checked="byClock" tabindex="0"
        :title="byClock ? 'The cycle changes by itself, by the time of day' : 'Choose the cycle yourself'"
        @click="emit('change', { schedule: { enabled: !byClock } })" @keydown.enter.space.prevent="emit('change', { schedule: { enabled: !byClock } })">
        {{ byClock ? "on" : "off" }}
      </span>
    </div>

    <!-- 2a. By hand: one choice. -->
    <template v-if="!byClock">
      <div class="seg-row" role="radiogroup" aria-label="Cycle">
        <button v-for="[id, name] in NAMES" :key="id" type="button" role="radio" :aria-checked="active === id" :class="{ on: active === id }"
          @click="emit('change', { active: id })">{{ name }}</button>
      </div>
      <div class="dim hint">
        <template v-if="active === 'off'">No cycle: the radio follows the mood arc (or its own).</template>
        <template v-else>
          {{ (PARTS.find(([id]) => id === active)?.[2] ?? '').replace(/^./, (c) => c.toUpperCase()) }} · now about {{ cycle.target_bpm }} BPM
        </template>
      </div>
    </template>

    <!-- 2b. By the clock: when each begins; the one that is on is marked, and nothing here can change it. -->
    <template v-else>
      <div v-for="[id, name] in PARTS" :key="id" class="part" :class="{ current: cycle.scheduled === id }">
        <span class="dot" aria-hidden="true" />
        <label :for="`cycle-${id}`">{{ name }} from</label>
        <input :id="`cycle-${id}`" type="time" :value="schedule[id]" required @change="setTime(id, ($event.target as HTMLInputElement).value)" />
      </div>
      <div class="dim hint">
        Now: {{ label(cycle.scheduled) }}, until {{ until }} · about {{ cycle.target_bpm }} BPM. The clock chooses; the cycle cannot be
        picked by hand.
      </div>
    </template>

    <!-- 3. What each cycle plays: pick one to look at, apart from what is on. -->
    <Disclosure id="cycle-plan" title="Plans" :summary="`${label(editing)}: ${summary(editing)}`">
      <div class="seg-row small" role="radiogroup" aria-label="Plan to edit">
        <button v-for="[id, name] in PARTS" :key="id" type="button" role="radio" :aria-checked="editing === id" :class="{ on: editing === id }"
          @click="editing = id">{{ name }}</button>
      </div>
      <BpmEditor v-if="ramp" :ramp="ramp" :limits="cycle" :stage="cycle.stage" :on="showsNow" @change="setRamp" />
      <div v-else class="day">
        <div class="dim hint">The mood arc (below) plays inside this range of tempo.</div>
        <div class="c-row">
          <span>Slowest</span>
          <div class="stepper">
            <button type="button" aria-label="Slower" @click="setDay(day.bpm_min - STEP, day.bpm_max)">−</button>
            <b>{{ day.bpm_min }} BPM</b>
            <button type="button" aria-label="Faster" @click="setDay(day.bpm_min + STEP, day.bpm_max)">+</button>
          </div>
        </div>
        <div class="c-row">
          <span>Fastest</span>
          <div class="stepper">
            <button type="button" aria-label="Slower" @click="setDay(day.bpm_min, day.bpm_max - STEP)">−</button>
            <b>{{ day.bpm_max }} BPM</b>
            <button type="button" aria-label="Faster" @click="setDay(day.bpm_min, day.bpm_max + STEP)">+</button>
          </div>
        </div>
      </div>
    </Disclosure>
  </div>
</template>

<style scoped>
.cycle { max-width: 360px; }
.c-row { display: flex; align-items: center; justify-content: space-between; font-size: 12px; margin-top: 8px; }
.c-row.first { margin: 0 0 8px; }
.seg-row { display: flex; padding: 2px; border-radius: 8px; background: var(--control); }
.seg-row.small { margin-bottom: 6px; }
.seg-row button { flex: 1; border: 0; background: transparent; padding: 3px 0; border-radius: 6px; font-size: 12px; color: var(--text2); cursor: default;
  transition: background 150ms var(--ease), color 150ms var(--ease), box-shadow 150ms var(--ease); }
.seg-row button.on { background: var(--seg-on); color: var(--text); box-shadow: 0 0 0 0.5px rgba(0, 0, 0, 0.04), 0 1px 3px rgba(0, 0, 0, 0.14); }
.hint { margin: 6px 0 4px; }
/* The times of the clock: the one that is on has a filled dot. */
.part { display: flex; align-items: center; gap: 8px; font-size: 12px; padding: 4px 0; color: var(--text2); }
.part label { flex: 1; }
.part.current { color: var(--text); font-weight: 500; }
.part .dot { width: 8px; height: 8px; border-radius: 50%; border: 1.5px solid var(--text2); box-sizing: border-box; transition: background 150ms var(--ease), border-color 150ms var(--ease); }
.part.current .dot { background: var(--accent); border-color: var(--accent); }
.part input[type="time"] { font: inherit; font-size: 12px; color: inherit; background: var(--control); border: 0; border-radius: 6px; padding: 3px 6px; height: 26px; outline: none; }
.part input[type="time"]:focus-visible { outline: 2px solid color-mix(in srgb, var(--accent) 70%, transparent); outline-offset: 1px; }
.stepper { display: flex; align-items: center; gap: 8px; }
.stepper b { min-width: 84px; text-align: center; font-weight: 500; font-variant-numeric: tabular-nums; }
.stepper button { width: 28px; height: 28px; border-radius: 6px; font-size: 14px; background: var(--control); color: inherit; border: 0; cursor: pointer; }
.stepper button:hover { background: var(--control-hover); }
</style>
