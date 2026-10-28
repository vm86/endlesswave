<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { computed, onBeforeUnmount, onMounted, reactive, ref } from "vue";
import { api, baseUrl, clock, coverUrl, setBase, type TrackInfo } from "./api";
import DjDeck, { type DeckState } from "./components/DjDeck.vue";

type Id = "a" | "b";
interface Status {
  running: boolean;
  decks: [DeckState, DeckState];
}

const status = ref<Status | null>(null);
const error = ref("");
const library = ref<TrackInfo[]>([]);
const tracks = reactive<Record<Id, TrackInfo | null>>({ a: null, b: null });
// What the controls say. The audio side keeps its own copy; these are sent whenever they change.
const xfade = ref(0.5);
const master = ref(0.9);
const knobs = reactive({
  a: { low: 0, mid: 0, high: 0, fader: 1 },
  b: { low: 0, mid: 0, high: 0, fader: 1 },
});

const set = (name: string, value: number) => {
  const [deck, what] = name.split(".") as [Id, string];
  if (what && what in knobs[deck]) (knobs[deck] as Record<string, number>)[what] = value;
  void invoke("dj_set", { name, value }).catch((e) => (error.value = String(e)));
};
const setNow = (name: string, value: number) => set(name, value);

async function load(deck: Id, track: TrackInfo) {
  error.value = "";
  tracks[deck] = track;
  lastLoaded.value = deck;
  try {
    await invoke("dj_load", { deck, url: `${baseUrl()}/api/track/${encodeURIComponent(track.track_id)}/pcm` });
    // A new track begins at its own tempo, with its own sound.
    for (const name of ["low", "mid", "high"]) set(`${deck}.${name}`, 0);
    set(`${deck}.tempo`, 1);
  } catch (e) {
    error.value = String(e);
  }
}

/** The tempo that makes `own` play at the speed of `other`, an octave up or down if that is nearer (70 BPM goes with 140). */
function matchTempo(own: number, other: number): number | null {
  const options = [other / own, (other * 2) / own, other / 2 / own];
  const best = options.reduce((a, b) => (Math.abs(b - 1) < Math.abs(a - 1) ? b : a));
  return best >= 0.84 && best <= 1.16 ? best : null;
}
function sync(deck: Id) {
  const other: Id = deck === "a" ? "b" : "a";
  const own = tracks[deck]?.bpm;
  const against = tracks[other]?.bpm;
  const otherTempo = status.value?.decks[other === "a" ? 0 : 1].tempo ?? 1;
  if (!own || !against) return;
  const tempo = matchTempo(own, against * otherTempo);
  if (tempo === null) {
    error.value = "The tempos are too far apart to match with the pitch fader";
    return;
  }
  error.value = "";
  set(`${deck}.tempo`, tempo);
}

const stateOf = (id: Id) => status.value?.decks[id === "a" ? 0 : 1] ?? null;
/** How far the two decks are from each other, in BPM: what the ear hears as the beats drifting. */
const drift = computed(() => {
  const [a, b] = [tracks.a, tracks.b];
  if (!a?.bpm || !b?.bpm || !status.value) return null;
  const [sa, sb] = [stateOf("a")!, stateOf("b")!];
  return a.bpm * sa.tempo - b.bpm * sb.tempo;
});

/** The deck the next track is to be mixed with: the one that is heard (by the crossfader) while it plays, else the one loaded last. */
const lastLoaded = ref<Id>("a");
const reference = computed<Id | null>(() => {
  const playing = (["a", "b"] as const).filter((id) => stateOf(id)?.playing && tracks[id]);
  if (playing.length === 2) return xfade.value < 0.5 ? "a" : "b";
  if (playing.length === 1) return playing[0]!;
  return tracks[lastLoaded.value] ? lastLoaded.value : null;
});

/** The wheel of Camelot: the same key, a step round the wheel, or the relative major/minor go together. 0 is the best, 3 clashes. */
function keyFit(a: string, b: string): number {
  const [x, y] = [a.match(/^(\d+)([AB])$/), b.match(/^(\d+)([AB])$/)];
  if (!x || !y) return 3;
  const [nx, ny, lx, ly] = [Number(x[1]), Number(y[1]), x[2], y[2]];
  const round = Math.min((nx - ny + 12) % 12, (ny - nx + 12) % 12);
  if (round === 0) return lx === ly ? 0 : 1;
  if (round === 1 && lx === ly) return 1;
  if (round === 2 && lx === ly) return 2;
  return 3;
}
const KEY_WORDS = ["same key", "fits", "possible", "clashes"];

