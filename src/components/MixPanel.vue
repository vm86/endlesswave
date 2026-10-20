<script setup lang="ts">
import { computed, ref } from "vue";
import type { ArcInfo, CycleInfo, GenresInfo, LanguageInfo, MoodsInfo } from "../api";
import ArcEditor from "./ArcEditor.vue";
import CycleEditor from "./CycleEditor.vue";

type Mode = "free" | "arc" | "dayparts";
const MODES: Array<[Mode, string]> = [
  ["free", "Free"],
  ["arc", "Mood arc"],
  ["dayparts", "Parts of the day"],
];
const HINTS: Record<Mode, string> = {
  free: "The radio picks the energy by itself. Narrow it with the moods, language and genres below.",
  arc: "The set follows your arc of moods, stage by stage.",
  dayparts: "Morning, day and evening each play at a tempo of their own, by hand or by the clock.",
};

const props = defineProps<{ mode: Mode; moods: MoodsInfo | undefined; genres: GenresInfo | null; language: LanguageInfo | undefined; arc: ArcInfo | null; cycle: CycleInfo | null }>();
const emit = defineEmits<{ moods: [string[]]; genres: [string[]]; blocked: [string[]]; language: [string | null]; mode: [Mode]; cycle: [change: Record<string, unknown>]; arc: [{ enabled?: boolean; stages?: string[]; stage_tracks?: number }] }>();

// The morning and the evening set the mood themselves: what the listener picks would only fight them.
const locked = computed(() => props.cycle?.locks_mood === true);
const lockNote = computed(() =>
  props.cycle?.active === "evening"
    ? "Set by the Evening cycle: quiet music only."
    : "Set by the Morning cycle: the tempo plan chooses.",
);

const pick = ref<"pick" | "block">("pick");
const flip = (list: string[], item: string) => (list.includes(item) ? list.filter((x) => x !== item) : [...list, item]);

function clickGenre(g: string) {
  const info = props.genres;
  if (!info) return;
  if (pick.value === "block") emit("blocked", flip(info.blocked, g));
  else emit("genres", flip(info.selected, g));
}
</script>

<template>
  <div class="mix">
    <!-- What plays the set: one choice, and only its settings below. -->
    <div class="seg-row" role="radiogroup" aria-label="What plays the set">
      <button v-for="[id, name] in MODES" :key="id" type="button" role="radio" :aria-checked="mode === id" :class="{ on: mode === id }"
        @click="emit('mode', id)">{{ name }}</button>
    </div>
    <div class="dim hint">{{ HINTS[mode] }}</div>

    <ArcEditor v-if="mode === 'arc' && arc" always :arc="arc" @change="emit('arc', $event)" />
    <CycleEditor v-else-if="mode === 'dayparts' && cycle" :cycle="cycle" @change="emit('cycle', $event)">
      <template #day>
        <ArcEditor v-if="arc" always :arc="arc" @change="emit('arc', $event)" />
      </template>
    </CycleEditor>

    <h4>Mood</h4>
    <div :class="{ locked }" :inert="locked">
      <span v-for="m in moods?.available" :key="m" class="chip" :class="{ sel: moods?.selected.includes(m) }"
        @click="emit('moods', flip(moods?.selected ?? [], m))">{{ m }}</span>
    </div>
    <div v-if="locked" class="dim">{{ lockNote }}</div>
    <div v-else-if="moods && moods.steered === false" class="dim">the radio judges moods by sound</div>

    <template v-if="language?.steered">
      <h4>Language</h4>
      <span v-for="l in language.available" :key="l" class="chip" :class="{ sel: language.selected === l }"
        @click="emit('language', language.selected === l ? null : l)">{{ l }}</span>
    </template>

    <h4>Genres
      <span class="mode" :class="{ on: pick === 'pick' }" @click="pick = 'pick'">pick</span>
      <span class="mode" :class="{ on: pick === 'block' }" @click="pick = 'block'">block</span>
    </h4>
    <span v-if="!genres?.available.length" class="dim">appear as tracks are played; none picked = any</span>
    <span v-for="g in genres?.available" :key="g" class="chip"
      :class="{ sel: genres?.selected.includes(g), blocked: genres?.blocked.includes(g) }" @click="clickGenre(g)">{{ g }}</span>
    <div v-if="genres && (genres.selected.join() !== genres.applied.join() || genres.blocked.join() !== genres.blocked_applied.join())" class="dim">
      changes apply on refresh (⟳)
    </div>
  </div>
</template>

<style scoped>
.seg-row { display: flex; padding: 2px; border-radius: 8px; background: var(--control); }
.seg-row button { flex: 1; border: 0; background: transparent; padding: 4px 0; border-radius: 6px; font-size: 12px; color: var(--text2); cursor: default;
  transition: background 150ms var(--ease), color 150ms var(--ease), box-shadow 150ms var(--ease); }
.seg-row button.on { background: var(--seg-on); color: var(--text); box-shadow: 0 0 0 0.5px rgba(0, 0, 0, 0.04), 0 1px 3px rgba(0, 0, 0, 0.14); }
.hint { margin: 6px 0 10px; }
</style>
