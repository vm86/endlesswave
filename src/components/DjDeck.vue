<script setup lang="ts">
import { mdiFlagVariant, mdiPause, mdiPlay, mdiSkipPrevious } from "@mdi/js";
import { computed } from "vue";
import { clock, coverUrl, type TrackInfo } from "../api";

export interface DeckState {
  state: "empty" | "loading" | "ready" | "error";
  message: string;
  playing: boolean;
  position: number;
  duration: number;
  cue: number;
  tempo: number;
  peak: number;
}

const props = defineProps<{
  id: "a" | "b";
  track: TrackInfo | null;
  deck: DeckState | null;
  /** The tempo of the other deck, to match to; null when there is nothing to match. */
  syncable: boolean;
  eq: { low: number; mid: number; high: number };
  fader: number;
}>();
const emit = defineEmits<{
  set: [control: string, value: number];
  sync: [];
}>();

const ready = computed(() => props.deck?.state === "ready");
const percent = computed(() => ((props.deck?.tempo ?? 1) - 1) * 100);
const bpm = computed(() => (props.track?.bpm ? props.track.bpm * (props.deck?.tempo ?? 1) : null));
const progress = computed(() => (props.deck && props.deck.duration ? Math.min(1, props.deck.position / props.deck.duration) : 0));
const cuePlace = computed(() => (props.deck && props.deck.duration ? props.deck.cue / props.deck.duration : 0));

function seek(event: PointerEvent) {
  if (!ready.value || !props.deck) return;
  const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
  emit("set", `${props.id}.seek`, Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)) * props.deck.duration);
}
const num = (event: Event) => Number((event.target as HTMLInputElement).value);
const bands = [
  ["high", "High"],
  ["mid", "Mid"],
  ["low", "Low"],
] as const;
</script>

<template>
  <section class="djdeck" :class="[id, { 'dj-empty': !track }]" :aria-label="`Deck ${id.toUpperCase()}`">
    <header>
      <span class="letter">{{ id.toUpperCase() }}</span>
      <img v-if="track?.cover" class="dj-art" :src="coverUrl(track.cover, 100)" alt="" />
      <div v-else class="dj-art"></div>
      <div class="dj-meta">
        <div class="title">{{ track?.title ?? "Empty" }}</div>
        <div class="dj-artist">{{ track ? track.artists.join(", ") : "Load a track from the list below" }}</div>
        <div class="chips" v-if="track">
          <span class="dj-chip bpm">{{ bpm ? bpm.toFixed(1) : "?" }} BPM</span>
          <span class="dj-chip">{{ track.camelot || "—" }}</span>
          <span v-if="track.mood" class="dj-chip">{{ track.mood }}</span>
        </div>
      </div>
    </header>

    <div class="time" :class="{ off: !ready }">
      <template v-if="deck?.state === 'loading'">Loading…</template>
      <template v-else-if="deck?.state === 'error'">{{ deck.message }}</template>
      <template v-else-if="ready && deck">
        <span class="dj-now">{{ clock(deck.position) }}</span>
        <span class="left">−{{ clock(Math.max(0, deck.duration - deck.position)) }}</span>
      </template>
      <template v-else>0:00</template>
    </div>
    <div class="bar" :class="{ off: !ready }" role="slider" aria-label="Position" :aria-valuenow="Math.round(progress * 100)" aria-valuemin="0" aria-valuemax="100" @pointerdown="seek">
      <i class="fill" :style="{ width: `${progress * 100}%` }"></i>
      <i class="cue" :style="{ left: `${cuePlace * 100}%` }"></i>
    </div>

    <div class="dj-transport">
      <button type="button" class="pad" :disabled="!ready" title="Back to the cue point" @click="emit('set', `${id}.cue`, 1)">
        <v-icon :icon="mdiSkipPrevious" size="18" /><span>Cue</span>
      </button>
      <button type="button" class="pad play" :class="{ on: deck?.playing }" :disabled="!ready" :title="deck?.playing ? 'Pause' : 'Play'" @click="emit('set', `${id}.play`, deck?.playing ? 0 : 1)">
        <v-icon :icon="deck?.playing ? mdiPause : mdiPlay" size="22" />
      </button>
      <button type="button" class="pad" :disabled="!ready" title="Put the cue point here" @click="emit('set', `${id}.setcue`, 1)">
        <v-icon :icon="mdiFlagVariant" size="16" /><span>Set cue</span>
      </button>
    </div>

    <div class="controls">
      <div class="col dj-eq" v-for="[band, label] in bands" :key="band">
        <input type="range" class="vert" min="-40" max="9" step="0.5" :value="eq[band]" :aria-label="`${label} of deck ${id.toUpperCase()}`"
          @input="emit('set', `${id}.${band}`, num($event))" @dblclick="emit('set', `${id}.${band}`, 0)" />
        <label>{{ label }}</label>
        <small>{{ eq[band] <= -39 ? "kill" : `${eq[band] > 0 ? "+" : ""}${eq[band].toFixed(0)}` }}</small>
      </div>
      <div class="col">
        <input type="range" class="vert" min="0" max="1" step="0.01" :value="fader" :aria-label="`Level of deck ${id.toUpperCase()}`" @input="emit('set', `${id}.fader`, num($event))" />
        <label>Level</label>
        <div class="meter"><i :style="{ height: `${Math.min(100, (deck?.peak ?? 0) * 100)}%` }"></i></div>
      </div>
      <div class="col tempo">
        <input type="range" class="vert" min="-16" max="16" step="0.1" :value="percent" :aria-label="`Tempo of deck ${id.toUpperCase()}`"
          @input="emit('set', `${id}.tempo`, 1 + num($event) / 100)" @dblclick="emit('set', `${id}.tempo`, 1)" />
        <label>Tempo</label>
        <small>{{ percent > 0 ? "+" : "" }}{{ percent.toFixed(1) }}%</small>
        <button type="button" class="pad sync" :disabled="!syncable || !track?.bpm" title="Match the tempo of the other deck" @click="emit('sync')">Sync</button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.djdeck { display: flex; flex-direction: column; gap: 10px; padding: 14px; border-radius: 12px; background: var(--panel); box-shadow: 0 0 0 0.5px var(--panel-border); min-width: 0; }