const MOOD_LEVEL: Record<string, number> = { sad: 0, calm: 1 / 3, fun: 2 / 3, active: 1 };
/** How much is going on in a track, 0 to 1: the tempo (60 BPM is nothing, 180 is all) and the mood, when the service knows it. */
function energy(track: TrackInfo): number {
  const tempo = Math.max(0, Math.min(1, ((track.bpm || 110) - 60) / 120));
  const mood = MOOD_LEVEL[track.mood];
  return mood === undefined ? tempo : 0.55 * tempo + 0.45 * mood;
}
/** The change of energy from the reference to a track: a lift, a drop or about the same. */
const SHIFT = 0.1;
type Move = "lift" | "drop" | "level";
const moveOf = (from: TrackInfo, to: TrackInfo): Move => {
  const delta = energy(to) - energy(from);
  return delta > SHIFT ? "lift" : delta < -SHIFT ? "drop" : "level";
};
const MOVE_MARK: Record<Move, string> = { lift: "↗", drop: "↘", level: "→" };
const MOVE_WORDS: Record<Move, string> = { lift: "The energy rises", drop: "The energy falls", level: "The energy stays" };
const wanted = ref<Move | "any">("any");

interface Fit { key: number; gap: number | null; score: number; move: Move }
/** How a track goes with the reference deck: the key on the wheel and how far the tempo has to be moved to match. */
function fitOf(track: TrackInfo): Fit | null {
  const ref = reference.value;
  const other = ref ? tracks[ref] : null;
  if (!ref || !other || track.track_id === other.track_id) return null;
  const key = keyFit(track.camelot, other.camelot);
  const target = other.bpm * (stateOf(ref)?.tempo ?? 1);
  let gap: number | null = null;
  if (track.bpm && target) {
    const ratio = [target / track.bpm, (target * 2) / track.bpm, target / 2 / track.bpm].reduce((a, b) => (Math.abs(b - 1) < Math.abs(a - 1) ? b : a));
    gap = (ratio - 1) * 100;
  }
  // A tempo out of reach of the pitch fader (±16%) is no match, however the key goes.
  const reach = gap === null ? 2 : Math.abs(gap) > 16 ? 4 : Math.abs(gap) / 8;
  return { key, gap, score: key + reach, move: moveOf(other, track) };
}
const bestFirst = ref(false);
const listed = computed(() => {
  const rows = library.value
    .map((track) => ({ track, fit: fitOf(track) }))
    .filter((row) => wanted.value === "any" || !row.fit || row.fit.move === wanted.value);
  return bestFirst.value && reference.value ? [...rows].sort((a, b) => (a.fit?.score ?? 99) - (b.fit?.score ?? 99)) : rows;
});

let poll: ReturnType<typeof setInterval> | undefined;
let refresh: ReturnType<typeof setInterval> | undefined;
async function loadLibrary() {
  try {
    const queue = await api.queue();
    library.value = [...(queue.now ? [queue.now] : []), ...queue.next];
  } catch {
    /* the radio may be starting; the next round asks again */
  }
}
onMounted(async () => {
  try {
    setBase(await invoke<string>("radio_url"));
    await invoke("dj_start");
  } catch (e) {
    error.value = String(e);
  }
  for (const [name, value] of [["xfade", xfade.value], ["master", master.value]] as const) void invoke("dj_set", { name, value }).catch(() => {});
  await loadLibrary();
  refresh = setInterval(loadLibrary, 8000);
  poll = setInterval(() => {
    invoke<Status>("dj_status").then((s) => (status.value = s)).catch(() => {});
  }, 70);
});
onBeforeUnmount(() => {
  clearInterval(poll);
  clearInterval(refresh);
  void invoke("dj_stop");
});
const loaded = (track: TrackInfo) => (tracks.a?.track_id === track.track_id ? "A" : tracks.b?.track_id === track.track_id ? "B" : "");
</script>

