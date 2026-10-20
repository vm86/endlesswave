<script setup lang="ts">
import { mdiChartBellCurve, mdiClockOutline, mdiShuffleVariant, mdiWeatherSunsetDown, mdiWeatherSunsetUp, mdiWhiteBalanceSunny } from "@mdi/js";
import { computed } from "vue";
import type { ArcInfo, CycleInfo } from "../api";

type Mode = "free" | "arc" | "dayparts";
const props = defineProps<{ mode: Mode; cycle: CycleInfo | null; arc: ArcInfo | null }>();
defineEmits<{ open: [] }>();

// What the display says: what plays the set, and where the plan is aiming now (the tempo and the mood), with the stage.
const shown = computed(() => {
  const { cycle, arc } = props;
  if (props.mode === "dayparts" && cycle) {
    const part = cycle.active === "off" ? cycle.scheduled : cycle.active;
    const icon = { morning: mdiWeatherSunsetUp, day: mdiWhiteBalanceSunny, evening: mdiWeatherSunsetDown, off: mdiWhiteBalanceSunny }[part] ?? mdiWhiteBalanceSunny;
    const stages = part === "morning" ? cycle.morning.stages.length : part === "evening" ? cycle.evening.stages.length : 0;
    return {
      icon,
      name: part[0]!.toUpperCase() + part.slice(1),
      clock: cycle.schedule.enabled,
      bpm: cycle.target_bpm,
      mood: cycle.mood,
      stage: stages ? Math.min(cycle.stage ?? 0, stages - 1) : null,
      stages,
    };
  }
  if (props.mode === "arc" && arc) {
    return { icon: mdiChartBellCurve, name: "Mood arc", clock: false, bpm: null, mood: arc.mood, stage: arc.stage, stages: arc.stages.length };
  }
  return { icon: mdiShuffleVariant, name: "Free", clock: false, bpm: null, mood: "", stage: null, stages: 0 };
});

const text = computed(() => {
  const s = shown.value;
  return [s.name, s.clock ? "by the clock" : "", s.bpm ? `aiming at ${s.bpm} BPM` : "", s.mood ? s.mood : ""].filter(Boolean).join(", ");
});
</script>

<template>
  <button type="button" class="plan" :class="{ free: mode === 'free' }" :title="`${text}\nClick to change`" :aria-label="`Plan: ${text}`" @click="$emit('open')">
    <v-icon :icon="shown.icon" size="13" />
    <span class="name">{{ shown.name }}</span>
    <v-icon v-if="shown.clock" :icon="mdiClockOutline" size="11" class="clk" />
    <template v-if="shown.bpm || shown.mood">
      <span class="sep">·</span>
      <span v-if="shown.bpm" class="bpm">{{ shown.bpm }} BPM</span>
      <span v-if="shown.mood" class="mood">{{ shown.mood }}</span>
    </template>
    <span v-if="shown.stages > 1 && shown.stage !== null" class="steps" aria-hidden="true">
      <i v-for="n in shown.stages" :key="n" :class="{ on: n - 1 === shown.stage, done: n - 1 < shown.stage }" />
    </span>
  </button>
</template>

<style scoped>
.plan { display: inline-flex; align-items: center; gap: 5px; max-width: 100%; margin-top: 3px; padding: 1px 7px 1px 5px; border-radius: 9px; cursor: default;
  font-size: 11px; line-height: 16px; white-space: nowrap; overflow: hidden; color: var(--accent); background: color-mix(in srgb, var(--accent) 14%, transparent);
  transition: background 150ms var(--ease), color 150ms var(--ease); }
.plan:hover { background: color-mix(in srgb, var(--accent) 24%, transparent); }
.plan.free { color: var(--text2); background: var(--control); }
.name { font-weight: 600; }
.clk { opacity: 0.8; }
.sep { opacity: 0.6; }
.bpm { font-variant-numeric: tabular-nums; }
.mood { font-weight: 500; }
/* The stages of the plan: the ones done are filled, the one that is on is wider. */
.steps { display: inline-flex; align-items: center; gap: 2px; margin-left: 2px; }
.steps i { width: 4px; height: 4px; border-radius: 2px; background: currentColor; opacity: 0.25; transition: width 200ms var(--ease), opacity 200ms var(--ease); }
.steps i.done { opacity: 0.6; }
.steps i.on { width: 9px; opacity: 1; }
</style>
