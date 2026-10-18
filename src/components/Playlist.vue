<script setup lang="ts">
import { mdiClose, mdiMusicNote, mdiPlay, mdiRadioTower } from "@mdi/js";
import { computed, onUnmounted, ref } from "vue";
import { clock, coverUrl, type AuthInfo, type GenresInfo, type LanguageInfo, type MoodsInfo, type NowTrack, type Queue, type Stats, type TrackInfo } from "../api";
import MixPanel from "./MixPanel.vue";
import SourcesPanel from "./SourcesPanel.vue";
import StatsPanel from "./StatsPanel.vue";

const props = defineProps<{
  queue: Queue | null;
  moods: MoodsInfo | undefined;
  language: LanguageInfo | undefined;
  genres: GenresInfo | null;
  auth: AuthInfo | null;
  stats: Stats | null;
  tab: string;
}>();
const emit = defineEmits<{
  moods: [string[]];
  genres: [string[]];
  blocked: [string[]];
  language: [string | null];
  toggleBackend: [id: string, enabled: boolean];
  signIn: [id: string, values: Record<string, string>];
  backendSettings: [id: string, values: Record<string, string>];
  play: [id: string];
  move: [id: string, position: number];
  remove: [id: string];
  copy: [text: string, what?: string];
}>();

interface Row {
  track: TrackInfo | NowTrack;
  kind: "played" | "current" | "next";
  /** The place among the tracks that are next, -1 for the others. */
  at: number;
}
const rows = computed<Row[]>(() => [
  ...(props.queue?.previous ?? []).map((track) => ({ track, kind: "played" as const, at: -1 })),
  ...(props.queue?.now ? [{ track: props.queue.now, kind: "current" as const, at: -1 }] : []),
  ...(props.queue?.next ?? []).map((track, at) => ({ track, kind: "next" as const, at })),
]);

// The next track is fixed while it is being blended in: it can't be moved, taken
// out, or have another put before it.
const locked = (row: Row) => row.kind === "next" && row.at === 0 && (props.queue?.blending ?? false);

// Moving a track by the mouse: press, drag, a line shows where it goes, release.
const list = ref<HTMLElement>();
const dragging = ref<{ id: string; from: number } | null>(null);
const slot = ref(-1); // the place the dragged track would take: before the next track of this index
let press: { id: string; from: number; y: number } | null = null;
const DRAG_FROM_PX = 4;

function onDown(event: PointerEvent, row: Row) {
  if (event.button !== 0 || row.kind !== "next" || locked(row)) return;
  if ((event.target as HTMLElement).closest(".acts")) return;
  press = { id: row.track.track_id, from: row.at, y: event.clientY };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("keydown", onKey);
}
function slotAt(y: number): number {
  const rects = [...(list.value?.querySelectorAll<HTMLElement>(".row.next") ?? [])].map((el) => el.getBoundingClientRect());
  let at = rects.length;
  for (let i = 0; i < rects.length; i++) {
    if (y < rects[i]!.top + rects[i]!.height / 2) {
      at = i;
      break;
    }
  }
  return Math.max(props.queue?.blending ? 1 : 0, at);
}
function onMove(event: PointerEvent) {
  if (!press) return;
  if (!dragging.value) {
    if (Math.abs(event.clientY - press.y) < DRAG_FROM_PX) return;
    dragging.value = { id: press.id, from: press.from };
  }
  slot.value = slotAt(event.clientY);
}
function stopDrag() {
  press = null;
  dragging.value = null;
  slot.value = -1;
  window.removeEventListener("pointermove", onMove);
  window.removeEventListener("pointerup", onUp);
  window.removeEventListener("keydown", onKey);
}
function onUp() {
  const drag = dragging.value;
  const at = slot.value;
  stopDrag();
  if (!drag || at < 0) return;
  const position = at > drag.from ? at - 1 : at; // the track leaves its place first
  if (position !== drag.from) emit("move", drag.id, position);
}
function onKey(event: KeyboardEvent) {
  if (event.key === "Escape") stopDrag();
}
onUnmounted(stopDrag);