<template>
  <div class="dj">
    <div class="top">
      <h4>DJ Console</h4>
      <span class="note">The radio keeps its own mix; the player of the stream is stopped while this window is open.</span>
    </div>
    <div v-if="error" class="err" role="alert">{{ error }}</div>

    <div class="decks">
      <DjDeck id="a" :track="tracks.a" :deck="stateOf('a')" :eq="knobs.a" :fader="knobs.a.fader" :syncable="!!tracks.b" @set="setNow" @sync="sync('a')" />

      <section class="mixer" aria-label="Mixer">
        <div class="drift" :class="{ ok: drift !== null && Math.abs(drift) < 0.5 }" title="The difference of the tempos of the two decks">
          <span class="label">Tempo gap</span>
          <b>{{ drift === null ? "—" : `${drift > 0 ? "+" : ""}${drift.toFixed(1)} BPM` }}</b>
        </div>
        <div class="master">
          <input type="range" class="vert" min="0" max="1" step="0.01" v-model.number="master" aria-label="Master volume" @input="set('master', master)" />
          <label>Master</label>
        </div>
        <div class="cross">
          <div class="ab"><span>A</span><span>B</span></div>
          <input type="range" min="0" max="1" step="0.005" v-model.number="xfade" aria-label="Crossfader" @input="set('xfade', xfade)" @dblclick="((xfade = 0.5), set('xfade', 0.5))" />
          <label>Crossfader</label>
          <div class="cuts">
            <button type="button" @click="((xfade = 0), set('xfade', 0))">A</button>
            <button type="button" @click="((xfade = 0.5), set('xfade', 0.5))">Center</button>
            <button type="button" @click="((xfade = 1), set('xfade', 1))">B</button>
          </div>
        </div>
      </section>

      <DjDeck id="b" :track="tracks.b" :deck="stateOf('b')" :eq="knobs.b" :fader="knobs.b.fader" :syncable="!!tracks.a" @set="setNow" @sync="sync('b')" />
    </div>

    <section class="library" aria-label="Tracks of the queue">
      <div class="lhead">
        <h4>Queue</h4>
        <span v-if="reference" class="hint">goes with deck {{ reference.toUpperCase() }} · {{ tracks[reference]?.camelot || "?" }}</span>
        <span class="moves" role="group" aria-label="Energy of the transition">
          <button v-for="m in (['any', 'lift', 'level', 'drop'] as const)" :key="m" type="button" :class="{ on: wanted === m }" :disabled="!reference && m !== 'any'" :title="m === 'any' ? 'Any transition' : MOVE_WORDS[m]" @click="wanted = m">
            {{ m === "any" ? "Any" : `${MOVE_MARK[m]} ${m}` }}
          </button>
        </span>
        <label class="best"><input type="checkbox" v-model="bestFirst" :disabled="!reference" /> Best match first</label>
      </div>
      <div v-if="!library.length" class="empty">The queue is empty: the radio is loading tracks.</div>
      <div v-for="{ track, fit } in listed" :key="track.track_id" class="item">
        <img v-if="track.cover" :src="coverUrl(track.cover, 60)" alt="" />
        <div v-else class="noart"></div>
        <div class="who">
          <div class="t">{{ track.title }}</div>
          <div class="a">{{ track.artists.join(", ") }}</div>
        </div>
        <span class="bpm">{{ track.bpm ? track.bpm.toFixed(0) : "?" }}</span>
        <span class="move" :class="fit?.move" :title="fit ? MOVE_WORDS[fit.move] : ''">{{ fit ? MOVE_MARK[fit.move] : "" }}</span>
        <span class="key" :class="fit ? `k${fit.key}` : ''" :title="fit ? `Key: ${KEY_WORDS[fit.key]}` : ''">{{ track.camelot || "—" }}</span>
        <span class="gap" :class="{ far: fit && fit.gap !== null && Math.abs(fit.gap) > 8 }" :title="fit?.gap == null ? '' : 'The pitch fader move that matches the tempo'">{{ fit?.gap == null ? "" : `${fit.gap > 0 ? "+" : ""}${fit.gap.toFixed(1)}%` }}</span>
        <span class="len">{{ clock(track.duration_seconds) }}</span>
        <span class="on">{{ loaded(track) }}</span>
        <button type="button" class="load a" @click="load('a', track)">A</button>
        <button type="button" class="load b" @click="load('b', track)">B</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.dj { height: 100vh; box-sizing: border-box; display: flex; flex-direction: column; gap: 10px; padding: 14px 16px; background: var(--bg); color: var(--text); overflow: auto; }
