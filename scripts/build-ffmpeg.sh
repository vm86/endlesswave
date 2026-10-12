#!/usr/bin/env bash
# Builds a small static ffmpeg for the app into ffmpeg/: the radio daemon runs
# it to decode the tracks, read tags and encode the MP3 and FLAC streams.
#
# LGPL: only ffmpeg's own code and LAME (LGPL) are in it, no --enable-gpl, no
# other libraries (--disable-autodetect), no network protocols. The sources
# are downloaded from their official sites and checked against the hashes
# below, which are those of Homebrew's formulae.
#
#   scripts/build-ffmpeg.sh        # skips the build if ffmpeg/ffmpeg exists
#   FORCE=1 scripts/build-ffmpeg.sh
set -euo pipefail

FFMPEG_VERSION=9.0.2
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
LAME_VERSION=4.0
LAME_URL="https://downloads.sourceforge.net/project/lame/lame/$LAME_VERSION/lame-$LAME_VERSION.tar.gz"
LAME_SHA256=3df5124d5ad3a98312ffd7ba6a9b36230e4f8a3e66d3ce0f425e336c32d216eb

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/build/ffmpeg"
OUT="$ROOT/ffmpeg"
JOBS="$(sysctl -n hw.ncpu 2>/dev/null || nproc)"

if [ -x "$OUT/ffmpeg" ] && [ -z "${FORCE:-}" ]; then
  echo "ffmpeg is built already: $OUT/ffmpeg (FORCE=1 to build again)"
  exit 0
fi

if command -v sha256sum >/dev/null 2>&1; then SHA256="sha256sum"; else SHA256="shasum -a 256"; fi

fetch() { # url sha256 file
  [ -f "$3" ] || curl -fL --retry 3 -o "$3" "$1"
  echo "$2  $3" | $SHA256 -c - >/dev/null || { echo "bad checksum of $3"; rm -f "$3"; exit 1; }
}

mkdir -p "$WORK" && cd "$WORK"
DEPS="$WORK/deps"
rm -rf "$DEPS" && mkdir -p "$DEPS"

fetch "$LAME_URL" "$LAME_SHA256" lame.tar.gz
fetch "$FFMPEG_URL" "$FFMPEG_SHA256" ffmpeg.tar.xz

rm -rf lame-src ffmpeg-src && mkdir lame-src ffmpeg-src
tar -xzf lame.tar.gz -C lame-src --strip-components=1
tar -xJf ffmpeg.tar.xz -C ffmpeg-src --strip-components=1

# LAME's configure wants a pkg-config, for optional things only: a stub that
# is new enough and finds no package does.
if ! command -v pkg-config >/dev/null 2>&1; then
  mkdir -p "$WORK/stub-bin"
  cat > "$WORK/stub-bin/pkg-config" <<'STUB'
#!/bin/sh
case "$1" in
  --atleast-pkgconfig-version*) exit 0 ;;
  --version) echo 1.0.0; exit 0 ;;
esac
exit 1
STUB
  chmod +x "$WORK/stub-bin/pkg-config"
  export PATH="$WORK/stub-bin:$PATH"
fi

echo "== LAME $LAME_VERSION"
(cd lame-src && ./configure --prefix="$DEPS" --disable-shared --enable-static --disable-frontend --disable-decoder >/dev/null && make -j"$JOBS" >/dev/null && make install >/dev/null)

echo "== ffmpeg $FFMPEG_VERSION"
cd ffmpeg-src
PKG_CONFIG_PATH="$DEPS/lib/pkgconfig" ./configure \
  --prefix="$WORK/install" --disable-autodetect --disable-debug --disable-doc \
  --disable-ffplay --disable-ffprobe --disable-network \
  --enable-static --disable-shared --enable-zlib --enable-libmp3lame \
  --extra-cflags="-I$DEPS/include" --extra-ldflags="-L$DEPS/lib" > "$WORK/configure.log" 2>&1 \
  || { tail -20 "$WORK/configure.log"; exit 1; }
make -j"$JOBS" > "$WORK/make.log" 2>&1 || { tail -20 "$WORK/make.log"; exit 1; }

mkdir -p "$OUT" && cp ffmpeg "$OUT/ffmpeg" && strip "$OUT/ffmpeg"
cp COPYING.LGPLv2.1 "$OUT/LICENSE.LGPL" 2>/dev/null || true
echo "built: $OUT/ffmpeg ($(du -h "$OUT/ffmpeg" | cut -f1))"
"$OUT/ffmpeg" -hide_banner -encoders 2>/dev/null | grep -E "libmp3lame|flac" || true
