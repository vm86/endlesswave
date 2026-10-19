<script setup lang="ts">
import { openUrl } from "@tauri-apps/plugin-opener";
import { ref } from "vue";
import { dockIcon, hasDock } from "../composables/useDock";
import EqualizerEditor from "./EqualizerEditor.vue";

defineProps<{ checking: boolean; updateStatus: string; resetting: boolean }>();
const emit = defineEmits<{ checkUpdate: []; reset: [] }>();
const confirming = ref(false);
const SOURCE = "https://github.com/vm86/endlesswave";
</script>

<template>
  <!-- Like the Settings window of a Mac app: a group of rows, the name on the left and the control on the right, the explanation under the group. -->
  <div class="prefs-group eq-group">
    <div class="prefs-row"><span>Equalizer</span></div>
    <EqualizerEditor />
  </div>
  <p class="prefs-note">Drag a point up or down (or use the arrow keys, double-click for 0 dB). It shapes the sound of this app only.</p>

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

  <div class="prefs-group">
    <div class="prefs-row">
      <span>Source code</span>
      <button class="btn-quiet" @click="openUrl(SOURCE)">Open</button>
    </div>
  </div>
  <p class="prefs-note">EndlessWave is free software under the GNU Affero General Public License, version 3 or later: the source of this version is at the link.</p>
</template>
