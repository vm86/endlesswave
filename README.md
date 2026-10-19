<h1 align="center">EndlessWave</h1>

<p align="center">
  A tray player for the endless DJ radio:<br>
  tracks from Yandex Music or your own folder, beat-matched and blended into one continuous mix.
</p>

<p align="center">
  <img src="docs/screenshots/player-light.png" width="330" alt="The player, light mode">
  &nbsp;
  <img src="docs/screenshots/player-dark.png" width="330" alt="The player, dark mode">
</p>

## What is it

**EndlessWave** takes tracks from a Yandex Music station (or from a folder of music files), matches
their tempo, swaps the bass and mids at the joins and streams the result as one endless mix with no
gaps. It plays the mix and shows what plays, what comes next and how the blends go:

- it plays the mix (MP3 or lossless FLAC) through the system's audio output;
- it lives in the tray and answers to the media keys and the system's Now Playing (Control Center on
  macOS, MPRIS on Linux);
- it runs on **macOS** (Apple Silicon) and **Linux** (x86_64), in light or dark mode as the system has it.

The radio is inside the app, and the packages hold ffmpeg: nothing else has to be installed, no `ffmpeg`.

## Features

| | |
|---|---|
| **A mix without gaps** | Beat-matched blends: bass swap, three-band EQ blend, filter sweeps, effects. Keys are chosen to sound harmonic |
| **The queue under control** | Drag the tracks to reorder them, take out the ones you do not want, play any of them at once |
| **Mood, genres, language** | Pick the mood (`calm`, `fun`...), the genres to play and the genres never to play, the language of the music |
| **Sources** | Yandex Music, VK Music and/or a local folder (`mp3`, `flac`, `m4a`, `ogg`, `opus`, `wav`...). Any of them can play at once |
| **MP3 or FLAC** | A switch for the stream: MP3 at 320 kbit/s, or lossless FLAC (16 bit) |
| **Spectrum** | The real spectrum of the mix, in time with the sound |
| **Native audio** | The app plays the stream itself, through the system's audio output (CoreAudio on macOS, PipeWire on Linux, ALSA where there is none) |
| **The system** | Tray, media keys, Now Playing with the cover, keyboard shortcuts, hide to the tray |
| **Statistics** | How many blends there were and how well tempo and key matched |

<p align="center">
  <img src="docs/screenshots/mix-dark.png" width="330" alt="Mood, language and genres">
</p>

## Install

Download a package from the [Releases](../../releases) page.

**macOS** (Apple Silicon): open the `.dmg` and drag the app to Applications. The build is not signed, so
macOS asks to confirm the first start: *System Settings → Privacy & Security → "Open Anyway"* (or
right-click the app → "Open").

**Fedora** (RPM):

```bash
sudo dnf install ./endlesswave-*.x86_64.rpm
```

**Debian / Ubuntu** (DEB):

```bash
sudo apt install ./endlesswave_*_amd64.deb
```

The sound is played by the app itself through PipeWire (ALSA is the fall back where there is no PipeWire), so nothing
else is needed. For the tray icon, GNOME needs the AppIndicator extension.

## First start and setup

Open the app: the radio service starts by itself (port `18000`, on `127.0.0.1` only).

Keyboard: `⌘R` rebuilds the queue, `⌘⇧R` (or holding the rebuild button for 3 s) clears it and loads it anew, `⌘D` likes or
unlikes the playing track; `⌘/` lists all the shortcuts.

### Equalizer

**Settings** (`⌘,`) has a ten-band equalizer (32 Hz to 16 kHz, ±12 dB): drag the points, use the arrow keys, or pick a preset. It
shapes the sound of the app's own player; other listeners of the stream (a browser, VLC) hear the radio as it is.

### Mood arc

In **Mix**, the **Mood arc** is a line of points: each point is a stage of the set, and its height is the mood, from sad
(bottom) to lively (top). Drag a point up or down to change its mood, add or remove stages with **+** and **−**, and set how
long each stage lasts (the same for all, in tracks). Switch it **on** and the radio goes through the stages one after the
other and starts over after the last. It steers the stations that follow a mood (Yandex Music, VK) and favours tracks of
the stage's mood in the queue, whatever the source. Off, the radio follows its built-in arc.

### Wave or liked tracks

In **Sources**, under each signed-in service, the row **Tracks** chooses where its tracks come from: the wave (My wave of
Yandex Music, the wave of VK) or your own liked tracks, in a random order. The choice is kept per service; when it
changes, the tracks of that service waiting in the queue make way for the new ones.

### Signing in to VK Music

In **Sources** turn on VK Music and press **sign in with VK**: a window with the VK site opens, you sign in there as in a
browser (password, code, QR), and the window closes by itself. The app takes the cookies of the session from it, swaps them
for an access token, and asks for a new token when it runs out. The window keeps nothing; the cookies are kept in the
database of the app (the file is readable by you only). You can also paste an access token instead. The radio plays the VK
wave (recommendations) by default, or your own music with the station `favorites`. The heart in the player adds a track to
your VK music or takes it out. This uses the unofficial API of the VK site, so it may stop working when VK changes it.

