#!/usr/bin/env bash
# Runs inside the builder container (see scripts/build-linux.sh): the project
# is mounted read-only at /src, the work is done in /work (a volume, so a
# failed build resumes), the packages go to /out.
set -euo pipefail

mkdir -p /work /out
cd /work
# Copy the sources, but not what was built on another system.
tar -C /src \
  --exclude=./node_modules --exclude=./build --exclude=./backend --exclude=./ffmpeg \
  --exclude=./dist --exclude=./dist-linux --exclude=./src-tauri/target --exclude=./src-tauri/gen \
  --exclude=./.git -cf - . | tar -xf -

npm ci
scripts/build-backend.sh
scripts/build-ffmpeg.sh

# The build runs in emulation (x86_64 on an ARM Mac): the cheaper release
# profile, or the linking takes hours.
export CARGO_PROFILE_RELEASE_LTO=off
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
npx tauri build --bundles deb,rpm --config src-tauri/tauri.bundle.conf.json

cp -v src-tauri/target/release/bundle/deb/*.deb src-tauri/target/release/bundle/rpm/*.rpm /out/
