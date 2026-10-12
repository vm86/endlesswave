<script setup lang="ts">
import { ref } from "vue";
import type { GenresInfo, LanguageInfo, MoodsInfo } from "../api";

const props = defineProps<{ moods: MoodsInfo | undefined; genres: GenresInfo | null; language: LanguageInfo | undefined }>();
const emit = defineEmits<{ moods: [string[]]; genres: [string[]]; blocked: [string[]]; language: [string | null] }>();

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
    <h4>Mood</h4>
    <span v-for="m in moods?.available" :key="m" class="chip" :class="{ sel: moods?.selected.includes(m) }"
      @click="emit('moods', flip(moods?.selected ?? [], m))">{{ m }}</span>
    <div v-if="moods && moods.steered === false" class="dim">the radio judges moods by sound</div>

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
