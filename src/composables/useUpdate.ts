import { openUrl } from "@tauri-apps/plugin-opener";
import { fetch } from "@tauri-apps/plugin-http";
import { onMounted, onUnmounted, ref } from "vue";

const REPO = "vm86/endlesswave";
const FIRST_CHECK_MS = 8_000; // let the app start first
const CHECK_EVERY_MS = 12 * 60 * 60 * 1000;
const SKIPPED = "update-skipped";

export interface Release {
  version: string;
  url: string;
}

/** 1.2.3-rc1 → [1, 2, 3] and "rc1" (empty for a final release). */
function parse(version: string): { numbers: number[]; pre: string } | null {
  const match = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/.exec(version.trim());
  return match ? { numbers: [Number(match[1]), Number(match[2]), Number(match[3])], pre: match[4] ?? "" } : null;
}

/** Whether `latest` is newer than `current`. A release comes after its own release candidates. */
export function isNewer(latest: string, current: string): boolean {
  const a = parse(latest);
  const b = parse(current);
  if (!a || !b) return false;
  for (let i = 0; i < 3; i++) {
    if (a.numbers[i] !== b.numbers[i]) return (a.numbers[i] as number) > (b.numbers[i] as number);
  }
  if (a.pre === b.pre) return false;
  if (!a.pre) return true;
  if (!b.pre) return false;
  return a.pre.localeCompare(b.pre, undefined, { numeric: true }) > 0;
}

/**
 * Tells about a new release of the app: asks GitHub a few seconds after the start and twice a day.
 * Whoever runs a release candidate hears of candidates too; the others only of the final releases.
 * A failure (no network, no such repository yet) is silent: this is only a hint.
 */
export function useUpdate() {
  const release = ref<Release | null>(null);
  // For the button of the settings and the menu item: what the last check found, in words.
  const checking = ref(false);
  const status = ref("");
  let timers: number[] = [];

  /** `asked`: the user asked for it, so a release once put off is told of again, and the answer is given. */
  async function check(asked = false) {
    if (checking.value) return;
    checking.value = true;
    status.value = asked ? "Checking…" : "";
    try {
      const response = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=10`, {
        headers: { Accept: "application/vnd.github+json", "User-Agent": "EndlessWave" },
      });
      if (!response.ok) {
        if (asked) status.value = "Cannot reach GitHub";
        return;
      }
      const list = (await response.json()) as Array<{ tag_name: string; html_url: string; draft: boolean; prerelease: boolean }>;
      const candidates = !!parse(__APP_VERSION__)?.pre;
      const newest = list
        .filter((item) => !item.draft && (candidates || !item.prerelease))
        .filter((item) => isNewer(item.tag_name, __APP_VERSION__))
        .sort((x, y) => (isNewer(x.tag_name, y.tag_name) ? -1 : 1))[0];
      if (!newest) {
        if (asked) status.value = `EndlessWave ${__APP_VERSION__} is up to date`;
        return;
      }
      if (!asked && localStorage.getItem(SKIPPED) === newest.tag_name) return;
      release.value = { version: newest.tag_name.replace(/^v/, ""), url: newest.html_url };
      if (asked) status.value = `Version ${release.value.version} is available`;
    } catch (error) {
      console.warn("the check for a new release failed", error);
      if (asked) status.value = "Cannot reach GitHub";
    } finally {
      checking.value = false;
    }
  }

  /** The button: the page of the release, where the packages are. */
  async function download() {
    if (release.value) await openUrl(release.value.url);
    release.value = null;
  }

  /** The cross: no word of this release again. */
  function dismiss() {
    if (release.value) {
      try {
        localStorage.setItem(SKIPPED, `v${release.value.version}`);
      } catch {
        /* the next start will tell again */
      }
    }
    release.value = null;
  }

  onMounted(() => {
    timers = [window.setTimeout(() => void check(), FIRST_CHECK_MS), window.setInterval(() => void check(), CHECK_EVERY_MS)];
  });
  onUnmounted(() => timers.forEach(clearTimeout));

  return { release, checking, status, check, download, dismiss };
}