// The menu of a right click on a track.
const menu = ref<{ x: number; y: number; row: Row } | null>(null);
function openMenu(event: MouseEvent, row: Row) {
  menu.value = { x: Math.min(event.clientX, window.innerWidth - 190), y: Math.min(event.clientY, window.innerHeight - 110), row };
}
const closeMenu = () => (menu.value = null);
function choose(action: "play" | "remove" | "copy") {
  const row = menu.value?.row;
  closeMenu();
  if (!row) return;
  if (action === "play") emit("play", row.track.track_id);
  else if (action === "remove") emit("remove", row.track.track_id);
  else emit("copy", `${row.track.artists.join(", ")} — ${row.track.title}`, "Title");
}
window.addEventListener("pointerdown", (event) => {
  if (menu.value && !(event.target as HTMLElement | null)?.closest(".ctx")) closeMenu();
});
window.addEventListener("blur", closeMenu);
window.addEventListener("keydown", (event) => event.key === "Escape" && closeMenu());

</script>

<template>
  <div ref="list" class="list" :class="{ dragging }">
    <template v-if="tab === 'list'">
      <div v-for="row in rows" :key="row.track.track_id" class="row" :class="[row.kind, {
        dragged: dragging?.id === row.track.track_id,
        'drop-before': dragging && row.kind === 'next' && slot === row.at,
        'drop-end': dragging && row.kind === 'next' && slot === (queue?.next.length ?? 0) && row.at === (queue?.next.length ?? 0) - 1,
        draggable: row.kind === 'next' && !locked(row),
      }]" @pointerdown="onDown($event, row)" @contextmenu.prevent="openMenu($event, row)" @dblclick="row.kind === 'next' && queue?.can_skip && emit('play', row.track.track_id)">
        <span class="thumb">
          <img v-if="row.track.cover" :src="coverUrl(row.track.cover, 100)" alt="" loading="lazy" draggable="false" referrerpolicy="no-referrer" />
          <v-icon v-else :icon="mdiMusicNote" size="16" />
        </span>
        <span class="t">
          <b>{{ row.track.title }}</b>
          <small>{{ row.track.artists.join(", ") }}</small>
        </span>
        <span class="d">{{ clock(row.track.duration_seconds) }}</span>
        <span v-if="row.kind === 'next'" class="acts">
          <button :disabled="!queue?.can_skip" title="Play now" @click="emit('play', row.track.track_id)"><v-icon :icon="mdiPlay" size="13" /></button>
          <button :disabled="locked(row)" title="Remove from the queue" @click="emit('remove', row.track.track_id)"><v-icon :icon="mdiClose" size="13" /></button>
        </span>
      </div>
      <div v-if="!rows.length" class="empty">
        <v-icon :icon="mdiRadioTower" size="36" />
        <span>{{ queue ? "The radio is picking the first tracks…" : "Connecting to the radio…" }}</span>
      </div>
    </template>
    <MixPanel v-else-if="tab === 'mix'" :moods="moods" :genres="genres" :language="language" @language="emit('language', $event)" @moods="emit('moods', $event)"
      @genres="emit('genres', $event)" @blocked="emit('blocked', $event)" />
    <SourcesPanel v-else-if="tab === 'sources'" :auth="auth" @toggle="(id, on) => emit('toggleBackend', id, on)"
      @sign-in="(id, v) => emit('signIn', id, v)"
      @setting="(id, v) => emit('backendSettings', id, v)" @copy="(t, w) => emit('copy', t, w)" />
    <StatsPanel v-else :stats="stats" />
    <div v-if="menu" class="ctx" :style="{ left: menu.x + 'px', top: menu.y + 'px' }" @contextmenu.prevent>
      <template v-if="menu.row.kind === 'next'">
        <button :disabled="!queue?.can_skip" @click="choose('play')">Play now</button>
        <button :disabled="locked(menu.row)" @click="choose('remove')">Remove from the queue</button>
        <hr />
      </template>
      <button @click="choose('copy')">Copy title</button>
    </div>
  </div>
</template>