### Signing in to Yandex Music

If you have not signed in yet, a card "Sign in to Yandex Music" shows at the top:

1. Press **Copy code** (the code goes to the clipboard) and **Open**: the Yandex page opens.
2. Paste the code and confirm the sign-in.

The token is kept in the database of the radio, `~/.cache/endlesswave.db` (mode `0600`), so you sign in once.

### Music from a folder

Open **Sources** → turn **Local folder** on → **Choose…** → pick the folder (or just drag the folder from
Finder onto the window). Subfolders are read too, and
the tags (title, artist, genre) come from the files. Yandex Music can stay on or be turned off: the tracks
of both sources go into one pool.

> Local tracks have no covers and no moods: the mood is judged by the sound. The language setting works
> for Yandex Music only.

### Mood, language, genres

The **Mix** tab:

- **Mood**: pick one or several. None picked means any;
- **Language**: the language of the music (Yandex Music);
- **Genres**: the **pick** mode chooses the genres to play, **block** the ones never to play. The list of
  genres grows as tracks are played;


A choice takes effect at once: the queue is built again for the new settings. You can rebuild it by hand
with the **⟳** button (⌘R). The settings are remembered and come back after a restart.

### Settings

The **Settings** sheet is the gear in the top bar (<kbd>⌘</kbd> <kbd>,</kbd>). The sound is played by the app's own player (the
system's audio output); if it cannot start (no audio device, for instance), the app says so.

On macOS, **Appearance** can hide the Dock icon and the app's menu: the player then lives in the menu bar
only, and the window opens from the tray icon.

**Reset** starts the radio afresh: it clears the queue, the settings of the mix, the sources and the app, and the
history of listening (with the statistics). The sign-in and the cache of the analysis of the tracks stay.

**Updates**: the app looks for a new version of itself twice a day and shows a card with a **Download**
button (it opens the page of the release). **Check now** (or *Check for Updates…* in the app menu on macOS)
asks at once.

## Using it

<table>
<tr>
<td><b>Buttons</b></td>
<td>▶/⏸ play and pause · ■ stop · ⏭ next track · ⟳ rebuild the queue · <b>MP3/FLAC</b> the stream format · the volume slider</td>
</tr>
<tr>
<td><b>Next track</b></td>
<td>The blend starts on the next bar line and is as smooth as any other. The button stays pressed until the new track is heard</td>
</tr>
<tr>
<td><b>The queue</b></td>
<td><b>Drag</b> a track with the mouse to move it. Hover a row: <b>▶</b> plays it now (or double-click), <b>✕</b> takes it out of the queue. The track being blended in just now is fixed</td>
</tr>
<tr>
<td><b>Tray</b></td>
<td>The menu: what plays, pause, next, show the window, quit. Closing the window only hides it to the tray; <b>Quit</b> stops the app and the service</td>
</tr>
<tr>
<td><b>System</b></td>
<td>Media keys and Now Playing (Control Center on macOS, MPRIS and <code>playerctl</code> on Linux)</td>
</tr>
</table>

### Keyboard shortcuts

On macOS the shortcuts are in the menu bar (**Controls** and **View**), so you can find them there. On Linux, Ctrl takes the place of ⌘. The whole list is shown by <kbd>⌘</kbd> <kbd>/</kbd>.

| Key | Action |
|---|---|
| <kbd>Space</kbd> | Play / pause |
| <kbd>⌘</kbd> <kbd>.</kbd> | Stop |
| <kbd>⌘</kbd> <kbd>→</kbd> | Next track |
| <kbd>⌘</kbd> <kbd>R</kbd> | Rebuild the queue |
| <kbd>⌘</kbd> <kbd>L</kbd> | MP3 ⇄ FLAC |
| <kbd>⌘</kbd> <kbd>↑</kbd> / <kbd>↓</kbd> | Volume |
| <kbd>⌘</kbd> <kbd>1</kbd>–<kbd>4</kbd> | Tabs: Playlist, Mix, Sources, Stats |
| <kbd>⌘</kbd> <kbd>,</kbd> | Settings |
| <kbd>⌘</kbd> <kbd>/</kbd> | The list of shortcuts |

## How it works

```
┌──────────────────────────┐   HTTP API, SSE, /stream    ┌──────────────────────────┐
│  EndlessWave             │ ◄─────────────────────────► │  the radio (radio-core)  │
│  Tauri + Vue 3           │      127.0.0.1:18000        │  Rust, on its own thread │
│  window, tray, sound     │                             │  ├─ MP3, FLAC, Ogg built │
└──────────────────────────┘                             │  └─ Yandex Music / folder│
                                                         └──────────────────────────┘
```

