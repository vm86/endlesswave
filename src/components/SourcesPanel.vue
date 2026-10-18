<script setup lang="ts">
import { mdiFolderOutline } from "@mdi/js";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { reactive } from "vue";
import type { AuthInfo, Backend } from "../api";

defineProps<{ auth: AuthInfo | null }>();
const emit = defineEmits<{ toggle: [id: string, enabled: boolean]; signIn: [id: string, values: Record<string, string>]; setting: [id: string, values: Record<string, string>]; copy: [text: string, what?: string] }>();
const values = reactive<Record<string, Record<string, string>>>({});

const status = (b: Backend) =>
  !b.enabled ? "off" : !b.authorized ? (folderField(b) ? "choose a folder" : "sign-in needed") : b.available ? "ready" : "resting after failures";

/** A backend whose setting is a folder (the local one) picks it in the system's dialog. */
const folderField = (b: Backend) => b.setting_fields?.find((f) => f.name === "directory");

/** The settings of a backend that are a choice (the quality of Yandex Music): a row for each. */
const choiceFields = (b: Backend) => b.setting_fields?.filter((f) => f.choices?.length) ?? [];

async function chooseFolder(b: Backend) {
  const field = folderField(b);
  if (!field) return;
  const chosen = await open({ directory: true, multiple: false, title: field.label, defaultPath: b.settings?.[field.name] || undefined });
  if (typeof chosen === "string") emit("signIn", b.id, { [field.name]: chosen });
}
</script>

<template>
  <div class="mix">
    <h4>Music sources</h4>
    <div v-for="b in auth?.backends" :key="b.id" class="source">
      <div class="head">
        <span class="chip" :class="{ sel: b.enabled }" @click="emit('toggle', b.id, !b.enabled)">{{ b.name }}</span>
        <span class="dim">{{ status(b) }}</span>
      </div>

      <div v-if="b.enabled && folderField(b)" class="folder">
        <v-icon :icon="mdiFolderOutline" size="16" />
        <span class="path" :title="b.settings?.directory">{{ b.settings?.directory || "no folder chosen" }}</span>
        <button class="btn-accent" @click="chooseFolder(b)">{{ b.settings?.directory ? "Change…" : "Choose…" }}</button>
      </div>

      <div v-if="b.enabled && b.authorized" class="choices">
        <div v-for="f in choiceFields(b)" :key="f.name" class="prefs-row">
          <span>{{ f.label }}</span>
          <div class="segmented">
            <button v-for="c in f.choices" :key="c.value" :class="{ on: b.settings?.[f.name] === c.value }"
              @click="emit('setting', b.id, { [f.name]: c.value })">{{ c.label }}</button>
          </div>
        </div>
      </div>

      <div v-else-if="b.enabled && !b.authorized && !folderField(b)" class="login">
        <template v-if="b.url && b.code">
          code <code class="code" title="Click to copy" @click="emit('copy', b.code)">{{ b.code }}</code>
          <a href="#" @click.prevent="emit('copy', b.code)">copy</a> ·
          <a href="#" @click.prevent="emit('copy', b.url, 'Link')">copy link</a> ·
          <a href="#" @click.prevent="openUrl(b.url)">open the page</a>
        </template>
        <form v-else-if="b.fields.length" @submit.prevent="emit('signIn', b.id, values[b.id] ?? {})">
          <input v-for="f in b.fields" :key="f.name" v-model="(values[b.id] ??= {})[f.name]" :placeholder="f.label"
            :type="f.secret ? 'password' : 'text'" />
          <button type="submit">sign in</button>
        </form>
        <span v-else class="dim">waiting for the service…</span>
      </div>
    </div>
    <div v-if="!auth?.backends.length" class="dim">no sources</div>
  </div>
</template>
