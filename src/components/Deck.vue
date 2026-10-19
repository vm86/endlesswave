<script setup lang="ts">
import { mdiHeart, mdiHeartOutline, mdiMusicNote, mdiPause, mdiPlay, mdiRefresh, mdiSkipNext, mdiStop, mdiVolumeHigh } from "@mdi/js";
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { clock, coverUrl, type NowTrack } from "../api";

const props = defineProps<{
  now: NowTrack | null;
  elapsed: number;
  playing: boolean;
  loading: boolean;
  refreshing: boolean;
  busy: boolean;
  canSkip: boolean;
  paused: boolean;
  levels: number[];
  format: "mp3" | "flac";
  likesSupported: boolean;
  liked: boolean;
}>();
const volume = defineModel<number>("volume", { required: true });
const emit = defineEmits<{ toggle: []; stop: []; next: []; refresh: []; clear: []; equalizer: []; format: []; like: [] }>();

// "Rebuild the queue" held for HOLD_MS clears the whole queue and loads it anew; a short press rebuilds as before.
const HOLD_MS = 3000;
const holding = ref(false);
let holdTimer: number | undefined;
let held = false;
function holdStart(e: PointerEvent) {
  if (props.refreshing || e.button !== 0) return;
  held = false;
  holding.value = true;
  holdTimer = window.setTimeout(() => {
    held = true;
    holding.value = false;
    emit("clear");
  }, HOLD_MS);
}
function holdCancel() {
  clearTimeout(holdTimer);
  holding.value = false;
}
// A click that came from the keyboard (Space or Return on the focused button) rebuilds the queue; the pointer has its own.
function keyboardClick(event: MouseEvent) {
  if (event.detail === 0 && !props.refreshing) emit("refresh");
}
function holdEnd() {
  const fired = held;
  held = false;
  const pressing = holding.value;
  holdCancel();
  if (!fired && pressing && !props.refreshing) emit("refresh");
}

const title = computed(() => props.now?.title ?? "EndlessWave");
const artist = computed(() => props.now?.artists.join(", ") || (props.now ? "" : "Waiting for the first track…"));
const tags = computed(() => {
  const n = props.now;
  if (!n) return "";
  return [`${Math.round(n.bpm)} BPM`, n.camelot, n.mood, props.format.toUpperCase()].filter(Boolean).join("  ·  ");
});
const percent = computed(() => (props.now ? (props.elapsed / props.now.duration_seconds) * 100 : 0));
const remaining = computed(() => (props.now ? props.now.duration_seconds - props.elapsed : 0));
const stopped = computed(() => !!props.now && (props.paused || !props.playing));

const coverFailed = ref(false);
watch(() => props.now?.cover, () => (coverFailed.value = false));
const cover = computed(() => (props.now && !coverFailed.value ? coverUrl(props.now.cover, 200) : ""));

// Scroll the title only when it really does not fit its box.
const box = ref<HTMLElement>();
const text = ref<HTMLElement>();
const boxWidth = ref(0);
const overflow = ref(false);
function measure() {
  if (!box.value || !text.value) return;
  boxWidth.value = box.value.clientWidth;
  overflow.value = text.value.offsetWidth > boxWidth.value;
}
const observer = new ResizeObserver(measure);
onMounted(() => {
  if (box.value) observer.observe(box.value);
  measure();
});
onUnmounted(() => observer.disconnect());
watch(title, () => void nextTick(measure));
</script>

<template>
  <div class="deck">
    <div class="now">
      <div class="art">
        <img v-if="cover" :src="cover" alt="" draggable="false" referrerpolicy="no-referrer" @error="coverFailed = true" />
        <v-icon v-else :icon="mdiMusicNote" size="26" />
      </div>
      <div class="meta">
        <div ref="box" class="marquee">
          <span ref="text" :class="{ scroll: overflow }"
            :style="{ '--box': `${boxWidth}px`, '--dur': `${Math.max(8, title.length * 0.3)}s` }">{{ title }}</span>
        </div>
        <div class="artist">{{ artist || " " }}</div>
        <div class="tags">{{ tags || " " }}<span v-if="stopped" class="paused">Paused</span></div>
      </div>
      <button type="button" class="eq" title="Equalizer  (⌘E)" aria-label="Open the equalizer" @click="emit('equalizer')">
        <i v-for="(level, i) in levels" :key="i" :style="{ transform: `scaleY(${Math.max(0.06, level)})` }" />
      </button>
    </div>

    <div class="progress">
      <span>{{ clock(elapsed) }}</span>
      <div class="seek"><div :style="{ width: `${percent}%` }" /></div>
      <span>−{{ clock(remaining) }}</span>
    </div>

    <div class="transport">
      <button type="button" class="tbtn" :class="{ on: playing && !loading, loading }"
        :title="loading ? 'Connecting…  (click to cancel)' : 'Play / Pause  (Space)'" :aria-label="loading ? 'Cancel' : playing ? 'Pause' : 'Play'"
        @click="$emit('toggle')">
        <v-progress-circular v-if="loading" indeterminate :size="16" :width="2" />
        <v-icon v-else :icon="playing ? mdiPause : mdiPlay" size="20" />
      </button>
      <button type="button" class="tbtn" title="Stop  (⌘.)" aria-label="Stop" @click="$emit('stop')"><v-icon :icon="mdiStop" size="20" /></button>
      <button type="button" class="tbtn" :class="{ loading: busy }" :disabled="!canSkip && !busy" aria-label="Next track"
        :title="busy ? 'Switching…' : 'Next track  (⌘→)'" @click="busy || $emit('next')">
        <v-progress-circular v-if="busy" indeterminate :size="16" :width="2" />
        <v-icon v-else :icon="mdiSkipNext" size="20" />
      </button>
      <button type="button" class="tbtn hold" :class="{ loading: refreshing, holding }" aria-label="Rebuild the queue"
        :title="refreshing ? 'Rebuilding…' : 'Rebuild the queue  (⌘R)\nHold for 3 s, or ⌘⇧R: clear the queue and load it anew'"
        @pointerdown="holdStart" @pointerup="holdEnd" @pointerleave="holdCancel" @pointercancel="holdCancel" @click="keyboardClick">
        <v-icon :icon="mdiRefresh" size="18" :class="{ spin: refreshing }" />
      </button>
      <button v-if="likesSupported && now" type="button" class="tbtn like" :class="{ on: liked, liked }"
        :title="liked ? 'Remove the like  (⌘D)' : 'Like  (⌘D)'" :aria-label="liked ? 'Remove the like' : 'Like the track'" :aria-pressed="liked"
        @click="$emit('like')">
        <v-icon :icon="liked ? mdiHeart : mdiHeartOutline" size="18" />
      </button>
      <button type="button" class="tbtn fmt" :title="`Stream: ${format.toUpperCase()}  (⌘L to switch)`" :aria-label="`Stream format ${format.toUpperCase()}`"
        @click="$emit('format')">
        {{ format.toUpperCase() }}
      </button>
      <v-icon :icon="mdiVolumeHigh" size="18" class="ml-2" color="secondary" />
      <v-slider v-model="volume" class="vol" :min="0" :max="100" hide-details density="compact" color="primary"
        track-size="3" thumb-size="12" title="Volume  (⌘↑ ⌘↓)" />
    </div>
  </div>
</template>
