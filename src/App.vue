<script setup lang="ts">
import { mdiKeyboardOutline } from "@mdi/js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import Deck from "./components/Deck.vue";
import Playlist from "./components/Playlist.vue";
import { useRadio } from "./composables/useRadio";

const TABS = [
  { id: "list", label: "Playlist" },
  { id: "mix", label: "Mix" },
  { id: "sources", label: "Sources" },
  { id: "stats", label: "Stats" },
];
const SHORTCUTS: Array<[string[], string]> = [
  [["Space"], "Play / Pause"],
  [["⌘", "."], "Stop"],
  [["⌘", "→"], "Next track"],
  [["⌘", "R"], "Rebuild the queue"],
  [["⌘", "L"], "Switch MP3 / FLAC"],
  [["⌘", "↑"], "Volume up"],
  [["⌘", "↓"], "Volume down"],
  [["⌘", "1–4"], "Playlist, Mix, Sources, Stats"],
  [["⌘", "/"], "This sheet"],
];

const tab = ref("list");
const help = ref(false);
const { moveTrack, removeTrack, playTrack, refreshing, notice, skipping, loading, setLanguage, format, eqNote, levels, queue, auth, genres, stats, setBlocked, setBackend, signIn, error, playing, volume, elapsed, toggle, stop, next, refreshQueue, setMoods, setGenres } = useRadio();
const left = computed(() => (queue.value?.next ?? []).reduce((sum, t) => sum + t.duration_seconds, 0));
const flipFormat = () => (format.value = format.value === "mp3" ? "flac" : "mp3");

// An error is a toast: it goes by itself after a while, or on a click.
let toastTimer: ReturnType<typeof setTimeout> | undefined;
let noticeTimer: ReturnType<typeof setTimeout> | undefined;
watch(notice, (value) => {
  clearTimeout(noticeTimer);
  if (value) noticeTimer = setTimeout(() => (notice.value = ""), 2500);
});
watch(error, (value) => {
  clearTimeout(toastTimer);
  if (value) toastTimer = setTimeout(() => (error.value = ""), 8000);
});

function onKey(event: KeyboardEvent) {
  if ((event.target as HTMLElement | null)?.tagName === "INPUT") return;
  const cmd = event.metaKey || event.ctrlKey;
  const run = (action: () => void) => {
    event.preventDefault();
    action();
  };
  if (event.key === "Escape" && help.value) return run(() => (help.value = false));
  if (event.key === " " && !cmd) return run(() => void toggle());
  if (!cmd) return;
  const index = Number(event.key);
  if (index >= 1 && index <= TABS.length) return run(() => (tab.value = TABS[index - 1]!.id));
  const byKey: Record<string, () => void> = {
    ".": () => stop(),
    ArrowRight: () => void next(),
    r: () => void refreshQueue(),
    l: flipFormat,
    ArrowUp: () => (volume.value = Math.min(100, volume.value + 10)),
    ArrowDown: () => (volume.value = Math.max(0, volume.value - 10)),
    "/": () => (help.value = !help.value),
  };
  const action = byKey[event.key];
  if (action) run(action);
}
onMounted(() => window.addEventListener("keydown", onKey));
onUnmounted(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <v-app>
    <v-main class="player">
      <div class="topbar" data-tauri-drag-region>
        <div class="title" data-tauri-drag-region>Yamusic Radio</div>
        <div class="icon-btn" title="Keyboard shortcuts  (⌘/)" @click="help = true">
          <v-icon :icon="mdiKeyboardOutline" size="18" />
        </div>
      </div>

      <Deck v-model:volume="volume" :now="queue?.now ?? null" :elapsed="elapsed" :playing="playing" :loading="loading" :refreshing="refreshing"
        :busy="skipping || (queue?.skip_pending ?? false) || (queue?.blending ?? false)" :levels="levels" :format="format"
        :can-skip="queue?.can_skip ?? false" :paused="queue?.paused ?? false" @toggle="toggle" @stop="stop"
        @next="next" @refresh="refreshQueue" @format="flipFormat" />

      <div v-if="auth && !auth.authorized" class="auth">
        <template v-if="auth.url && auth.code">
          <span>Sign in to Yandex Music: code <b>{{ auth.code }}</b></span>
          <button class="btn-accent" @click="openUrl(auth.url)">Open</button>
        </template>
        <span v-else>Waiting for Yandex…</span>
      </div>

      <div class="seg" role="tablist">
        <button v-for="(t, i) in TABS" :key="t.id" :class="{ on: tab === t.id }" :title="`${t.label}  (⌘${i + 1})`"
          @click="tab = t.id">{{ t.label }}</button>
      </div>

      <Playlist :queue="queue" :moods="queue?.moods" :language="queue?.language" :genres="genres" :auth="auth" :stats="stats" :tab="tab"
        @play="playTrack" @move="moveTrack" @remove="removeTrack" @moods="setMoods" @language="setLanguage" @genres="setGenres" @blocked="setBlocked" @toggle-backend="setBackend" @sign-in="signIn" />

      <div class="status">
        <span>{{ eqNote || (loading ? "Connecting…" : playing ? "Playing" : "Stopped") }}</span>
        <span>{{ queue?.next.length ?? 0 }} queued · ~{{ Math.round(left / 60) }} min</span>
      </div>

      <Transition name="fade">
        <div v-if="error || notice" class="toast" @click="error = ''; notice = ''">{{ error || notice }}</div>
      </Transition>

      <Transition name="fade">
        <div v-if="help" class="sheet-wrap" @click.self="help = false">
          <div class="sheet">
            <h3>Keyboard shortcuts</h3>
            <div v-for="[keys, what] in SHORTCUTS" :key="what" class="kv">
              <span>{{ what }}</span>
              <span class="d-flex ga-1"><kbd v-for="k in keys" :key="k">{{ k }}</kbd></span>
            </div>
          </div>
        </div>
      </Transition>
    </v-main>
  </v-app>
</template>
