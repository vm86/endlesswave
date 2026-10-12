import { invoke } from "@tauri-apps/api/core";
import { ref, watch } from "vue";

/** Only macOS has a Dock: elsewhere the option does not exist. */
export const hasDock = /Mac/i.test(navigator.platform);

/** Show the icon of the app in the Dock (off: the app lives in the menu bar only). */
export const dockIcon = ref(localStorage.getItem("dock") !== "off");

let started = false;

export function useDock() {
  if (started || !hasDock) return;
  started = true;
  const apply = (visible: boolean) => void invoke("set_dock_visible", { visible }).catch(console.warn);
  watch(dockIcon, (value) => {
    localStorage.setItem("dock", value ? "on" : "off");
    apply(value);
  });
  if (!dockIcon.value) apply(false);
}
