<script setup lang="ts">
import { computed } from "vue";
import { keyFit, MAJOR, MINOR } from "../camelot";

/** The wheel of Camelot with the rules of mixing by it. With a key (the deck that is heard) the wheel shows what goes with it. */
const props = defineProps<{ reference: string | null }>();

const C = 110;
const RINGS = { B: [70, 104], A: [36, 66] } as const;
const point = (radius: number, turn: number) => {
  const angle = ((turn * 30 - 90) * Math.PI) / 180;
  return [C + radius * Math.cos(angle), C + radius * Math.sin(angle)] as const;
};
/** The cell of a key: the ring, 12 at the top, 1 a step to the right. */
function cell(number: number, ring: "A" | "B") {
  const [inner, outer] = RINGS[ring];
  const [from, to] = [(number % 12) - 0.5, (number % 12) + 0.5];
  const [x1, y1] = point(outer, from);
  const [x2, y2] = point(outer, to);
  const [x3, y3] = point(inner, to);
  const [x4, y4] = point(inner, from);
  const [mx, my] = point((inner + outer) / 2, number % 12);
  return { d: `M${x1} ${y1} A${outer} ${outer} 0 0 1 ${x2} ${y2} L${x3} ${y3} A${inner} ${inner} 0 0 0 ${x4} ${y4} Z`, mx, my };
}
const cells = computed(() =>
  Array.from({ length: 12 }, (_, i) => i + 1).flatMap((number) =>
    (["B", "A"] as const).map((ring) => {
      const code = `${number}${ring}`;
      return { code, note: ring === "A" ? MINOR[number - 1]! : MAJOR[number - 1]!, ...cell(number, ring), fit: props.reference ? keyFit(props.reference, code) : null, own: code === props.reference };
    }),
  ),
);

const RULES = [
  { mark: "k0", head: "Same number and letter", text: "The same key: the blend is seamless. Safe at any length." },
  { mark: "k1", head: "One step round the wheel (±1, the same letter)", text: "+1 lifts the energy a little, −1 lets it down. The classic move." },
  { mark: "k1", head: "The same number, the other letter (A↔B)", text: "The relative major or minor: the same notes, another mood." },
  { mark: "k2", head: "Two steps (±2, the same letter)", text: "A bolder step: the energy jumps. Keep the blend short." },
  { mark: "k3", head: "Anything else", text: "The keys clash. Cut on a drop, over a beat without melody, or do not." },
];
</script>

<template>
  <div class="cheat">
    <svg viewBox="0 0 220 220" role="img" aria-label="The wheel of Camelot" class="wheel">
      <g v-for="c in cells" :key="c.code">
        <path :d="c.d" :class="['cell', c.fit === null ? '' : `k${c.fit}`, { own: c.own }]" />
        <text :x="c.mx" :y="c.my - 1" class="cell-code">{{ c.code }}</text>
        <text :x="c.mx" :y="c.my + 8" class="note">{{ c.note }}</text>
      </g>
      <text :x="C" :y="C - 2" class="mid">outer B</text>
      <text :x="C" :y="C + 8" class="mid">inner A</text>
    </svg>
    <div class="rules">
      <p v-if="reference" class="now-line">With <b>{{ reference }}</b> (the deck that is heard) go the cells that are lit.</p>
      <p v-else class="now-line">Load a track into a deck and the cells that go with it light up.</p>
      <ul>
        <li v-for="rule in RULES" :key="rule.head">
          <i :class="rule.mark"></i>
          <div><b>{{ rule.head }}</b><span>{{ rule.text }}</span></div>
        </li>
      </ul>
      <p class="tip">A is minor (the inner ring), B is major (the outer ring). The wheel is a circle: 12 is next to 1. The tempo goes along: within about 8% the pitch fader hides the difference; a rise of energy is best made by +1 and by the tempo going up.</p>
    </div>
  </div>
</template>

<style scoped>
.cheat { display: flex; gap: 18px; align-items: flex-start; padding: 10px 6px 4px; }
.wheel { flex: none; width: 250px; height: 250px; }
.cell { fill: var(--control); stroke: var(--panel); stroke-width: 1.5; transition: fill 150ms var(--ease); }
.cell.k0 { fill: var(--accent); }
.cell.k1 { fill: #30d158; }
.cell.k2 { fill: color-mix(in srgb, #ff9f0a 70%, transparent); }
.cell.k3 { fill: var(--control); opacity: 0.55; }
.cell.own { stroke: var(--text); stroke-width: 2; }
.cell-code { font-size: 9.5px; font-weight: 700; text-anchor: middle; fill: var(--text); pointer-events: none; }
.note { font-size: 7px; text-anchor: middle; fill: var(--text2); pointer-events: none; }
.cell.k0 + .cell-code, .cell.k1 + .cell-code { fill: #fff; }
.cell.k0 ~ .note, .cell.k1 ~ .note { fill: rgba(255, 255, 255, 0.85); }
.mid { font-size: 7.5px; text-anchor: middle; fill: var(--text2); }
.rules { min-width: 0; flex: 1; font-size: 12px; }
.now-line { margin: 0 0 8px; color: var(--text2); }
ul { margin: 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 7px; }
li { display: flex; gap: 8px; align-items: flex-start; }
li i { flex: none; width: 10px; height: 10px; margin-top: 3px; border-radius: 3px; background: var(--control); }
li i.k0 { background: var(--accent); }
li i.k1 { background: #30d158; }
li i.k2 { background: #ff9f0a; }
li i.k3 { background: var(--text2); opacity: 0.5; }
li b { display: block; font-weight: 600; }
li span { color: var(--text2); }
.tip { margin: 10px 0 0; color: var(--text2); font-size: 11px; }
</style>
