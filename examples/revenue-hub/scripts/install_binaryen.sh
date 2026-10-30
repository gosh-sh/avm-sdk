#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
dest="${1:-$root/.runtime}"
mkdir -p "$dest"
dest="$(cd "$dest" && pwd)"
archive="$dest/binaryen-version_133-x86_64-linux.tar.gz"
if [[ ! -f "$archive" ]]; then
  curl -fL --retry 2 --max-time 300 \
    https://github.com/WebAssembly/binaryen/releases/download/version_133/binaryen-version_133-x86_64-linux.tar.gz \
    -o "$archive"
fi
printf '%s  %s\n' 2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489 "$archive" | sha256sum -c -
tar --no-same-owner -xzf "$archive" -C "$dest" binaryen-version_133/bin/wasm-opt
"$dest/binaryen-version_133/bin/wasm-opt" --version