h4 { margin: 0; font-size: 11px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--text2); }
.top { display: flex; align-items: baseline; gap: 12px; }
.note { font-size: 11px; color: var(--text2); }
.err { padding: 6px 10px; border-radius: 6px; font-size: 12px; color: var(--red); background: color-mix(in srgb, var(--red) 12%, transparent); }
.decks { display: grid; grid-template-columns: minmax(0, 1fr) 150px minmax(0, 1fr); gap: 12px; align-items: start; }
.mixer { display: flex; flex-direction: column; gap: 14px; align-items: center; padding: 14px 10px; border-radius: 12px; background: var(--panel); box-shadow: 0 0 0 0.5px var(--panel-border); }
.drift { text-align: center; font-size: 11px; color: var(--text2); display: flex; flex-direction: column; gap: 2px; }
.drift b { font-size: 13px; font-variant-numeric: tabular-nums; color: var(--text); }
.drift.ok b { color: #30d158; }
.master, .cross { display: flex; flex-direction: column; align-items: center; gap: 4px; width: 100%; }
label { font-size: 11px; color: var(--text2); }
.vert { writing-mode: vertical-lr; direction: rtl; width: 22px; height: 130px; margin: 0; cursor: default; accent-color: var(--accent); }
.cross input[type="range"] { width: 100%; margin: 0; cursor: default; accent-color: var(--accent); }
.ab { display: flex; justify-content: space-between; width: 100%; font-size: 11px; font-weight: 700; color: var(--text2); }
.cuts { display: flex; gap: 4px; margin-top: 2px; }
.cuts button { height: 22px; padding: 0 8px; border-radius: 6px; font-size: 11px; background: var(--control); cursor: default; }
.cuts button:hover { background: var(--control-hover); }
.library { display: flex; flex-direction: column; gap: 2px; padding: 10px; border-radius: 12px; background: var(--panel); box-shadow: 0 0 0 0.5px var(--panel-border); min-height: 120px; }
.lhead { display: flex; align-items: baseline; gap: 12px; margin: 0 0 6px 4px; }
.hint { font-size: 11px; color: var(--text2); }
.moves { display: inline-flex; gap: 2px; padding: 2px; border-radius: 7px; background: var(--control); margin-left: auto; }
.moves button { height: 20px; padding: 0 8px; border-radius: 5px; font-size: 11px; cursor: default; color: var(--text2); }
.moves button.on { background: var(--seg-on); color: var(--text); box-shadow: 0 0 0 0.5px var(--panel-border); }
.moves button:disabled { opacity: 0.4; }
.move { flex: none; width: 16px; text-align: center; font-weight: 700; }
.move.lift { color: #ff9f0a; }
.move.drop { color: var(--accent); }
.move.level { color: var(--text2); }
.best { margin-left: 4px; display: inline-flex; align-items: center; gap: 5px; font-size: 11px; color: var(--text2); }
.key { border-radius: 8px; padding: 0 6px; }
.key.k0, .key.k1 { color: #fff; background: #30d158; }
.key.k1 { background: color-mix(in srgb, #30d158 70%, transparent); }
.key.k2 { color: var(--text); background: color-mix(in srgb, #ff9f0a 45%, transparent); }
.key.k3 { color: var(--text2); text-decoration: line-through; }
.gap { flex: none; width: 48px; text-align: right; font-size: 11px; color: var(--text2); font-variant-numeric: tabular-nums; }
.gap.far { color: var(--red); }
.empty { padding: 10px; color: var(--text2); }
.item { display: flex; align-items: center; gap: 10px; padding: 4px 6px; border-radius: 6px; }
.item:hover { background: var(--control); }
.item img, .noart { flex: none; width: 30px; height: 30px; border-radius: 4px; background: var(--control); }
.who { flex: 1; min-width: 0; }
.t, .a { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.t { font-weight: 500; }
.a { font-size: 11px; color: var(--text2); }
.bpm, .len { flex: none; width: 34px; text-align: right; font-variant-numeric: tabular-nums; color: var(--text2); font-size: 12px; }
.key { flex: none; min-width: 34px; text-align: center; font-size: 12px; font-variant-numeric: tabular-nums; }
.on { flex: none; width: 14px; font-weight: 700; color: var(--accent); font-size: 11px; }
.load { flex: none; width: 30px; height: 24px; border-radius: 6px; font-weight: 700; color: #fff; cursor: default; }
.load.a { background: var(--accent); }
.load.b { background: #ff9f0a; }
.load:hover { filter: brightness(1.1); }
.load:active { transform: scale(0.95); }
</style>
