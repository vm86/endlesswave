<script setup lang="ts">
import { mdiFolderMusicOutline, mdiKeyboardOutline } from "@mdi/js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import Deck from "./components/Deck.vue";
import Playlist from "./components/Playlist.vue";
import { copyText } from "./clipboard";
import { useRadio } from "./composables/useRadio";

const TABS = [
  { id: "list", label: "Playlist" },
  { id: "mix", label: "Mix" },
  { id: "sources", label: "Sources" },
  { id: "stats", label: "Stats" },
  { id: "settings", label: "Settings" },
];
const SHORTCUTS: Array<[string[], string]> = [
  [["Space"], "Play / Pause"],
  [["⌘", "."], "Stop"],
  [["⌘", "→"], "Next track"],
  [["⌘", "R"], "Rebuild the queue"],
  [["⌘", "L"], "Switch MP3 / FLAC"],
  [["⌘", "↑"], "Volume up"],
  [["⌘", "↓"], "Volume down"],
  [["⌘", "1–5"], "Playlist, Mix, Sources, Stats, Settings"],
  [["⌘", ","], "Settings"],
  [["⌘", "/"], "This sheet"],
];

const tab = ref("list");
async function copy(text: string, what = "Code") {
  notice.value = (await copyText(text)) ? `${what} copied` : "Could not copy";
}
const help = ref(false);
const { starting, engine, activeEngine, moveTrack, removeTrack, playTrack, refreshing, notice, skipping, loading, setLanguage, format, eqNote, levels, queue, auth, genres, stats, setBlocked, setBackend, signIn, error, playing, volume, elapsed, toggle, stop, next, refreshQueue, setMoods, setGenres } = useRadio();
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

// On macOS the shortcuts belong to the menu bar (src-tauri/src/menu.rs), and so do the actions
// of its items. Space has no menu item (it would eat the spaces of a text field): the page keeps it.
const mac = document.documentElement.classList.contains("mac");

// A key press that changes something without a visible control moving says so for a moment (the
// toast), so that no key goes unanswered. The buttons show their own state.
const hud = (text: string) => (notice.value = text);

const actions: Record<string, () => void> = {
  play: () =>
    void toggle().then(() => hud(loading.value ? "Connecting…" : playing.value ? "Playing" : "Paused")),
  stop: () => {
    void stop();
    hud("Stopped");
  },
  next: () => {
    void next();
    hud("Next track");
  },
  refresh: () => void refreshQueue(),
  lossless: () => {
    flipFormat();
    hud(format.value === "flac" ? "Lossless stream (FLAC)" : "MP3 stream (320 kbit/s)");
  },
  "volume-up": () => {
    volume.value = Math.min(100, volume.value + 10);
    hud(`Volume ${volume.value}%`);
  },
  "volume-down": () => {
    volume.value = Math.max(0, volume.value - 10);
    hud(volume.value === 0 ? "Muted" : `Volume ${volume.value}%`);
  },
  help: () => (help.value = !help.value),
  settings: () => (tab.value = "settings"),
  ...Object.fromEntries(TABS.map((t, i) => [`tab-${i + 1}`, () => (tab.value = t.id)])),
};

// A folder dropped on the window is the folder of the local source.
const dropping = ref(false);
const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;
async function dropFolder(paths: string[]) {
  const path = paths[0];
  const local = auth.value?.backends.find((b) => b.id === "local");
  if (!path || !local) return;
  const field = local.setting_fields?.[0]?.name ?? "directory";
  if (!local.enabled && !(await setBackend("local", true))) return;
  if (await signIn("local", { [field]: path })) {
    hud(`Playing from ${folderName(path)}`);
    tab.value = "sources";
  }
}

