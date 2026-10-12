<script setup lang="ts">
defineProps<{ engine: "native" | "web"; active: "native" | "web" }>();
const emit = defineEmits<{ engine: ["native" | "web"] }>();
</script>

<template>
  <div class="mix">
    <h4>Player</h4>
    <span class="chip" :class="{ sel: engine === 'native' }" @click="emit('engine', 'native')">native</span>
    <span class="chip" :class="{ sel: engine === 'web' }" @click="emit('engine', 'web')">browser</span>
    <div class="dim">
      Native plays the stream with the system's audio output. Browser plays it with the audio of the
      window: it is the fall back, and the app uses it by itself when the native player cannot start.
    </div>
    <div v-if="engine !== active" class="dim">Playing now: {{ active === "native" ? "native" : "browser" }} (fell back).</div>
  </div>
</template>
