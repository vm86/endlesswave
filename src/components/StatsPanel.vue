<script setup lang="ts">
import { computed, ref } from "vue";
import type { Stats, TransitionData } from "../api";

const props = defineProps<{ stats: Stats | null }>();
const scope = ref<"session" | "all_time">("session");
const data = computed<TransitionData | null>(() => props.stats?.[scope.value] ?? null);
const share = (n: number) => (data.value?.total ? `${Math.round((n / data.value.total) * 100)}%` : "–");
</script>

<template>
  <div class="mix">
    <h4>Blends
      <span class="mode" :class="{ on: scope === 'session' }" @click="scope = 'session'">session</span>
      <span class="mode" :class="{ on: scope === 'all_time' }" @click="scope = 'all_time'">all time</span>
    </h4>
    <div v-if="!data" class="dim">no cache, no history</div>
    <template v-else>
      <div class="kv"><span>total</span><span>{{ data.total }}</span></div>
      <div class="kv"><span>beat-matched</span><span>{{ data.beat_matched }} ({{ share(data.beat_matched) }})</span></div>
      <h4>Type</h4>
      <div v-for="(n, k) in data.by_type" :key="k" class="kv"><span>{{ k }}</span><span>{{ n }}</span></div>
      <h4>Key match</h4>
      <div class="kv"><span>good</span><span>{{ data.by_quality.good }}</span></div>
      <div class="kv"><span>fair</span><span>{{ data.by_quality.fair }}</span></div>
      <div class="kv"><span>poor</span><span>{{ data.by_quality.poor }}</span></div>
      <h4>Effects</h4>
      <div v-for="(n, k) in data.by_effect" :key="k" class="kv"><span>{{ k }}</span><span>{{ n }}</span></div>
    </template>
  </div>
</template>
