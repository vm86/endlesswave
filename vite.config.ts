import vue from "@vitejs/plugin-vue";
import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// The version of the app, for the check of new releases (the release build puts the tag into package.json).
const { version } = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")) as { version: string };

export default defineConfig(() => ({
  plugins: [vue()],
  define: { __APP_VERSION__: JSON.stringify(version) },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
}));
