<script setup lang="ts">
import { computed, ref } from "vue";
import type { ArcInfo, CycleInfo, GenresInfo, LanguageInfo, MoodsInfo } from "../api";
import ArcEditor from "./ArcEditor.vue";
import CycleEditor from "./CycleEditor.vue";

const props = defineProps<{ moods: MoodsInfo | undefined; genres: GenresInfo | null; language: LanguageInfo | undefined; arc: ArcInfo | null; cycle: CycleInfo | null }>();
const emit = defineEmits<{ moods: [string[]]; genres: [string[]]; blocked: [string[]]; language: [string | null]; cycle: [change: Record<string, unknown>]; arc: [{ enabled?: boolean; stages?: string[]; stage_tracks?: number }] }>();

// The morning and the evening set the mood themselves: what the listener picks would only fight them.
const locked = computed(() => props.cycle?.locks_mood === true);
const lockNote = computed(() =>
  props.cycle?.active === "evening"
    ? "Set by the Evening cycle: quiet music only."
    : "Set by the Morning cycle: the tempo plan chooses.",
);

const mode = ref<"pick" | "block">("pick");
const flip = (list: string[], item: string) => (list.includes(item) ? list.filter((x) => x !== item) : [...list, item]);

function clickGenre(g: string) {
  const info = props.genres;
  if (!info) return;
  if (mode.value === "block") emit("blocked", flip(info.blocked, g));
  else emit("genres", flip(info.selected, g));
}
</script>

<template>
  <div class="mix">
    <h4>Cycle</h4>
    <CycleEditor v-if="cycle" :cycle="cycle" @change="emit('cycle', $event)" />

    <h4>Mood</h4>
    <div :class="{ locked }" :inert="locked">
      <span v-for="m in moods?.available" :key="m" class="chip" :class="{ sel: moods?.selected.includes(m) }"
        @click="emit('moods', flip(moods?.selected ?? [], m))">{{ m }}</span>
    </div>
    <div v-if="locked" class="dim">{{ lockNote }}</div>
    <div v-else-if="moods && moods.steered === false" class="dim">the radio judges moods by sound</div>

    <h4>Mood arc</h4>
    <ArcEditor v-if="arc" :arc="arc" :locked="locked ? lockNote : ''" @change="emit('arc', $event)" />

    <template v-if="language?.steered">
      <h4>Language</h4>
      <span v-for="l in language.available" :key="l" class="chip" :class="{ sel: language.selected === l }"
        @click="emit('language', language.selected === l ? null : l)">{{ l }}</span>
    </template>

    <h4>Genres
      <span class="mode" :class="{ on: mode === 'pick' }" @click="mode = 'pick'">pick</span>
      <span class="mode" :class="{ on: mode === 'block' }" @click="mode = 'block'">block</span>
    </h4>
    <span v-if="!genres?.available.length" class="dim">appear as tracks are played; none picked = any</span>
    <span v-for="g in genres?.available" :key="g" class="chip"
      :class="{ sel: genres?.selected.includes(g), blocked: genres?.blocked.includes(g) }" @click="clickGenre(g)">{{ g }}</span>
    <div v-if="genres && (genres.selected.join() !== genres.applied.join() || genres.blocked.join() !== genres.blocked_applied.join())" class="dim">
      changes apply on refresh (⟳)
    </div>
  </div>
</template>