The app reads the stream itself, decodes it (MP3, FLAC) and plays it through the system's audio output;
the window only controls it.
Everything else (the queue, the moods, the spectrum) comes from the API of the radio. The radio runs inside the app (`crates/radio-core`); if something already answers on port `18000`, the app connects to it.

Files of the radio (versions before 0.2 kept them as `~/.config/yamusic-radio.json`, `~/.cache/yamusic-radio.db` and so on; the first start moves the database to the new name, takes the token file into it and removes the old files, the loaded tracks and the log):

| What | Where |
|---|---|
| The Yandex Music token and its settings, the cache of the analysis, the settings of the radio | `~/.cache/endlesswave.db` (mode `0600`) |
| The loaded tracks that wait for their turn | `~/.cache/endlesswave-tracks/` |
| The log (only with `ENDLESSWAVE_LOG=1`) | `~/.cache/endlesswave.log` |

Environment: `ENDLESSWAVE_PORT` (the port, 18000 by default), `ENDLESSWAVE_ARGS` (any more flags of
the radio, e.g. `--no-keep-tracks`; `endlesswave-radio --help` in `crates/radio-core` lists them), `ENDLESSWAVE_LOG=1` (write the log to a file; it is not written otherwise), `ENDLESSWAVE_DEBUG=1` (a detailed log, to the file too).

## Troubleshooting

- **No sound, the queue is empty.** The service is waiting for tracks. The log is not written by default.
  Start the app with the log on, then read it: on macOS
  `open -n --env ENDLESSWAVE_LOG=1 -a "EndlessWave"`, on Linux
  `ENDLESSWAVE_LOG=1 endlesswave`, and `tail -f ~/.cache/endlesswave.log`.
  `Unknown HTTPError` in it means Yandex Music does not answer or limits the requests.
- **No tray icon on Linux.** AppIndicator is needed (in GNOME, the *AppIndicator and
  KStatusNotifierItem Support* extension). The tray menu still works, a click on the icon does not open
  the window.
- **No sound on Linux.** The native player needs a working audio output (PipeWire, or ALSA).

## Development

You need Node.js 22+ and Rust (stable). The radio is in `crates/radio-core` (see `docs/rust-port.md`); for the keys and the loudness the way the original radio found them build with `--features essentia` after `crates/radio-core/tools/fetch-vendor.sh`.

```bash
npm install
npm run tauri dev      # the window, with hot reload
```

Type check: `npx vue-tsc --noEmit`. The radio's tests: `cargo test` in `crates/radio-core`.

The app icon is drawn in `assets/icon.svg` (`python3 assets/make-icon.py`, then `python3 assets/render-icon.py` for `assets/icon.png`, then
`npx tauri icon assets/icon.png` makes all the sizes); the menu bar icon of macOS is
`src-tauri/icons/tray.png` (`assets/tray.svg`).

Layout: `src/` is the interface (Vue 3, Vuetify), `src-tauri/` the shell (starts the radio, the tray, the
window), `crates/radio-core/` the radio, `scripts/` builds ffmpeg and the packages.

## Build

The packages hold a static LGPL ffmpeg (it decodes what the radio can not: HE-AAC, Opus); the radio is built into the app.

| Platform | Result | Command |
|---|---|---|
| macOS arm64 | `.app` / `.dmg` | `npm run bundle` on a Mac |
| Linux x86_64 | `.rpm`, `.deb` | `npm run bundle:linux` (in a podman container) |

## Limits

- The macOS build is not signed or notarized (that needs a Developer ID certificate).
- Apple Silicon only on macOS, x86_64 only on Linux.
- This is a radio: there is no previous track and no seeking.
- On Linux the window frame of the system is drawn together with the app's own top bar.

## License

The player is released under the [GNU Affero General Public License, version 3 or later](LICENSE). The source
is this repository; the radio was ported from the author's Python radio, whose answers are kept as test data in `crates/radio-core/tests/golden`.
Versions up to 0.1.0 were released under the MIT License.

### Third-party parts

The full texts of all of them (the Rust crates and the JavaScript packages too) are in `THIRD-PARTY-LICENSES` next to the app in the package; `scripts/third-party.sh` makes it.

- **ffmpeg 9.0.2** (LGPL-2.1+) and **LAME 4.0** (LGPL): built without any GPL parts; the license text is
  next to the binary in the package.
- **Essentia** (AGPL-3.0): built into the app from source (`crates/radio-core/vendor`, fetched by
  `tools/fetch-vendor.sh`), finds the keys and the loudness of the tracks. The packages that hold it can only be
  passed on under the AGPL-3.0, which is why the player has this license.
- **LAME** (LGPL), **flacenc** (Apache-2.0), **symphonia** (MPL-2.0), **rubato** (MIT): the MP3 and FLAC encoders and
  the decoders of the radio.
