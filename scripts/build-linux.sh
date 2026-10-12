#!/usr/bin/env bash
# Builds the Linux packages (x86_64, RPM and DEB) in a container, into dist-linux/.
# Needs podman (or docker as `docker`) and the network. On an ARM Mac the x86_64
# build is emulated: it takes an hour or more; the work is kept in a volume.
#
#   scripts/build-linux.sh
#   PLATFORM=linux/arm64 scripts/build-linux.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PLATFORM="${PLATFORM:-linux/amd64}"
ENGINE="${ENGINE:-podman}"
IMAGE="endlesswave-builder"
VOLUME="yamusic-build-work-${PLATFORM#linux/}"

"$ENGINE" build --platform "$PLATFORM" -t "$IMAGE" -f "$ROOT/scripts/linux/Containerfile" "$ROOT/scripts/linux"
mkdir -p "$ROOT/dist-linux"
"$ENGINE" run --rm --platform "$PLATFORM" \
  -v "$ROOT":/src:ro -v "$VOLUME":/work -v "$ROOT/dist-linux":/out \
  "$IMAGE" /src/scripts/linux/build-in-container.sh
echo "packages: $ROOT/dist-linux"
