<script setup lang="ts">
import { mdiChevronRight } from "@mdi/js";
import { ref, watch } from "vue";

/** A row that opens: the name on the left, what is set (in short) on the right, a chevron. Like a disclosure group of a Mac app. */
const props = defineProps<{ title: string; summary?: string; id: string }>();
const KEY = `open:${props.id}`;
const read = () => {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
};
const open = ref(read());
watch(open, (value) => {
  try {
    localStorage.setItem(KEY, value ? "1" : "0");
  } catch {
    /* it is only a convenience */
  }
});
</script>

<template>
  <div class="disclosure" :class="{ open }">
    <button type="button" class="head" :aria-expanded="open" :aria-controls="`d-${id}`" @click="open = !open">
      <v-icon :icon="mdiChevronRight" size="14" class="chev" />
      <span class="name">{{ title }}</span>
      <span v-if="summary" class="sum">{{ summary }}</span>
    </button>
    <div :id="`d-${id}`" class="body" :inert="!open">
      <div class="inner"><slot /></div>
    </div>
  </div>
</template>

<style scoped>
.disclosure { border-top: 0.5px solid var(--sep); }
.head { display: flex; align-items: center; gap: 6px; width: 100%; padding: 7px 0; text-align: left; cursor: default; font-size: 12px; }
.chev { flex: none; color: var(--text2); transition: transform 200ms var(--ease); }
.open .chev { transform: rotate(90deg); }
.name { font-weight: 500; }
.sum { margin-left: auto; color: var(--text2); font-size: 11px; font-variant-numeric: tabular-nums; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
/* The body opens by growing from nothing: the grid row goes from 0 to its content. */
.body { display: grid; grid-template-rows: 0fr; opacity: 0; transition: grid-template-rows 250ms var(--ease), opacity 200ms var(--ease); }
.open .body { grid-template-rows: 1fr; opacity: 1; }
.inner { overflow: hidden; min-height: 0; padding-bottom: 0; }
.open .inner { padding-bottom: 8px; }
</style>
