# Yamusic Radio Player

A tray player for [yamusic-radio](https://github.com/vm86/yamusic-radio), the endless
beat-matched DJ radio (Yandex Music or a local folder). Tauri v2, Vue 3, macOS and Linux.

```bash
npm install
npm run tauri dev      # the daemon is started from ../yamusic-radio/.venv, or YAMUSIC_DJ_BIN
```

Environment: `YAMUSIC_RADIO_PORT` (default 18000), `YAMUSIC_RADIO_ARGS` (more flags
for the daemon), `YAMUSIC_RADIO_DEBUG=1`. The daemon's log is `~/.cache/yamusic_radio.log`.

## Packages and releases

The app ships with the radio daemon (`yamusic-dj`, built from
[vm86/yamusic-radio](https://github.com/vm86/yamusic-radio)) and a static LGPL
ffmpeg, so nothing else has to be installed.

| Where | What | How |
|---|---|---|
| macOS arm64 | `.dmg` | `npm run bundle` (on a Mac) |
| Linux x86_64 | `.rpm`, `.deb` | `npm run bundle:linux` (in a podman container; slow on an ARM Mac, it is emulated) |

**GitHub Actions** (`.github/workflows/release.yml`) builds both on every pull
request and manual run (the packages are kept as artifacts). **A release is
made only by pushing a tag**:

```bash
git tag v1.2.3 && git push origin v1.2.3
```

The tag sets the version of the app (`scripts/set-version.py`); a tag with a
suffix (`v1.2.3-rc1`) makes a pre-release. The release gets the `.dmg`, `.rpm`,
`.deb` and `SHA256SUMS`, and the notes name the commit of the daemon that went
in. The daemon is taken from the `main` branch of its repository, or from the
repository variable `YAMUSIC_RADIO_REF` (a branch, a tag or a commit): set it to
a tag to make a release repeatable.

The macOS build is not signed or notarized (Gatekeeper asks to confirm the first
start); signing needs a Developer ID certificate in the repository secrets.
