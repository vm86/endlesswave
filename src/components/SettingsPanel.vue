<script setup lang="ts">
import { ref } from "vue";
import { dockIcon, hasDock } from "../composables/useDock";

defineProps<{ engine: "native" | "web"; active: "native" | "web"; checking: boolean; updateStatus: string; resetting: boolean }>();
const emit = defineEmits<{ engine: ["native" | "web"]; checkUpdate: []; reset: [] }>();
const confirming = ref(false);
</script>

<template>
  <!-- Like the Settings window of a Mac app: a group of rows, the name on the left and the control on the right, the explanation under the group. -->
  <div class="prefs-group">
    <div class="prefs-row">
      <span>Player</span>
      <div class="segmented">
        <!-- What plays is lit; the one that was chosen but failed is outlined: a click tries it again. -->
        <button :class="{ on: active === 'native', wanted: engine === 'native' && active !== 'native' }" @click="emit('engine', 'native')">Native</button>
        <button :class="{ on: active === 'web', wanted: engine === 'web' && active !== 'web' }" @click="emit('engine', 'web')">Browser</button>
      </div>
    </div>
  </div>
  <p class="prefs-note">
    Native plays through the system's audio output. Browser uses the audio of the window; the app switches
    to it by itself when the native player cannot start.
    <template v-if="engine !== active"><br /><b>Native audio did not start: playing with the browser.</b> Press Native to try again.</template>
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

  <div class="prefs-group">
    <div class="prefs-row">
      <span>Updates</span>
      <button class="btn-quiet" :disabled="checking" @click="emit('checkUpdate')">Check now</button>
    </div>
  </div>
  <p class="prefs-note">{{ updateStatus || "EndlessWave looks for a new version by itself, twice a day." }}</p>

  <div class="prefs-group">
    <div class="prefs-row">
      <span>Reset</span>
      <button v-if="!confirming" class="btn-quiet danger" :disabled="resetting" @click="confirming = true">Reset…</button>
      <span v-else class="confirm">
        <button class="btn-quiet" @click="confirming = false">Cancel</button>
        <button class="btn-danger" :disabled="resetting" @click="emit('reset')">Reset</button>
      </span>
    </div>
  </div>
  <p class="prefs-note">
    <template v-if="confirming">The queue, the settings and the history of listening (with the statistics) will be cleared, and the radio
      will start again. Your sign-in and the analysis of the tracks stay.</template>
    <template v-else>Starts the radio afresh: clears the queue, the settings and the history. The sign-in and the analysis stay.</template>
  </p>
</template>
