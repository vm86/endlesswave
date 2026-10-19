<script setup lang="ts">
import { computed } from "vue";
import type { CycleInfo, CycleName, Ramp } from "../api";
import BpmEditor from "./BpmEditor.vue";

const props = defineProps<{ cycle: CycleInfo }>();
const emit = defineEmits<{ change: [change: Record<string, unknown>] }>();

const NAMES: Array<[CycleName, string]> = [
  ["off", "Off"],
  ["morning", "Morning"],
  ["day", "Day"],
  ["evening", "Evening"],
];
const HINT: Record<CycleName, string> = {
  off: "The radio follows the mood arc (or its own).",
  morning: "Starts quiet and goes up to the lively tempo, stage by stage.",
  day: "The mood arc below, within a range of BPM.",
  evening: "Quiet music only, slowing down stage by stage.",
};

const active = computed(() => props.cycle.active);
const ramp = computed<Ramp | null>(() => (active.value === "morning" ? props.cycle.morning : active.value === "evening" ? props.cycle.evening : null));
const setRamp = (value: Ramp) => emit("change", { [active.value]: value });

const schedule = computed(() => props.cycle.schedule);
const TIMES: Array<["morning" | "day" | "evening", string]> = [
  ["morning", "Morning begins"],
  ["day", "Day begins"],
  ["evening", "Evening begins"],
];
function setTime(key: "morning" | "day" | "evening", value: string) {
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
    <div class="seg-row" role="radiogroup" aria-label="Cycle">
      <button v-for="[id, label] in NAMES" :key="id" type="button" role="radio" :aria-checked="active === id" :class="{ on: active === id }"
        @click="emit('change', { active: id })">{{ label }}</button>
    </div>
    <div class="dim hint">
      {{ HINT[active] }}
      <template v-if="active !== 'off' && cycle.target_bpm"> · now about {{ cycle.target_bpm }} BPM, {{ cycle.mood }}</template>
    </div>
    <BpmEditor v-if="ramp" :ramp="ramp" :limits="cycle" :stage="cycle.stage" :on="true" @change="setRamp" />
    <div v-else-if="active === 'day'" class="day">
      <div class="row">
        <span>Slowest</span>
        <div class="stepper">
          <button type="button" aria-label="Slower" @click="setDay(day.bpm_min - STEP, day.bpm_max)">−</button>
          <b>{{ day.bpm_min }} BPM</b>
          <button type="button" aria-label="Faster" @click="setDay(day.bpm_min + STEP, day.bpm_max)">+</button>
        </div>
      </div>
      <div class="row">
        <span>Fastest</span>
        <div class="stepper">
          <button type="button" aria-label="Slower" @click="setDay(day.bpm_min, day.bpm_max - STEP)">−</button>
          <b>{{ day.bpm_max }} BPM</b>
          <button type="button" aria-label="Faster" @click="setDay(day.bpm_min, day.bpm_max + STEP)">+</button>
        </div>
      </div>
    </div>

    <div class="schedule">
      <div class="row">
        <span>Schedule</span>
        <span class="chip" :class="{ sel: schedule.enabled }" :title="schedule.enabled ? 'The cycle follows the clock' : 'Switch by the time of day'"
          @click="emit('change', { schedule: { enabled: !schedule.enabled } })">{{ schedule.enabled ? "on" : "off" }}</span>
      </div>
      <div v-for="[key, label] in TIMES" :key="key" class="row">
        <label :for="`cycle-${key}`">{{ label }}</label>
        <input :id="`cycle-${key}`" type="time" :value="schedule[key]" required @change="setTime(key, ($event.target as HTMLInputElement).value)" />
      </div>
      <div v-if="schedule.enabled" class="dim hint">
        By the clock it is {{ cycle.scheduled }} now; the cycle changes by itself when the next one begins. A cycle chosen by hand
        stays until then.
      </div>
    </div>
  </div>
</template>

<style scoped>
.cycle { max-width: 360px; }
.seg-row { display: flex; padding: 2px; border-radius: 8px; background: var(--control); }
.seg-row button { flex: 1; border: 0; background: transparent; padding: 3px 0; border-radius: 6px; font-size: 12px; color: var(--text2); cursor: default;
  transition: background 150ms var(--ease), color 150ms var(--ease), box-shadow 150ms var(--ease); }
.seg-row button.on { background: var(--seg-on); color: var(--text); box-shadow: 0 0 0 0.5px rgba(0, 0, 0, 0.04), 0 1px 3px rgba(0, 0, 0, 0.14); }
.hint { margin: 6px 0 4px; }
.schedule { margin-top: 14px; padding-top: 6px; border-top: 0.5px solid var(--sep); }
.schedule input[type="time"] { font: inherit; font-size: 12px; color: inherit; background: var(--control); border: 0; border-radius: 6px; padding: 3px 6px; height: 26px; outline: none; }
.schedule input[type="time"]:focus-visible { outline: 2px solid color-mix(in srgb, var(--accent) 70%, transparent); outline-offset: 1px; }
.row { display: flex; align-items: center; justify-content: space-between; font-size: 12px; margin-top: 8px; }
.stepper { display: flex; align-items: center; gap: 8px; }
.stepper b { min-width: 84px; text-align: center; font-weight: 500; font-variant-numeric: tabular-nums; }
.stepper button { width: 28px; height: 28px; border-radius: 6px; font-size: 14px; background: var(--control); color: inherit; border: 0; cursor: pointer; }
.stepper button:hover { background: var(--control-hover); }
</style>
