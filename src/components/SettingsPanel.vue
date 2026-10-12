<script setup lang="ts">
import { dockIcon, hasDock } from "../composables/useDock";

defineProps<{ engine: "native" | "web"; active: "native" | "web" }>();
const emit = defineEmits<{ engine: ["native" | "web"] }>();
</script>

<template>
  <!-- Like the Settings window of a Mac app: a group of rows, the name on the left and the control on the right, the explanation under the group. -->
  <div class="prefs-group">
    <div class="prefs-row">
      <span>Player</span>
      <div class="segmented">
        <button :class="{ on: engine === 'native' }" @click="emit('engine', 'native')">Native</button>
        <button :class="{ on: engine === 'web' }" @click="emit('engine', 'web')">Browser</button>
      </div>
    </div>
  </div>
  <p class="prefs-note">
    Native plays through the system's audio output. Browser uses the audio of the window; the app switches
    to it by itself when the native player cannot start.
    <template v-if="engine !== active"><br /><b>Playing now: {{ active === "native" ? "native" : "browser" }}</b> (fell back).</template>
  </p>

  <template v-if="hasDock">
    <div class="prefs-group">
      <div class="prefs-row">
        <span>Show in</span>
        <div class="segmented">
          <button :class="{ on: dockIcon }" @click="dockIcon = true">Dock</button>
          <button :class="{ on: !dockIcon }" @click="dockIcon = false">Menu bar</button>
        </div>
      </div>
    </div>
    <p class="prefs-note">
      With the menu bar only, the Dock icon and the app's menu go away; the window opens from the tray icon.
    </p>
  </template>
</template>
