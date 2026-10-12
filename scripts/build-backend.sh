#!/usr/bin/env bash
# Builds the radio daemon (yamusic-dj) from its repository into backend/, to be
# bundled into the app: one folder with the executable and what it needs
# (PyInstaller "onedir": it starts at once, a single file unpacks itself every run).
#
#   scripts/build-backend.sh            # the main branch of vm86/yamusic-radio
#   YAMUSIC_RADIO_REF=v0.2.0 scripts/build-backend.sh
#   YAMUSIC_RADIO_REPO=file://$HOME/devel/yamusic-radio scripts/build-backend.sh   # a local checkout
#
# Needs python3 (3.12+), git and the network (GitHub, PyPI). ffmpeg with
# libmp3lame is NOT bundled: the app finds it in PATH, /opt/homebrew/bin or /usr/local/bin.
set -euo pipefail

REPO="${YAMUSIC_RADIO_REPO:-https://github.com/vm86/yamusic-radio.git}"
REF="${YAMUSIC_RADIO_REF:-main}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/build/backend"
OUT="$ROOT/backend"

mkdir -p "$WORK"
# REF is a branch, a tag or a commit.
if [ ! -d "$WORK/src/.git" ]; then
  git init -q "$WORK/src"
  git -C "$WORK/src" remote add origin "$REPO"
fi
# YAMUSIC_RADIO_REPO may change between runs (a local checkout, a fork).
git -C "$WORK/src" remote set-url origin "$REPO"
git -C "$WORK/src" fetch -q --depth 1 origin "$REF"
git -C "$WORK/src" checkout -q --force FETCH_HEAD
echo "backend: $(git -C "$WORK/src" log --oneline -1)"

[ -d "$WORK/venv" ] || python3 -m venv "$WORK/venv"
PIP="$WORK/venv/bin/pip"
# The checkout has no tags: the version is pretended.
export SETUPTOOLS_SCM_PRETEND_VERSION="${YAMUSIC_RADIO_VERSION:-0.1.0}"
"$PIP" install -q --upgrade pyinstaller
collect=()
# Essentia finds the keys better; without a wheel for this machine the daemon
# falls back to a simpler detection.
if "$PIP" install -q --upgrade "$WORK/src[key]"; then
  collect=(--collect-all essentia)
else
  "$PIP" install -q --upgrade "$WORK/src"
fi

printf 'from yamusic_radio.main import main\n\nmain()\n' > "$WORK/launcher.py"
"$WORK/venv/bin/pyinstaller" --noconfirm --clean --onedir --name yamusic-dj \
  --distpath "$WORK/dist" --workpath "$WORK/work" --specpath "$WORK" \
  --collect-submodules yandex_music --collect-data yamusic_radio \
  "${collect[@]}" "$WORK/launcher.py" > "$WORK/pyinstaller.log" 2>&1 \
  || { tail -20 "$WORK/pyinstaller.log"; exit 1; }

rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$WORK/dist/yamusic-dj/." "$OUT/"
echo "built: $OUT/yamusic-dj ($(du -sh "$OUT" | cut -f1))"
