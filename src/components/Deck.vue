<script setup lang="ts">
import { mdiMusicNote, mdiPause, mdiPlay, mdiRefresh, mdiSkipNext, mdiStop, mdiVolumeHigh } from "@mdi/js";
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
}>();
const volume = defineModel<number>("volume", { required: true });
defineEmits<{ toggle: []; stop: []; next: []; refresh: []; format: [] }>();

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
      <div class="eq"><i v-for="(level, i) in levels" :key="i" :style="{ transform: `scaleY(${Math.max(0.06, level)})` }" /></div>
    </div>

    <div class="progress">
      <span>{{ clock(elapsed) }}</span>
      <div class="seek"><div :style="{ width: `${percent}%` }" /></div>
      <span>−{{ clock(remaining) }}</span>
    </div>

    <div class="transport">
      <div class="tbtn" :class="{ on: playing && !loading, loading }" :title="loading ? 'Connecting…  (click to cancel)' : 'Play / Pause  (Space)'"
        @click="$emit('toggle')">
        <v-progress-circular v-if="loading" indeterminate :size="16" :width="2" />
        <v-icon v-else :icon="playing ? mdiPause : mdiPlay" size="20" />
      </div>
      <div class="tbtn" title="Stop  (⌘.)" @click="$emit('stop')"><v-icon :icon="mdiStop" size="20" /></div>
      <div class="tbtn" :class="{ loading: busy }" :disabled="(!canSkip && !busy) || undefined"
        :title="busy ? 'Switching…' : 'Next track  (⌘→)'" @click="busy || $emit('next')">
        <v-progress-circular v-if="busy" indeterminate :size="16" :width="2" />
        <v-icon v-else :icon="mdiSkipNext" size="20" />
      </div>
      <div class="tbtn" :class="{ loading: refreshing }" :title="refreshing ? 'Rebuilding…' : 'Rebuild the queue  (⌘R)'"
        @click="refreshing || $emit('refresh')">
        <v-icon :icon="mdiRefresh" size="18" :class="{ spin: refreshing }" />
      </div>
      <div class="tbtn fmt" :title="`Stream: ${format.toUpperCase()}  (⌘L to switch)`" @click="$emit('format')">
        {{ format.toUpperCase() }}
      </div>
      <v-icon :icon="mdiVolumeHigh" size="18" class="ml-2" color="secondary" />
      <v-slider v-model="volume" class="vol" :min="0" :max="100" hide-details density="compact" color="primary"
        track-size="3" thumb-size="12" title="Volume  (⌘↑ ⌘↓)" />
    </div>
  </div>
</template>
