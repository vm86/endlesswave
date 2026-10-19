<script setup lang="ts">
import EqualizerEditor from "./components/EqualizerEditor.vue";
</script>

<template>
  <div class="eq-window">
    <h4>Equalizer</h4>
    <EqualizerEditor />
    <p class="prefs-note">
      Drag a point up or down, or use the arrow keys (⇧ for 3 dB, 0 for flat); double-click a point for 0 dB. It shapes the sound of this
      app's player; the stream for other players is not touched.
    </p>
  </div>
</template>

<style scoped>
.eq-window { height: 100vh; box-sizing: border-box; padding: 14px 18px; background: var(--bg); color: var(--text); overflow: auto; }
.eq-window h4 { margin: 0 0 8px; font-size: 11px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--text2); }
</style>
