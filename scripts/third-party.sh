#!/usr/bin/env bash
# Makes THIRD-PARTY-LICENSES: the licenses of what the packages hold besides the app's own code.
#   Essentia, Eigen, LAME, ffmpeg (native, by hand), the Rust crates (cargo-about), the JavaScript packages.
#
#   cargo install cargo-about --features cli     # once
#   crates/radio-core/tools/fetch-vendor.sh      # the sources of Essentia and Eigen, for their texts
#   scripts/build-ffmpeg.sh                      # ffmpeg/LICENSE.LGPL is its text
#   scripts/third-party.sh                       # -> THIRD-PARTY-LICENSES
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=THIRD-PARTY-LICENSES
section() { # title file
  printf '================================================================================\n%s\n\n' "$1"
  if [ -f "$2" ]; then cat "$2"; else echo "(the text is not here: $2 is missing, see the project's own repository)"; fi
  printf '\n'
}

lame=$(cargo metadata --manifest-path crates/radio-core/Cargo.toml --format-version 1 2>/dev/null \
  | python3 -c "import json,sys; print(next(p['manifest_path'] for p in json.load(sys.stdin)['packages'] if p['name']=='mp3lame-sys'))")
lame_dir=$(dirname "$lame")/lame-3.100

{
  cat <<'EOF'
EndlessWave holds the parts below besides its own code (AGPL-3.0-or-later, see LICENSE).
Where a license asks for the source, it is: https://github.com/vm86/endlesswave (the app and the radio),
the sources of Essentia, Eigen and LAME are fetched/built by the files named in README.md.

Native parts
EOF
  section "Essentia (AGPL-3.0-or-later): the keys and the loudness of the tracks, built into the app" crates/radio-core/vendor/essentia/COPYING.txt
  section "Eigen (MPL-2.0): headers used by Essentia" crates/radio-core/vendor/eigen/COPYING.MPL2
  section "LAME 3.100 (LGPL-2.0-or-later): the MP3 encoder, via the crate mp3lame-sys" "$lame_dir/COPYING"
  section "FFmpeg 9.0.2 (LGPL-2.1-or-later): a static build without any GPL parts, decodes what the radio can not" ffmpeg/LICENSE.LGPL
  (cd src-tauri && cargo about generate about.hbs --features essentia 2>/dev/null)
  echo
  node scripts/third-party.mjs
} > "$OUT"
echo "$OUT: $(wc -l < "$OUT") lines"