header { display: flex; gap: 10px; align-items: center; min-width: 0; }
.letter { flex: none; width: 24px; height: 24px; border-radius: 6px; display: grid; place-items: center; font-weight: 700; color: #fff; background: var(--accent); }
.b .letter { background: #ff9f0a; }
.dj-art { flex: none; width: 56px; height: 56px; border-radius: 6px; background: var(--control); object-fit: cover; }
.dj-meta { min-width: 0; flex: 1; }
.title { font-weight: 600; font-size: 14px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.dj-artist { color: var(--text2); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.dj-empty .title { color: var(--text2); }
.chips { display: flex; gap: 5px; margin-top: 4px; }
.dj-chip { padding: 0 6px; border-radius: 8px; font-size: 11px; line-height: 16px; background: var(--control); color: var(--text2); }
.dj-chip.bpm { color: var(--text); font-variant-numeric: tabular-nums; font-weight: 600; }
.time { display: flex; justify-content: space-between; align-items: baseline; font-family: var(--mono); font-variant-numeric: tabular-nums; font-size: 22px; min-height: 28px; }
.time.off { font-size: 13px; color: var(--text2); font-family: var(--font); }
.time .left { font-size: 13px; color: var(--text2); }
.bar { position: relative; height: 22px; border-radius: 6px; background: var(--control); cursor: pointer; overflow: hidden; touch-action: none; }
.bar.off { opacity: 0.5; cursor: default; }
.bar .fill { position: absolute; inset: 0 auto 0 0; background: color-mix(in srgb, var(--accent) 55%, transparent); }
.b .bar .fill { background: color-mix(in srgb, #ff9f0a 55%, transparent); }
.bar .cue { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--red); }
.dj-transport { display: flex; gap: 8px; justify-content: center; }
.pad { min-width: 64px; height: 38px; padding: 0 12px; border-radius: 8px; display: inline-flex; gap: 5px; align-items: center; justify-content: center; font-size: 12px; font-weight: 500;
  background: var(--control); color: var(--text); cursor: default; transition: background 120ms var(--ease); }
.pad:hover:not(:disabled) { background: var(--control-hover); }
.pad:active:not(:disabled) { transform: scale(0.97); }
.pad:disabled { opacity: 0.4; }
.pad.play { min-width: 84px; }
.pad.play.on { background: var(--accent); color: #fff; }
.b .pad.play.on { background: #ff9f0a; }
.controls { display: flex; gap: 6px; justify-content: space-between; align-items: stretch; margin-top: 2px; }
.col { flex: 1; display: flex; flex-direction: column; align-items: center; gap: 4px; min-width: 0; position: relative; }
.col label { font-size: 11px; color: var(--text2); }
.col small { font-size: 11px; color: var(--text2); font-variant-numeric: tabular-nums; min-height: 14px; }
.vert { writing-mode: vertical-lr; direction: rtl; width: 22px; height: 130px; accent-color: var(--accent); cursor: default; margin: 0; }
.b .vert { accent-color: #ff9f0a; }
.meter { position: absolute; right: 4px; top: 4px; bottom: 36px; width: 4px; border-radius: 2px; background: var(--control); display: flex; align-items: flex-end; overflow: hidden; }
.meter i { width: 100%; background: #30d158; transition: height 60ms linear; }
.sync { min-width: 0; width: 100%; height: 26px; margin-top: 2px; }
</style>