function onKey(event: KeyboardEvent) {
  if ((event.target as HTMLElement | null)?.tagName === "INPUT") return;
  const cmd = event.metaKey || event.ctrlKey;
  const run = (action: () => void) => {
    event.preventDefault();
    action();
  };
  if (event.key === "Escape" && help.value) return run(() => (help.value = false));
  if (event.key === " " && !cmd) return run(actions.play!);
  if (!cmd || mac) return;
  const index = Number(event.key);
  if (index >= 1 && index <= TABS.length) return run(actions[`tab-${index}`]!);
  const byKey: Record<string, string> = {
    ".": "stop",
    ArrowRight: "next",
    r: "refresh",
    l: "lossless",
    ArrowUp: "volume-up",
    ArrowDown: "volume-down",
    "/": "help",
    ",": "settings",
  };
  const name = byKey[event.key];
  if (name) run(actions[name]!);
}

// What the menu bar shows: Next is off when the radio cannot skip, FLAC is checked while it plays.
watch(
  [() => queue.value?.can_skip, skipping, format],
  () => {
    void invoke("update_menu", { canSkip: (queue.value?.can_skip ?? false) && !skipping.value, lossless: format.value === "flac" });
  },
  { immediate: true },
);

let unlistenMenu: (() => void) | undefined;
const unlistenDrop: Array<() => void> = [];
onMounted(async () => {
  window.addEventListener("keydown", onKey);
  unlistenMenu = await listen<string>("menu", (event) => actions[event.payload]?.());
  // Files dragged in from Finder (the events of the window; the queue's own drag is not one of them).
  unlistenDrop.push(await listen("tauri://drag-enter", () => (dropping.value = !!auth.value?.backends.some((b) => b.id === "local"))));
  unlistenDrop.push(await listen("tauri://drag-leave", () => (dropping.value = false)));
  unlistenDrop.push(
    await listen<{ paths: string[] }>("tauri://drag-drop", (event) => {
      dropping.value = false;
      void dropFolder(event.payload.paths);
    }),
  );
});
onUnmounted(() => {
  window.removeEventListener("keydown", onKey);
  unlistenMenu?.();
  unlistenDrop.forEach((f) => f());
});
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
          <div class="code-line">
            <span>Sign in to Yandex Music: enter the code</span>
            <code class="code" title="Click to copy" @click="copy(auth.code)">{{ auth.code }}</code>
          </div>
          <span class="actions">
            <button class="btn-quiet" @click="copy(auth.code)">Copy code</button>
            <button class="btn-quiet" @click="copy(auth.url, 'Link')">Copy link</button>
            <button class="btn-accent" @click="openUrl(auth.url)">Open</button>
          </span>
        </template>
        <span v-else>Waiting for Yandex…</span>
      </div>

      <div class="seg" role="tablist">
        <button v-for="(t, i) in TABS" :key="t.id" :class="{ on: tab === t.id }" :title="`${t.label}  (⌘${i + 1})`"
          @click="tab = t.id">{{ t.label }}</button>
      </div>

      <Playlist :queue="queue" :moods="queue?.moods" :language="queue?.language" :engine="engine" :active-engine="activeEngine" @engine="engine = $event" :genres="genres" :auth="auth" :stats="stats" :tab="tab"
        @play="playTrack" @move="moveTrack" @remove="removeTrack" @moods="setMoods" @language="setLanguage" @genres="setGenres" @blocked="setBlocked" @toggle-backend="setBackend" @sign-in="signIn" @copy="copy" />

      <div class="status">
        <span>{{ eqNote || (loading ? "Connecting…" : playing ? `Playing · ${activeEngine === "native" ? "native" : "browser"}` : "Stopped") }}</span>
        <span>{{ queue?.next.length ?? 0 }} queued · ~{{ Math.round(left / 60) }} min</span>
      </div>

      <Transition name="fade">
        <div v-if="starting" class="starting">
          <v-progress-circular indeterminate size="28" width="3" />
          <span>Starting the radio…</span>
        </div>
      </Transition>

      <Transition name="fade">
        <div v-if="dropping" class="drop">
          <v-icon :icon="mdiFolderMusicOutline" size="44" />
          <span>Drop a folder to play its music</span>
        </div>
      </Transition>

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
