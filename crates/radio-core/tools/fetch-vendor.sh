#!/usr/bin/env bash
# Downloads what the feature `essentia` is built from, into vendor/ (not in git):
#   Essentia   https://github.com/MTG/essentia   (AGPL-3.0), the commit below, about 90 MB
#   Eigen      https://gitlab.com/libeigen/eigen (MPL-2.0, headers only), about 2 MB
# Then:  cargo test --features essentia
set -euo pipefail
cd "$(dirname "$0")/.."

ESSENTIA_COMMIT=dc91e47ed634e0b152d3d0229ec86751bb315473 # the master the golden files were made with
EIGEN_VERSION=3.3.7                                       # the one Essentia names in packaging/build_config.sh

fetch() { # name url
  local name=$1 url=$2
  if [ -e "vendor/$name" ]; then echo "vendor/$name is there"; return; fi
  mkdir -p vendor/dl
  curl -fL --retry 5 -C - -o "vendor/dl/$name.tar.gz" "$url"
  gzip -t "vendor/dl/$name.tar.gz"
  mkdir "vendor/$name"
  tar -xzf "vendor/dl/$name.tar.gz" -C "vendor/$name" --strip-components=1
}

fetch essentia "https://codeload.github.com/MTG/essentia/tar.gz/$ESSENTIA_COMMIT"
fetch eigen "https://gitlab.com/libeigen/eigen/-/archive/$EIGEN_VERSION/eigen-$EIGEN_VERSION.tar.gz"
