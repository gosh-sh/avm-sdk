#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
runtime_dir="${1:-$root/.runtime}"
tag="avm-dev-335a0f09fdbc-sdk-0cbca40fad9a"
expected="2171ee865bc9fc2e1a1274dfdd0f4330dc362b1f49d38c0a35c4de41aa61d6f3"
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
    echo "The published AVM stand requires Linux x86-64 (Windows: use WSL2)." >&2
    exit 2
fi
mkdir -p "$runtime_dir"
runtime_dir="$(cd "$runtime_dir" && pwd)"
archive="$runtime_dir/avm-sdk-linux-x86_64.tar.gz"
if [[ ! -f "$archive" ]]; then
    curl -fL --retry 2 --max-time 180 \
        "https://github.com/gosh-sh/avm-sdk/releases/download/$tag/avm-sdk-linux-x86_64.tar.gz" \
        -o "$archive"
fi
printf '%s  %s\n' "$expected" "$archive" | sha256sum -c -
tar --no-same-owner -xzf "$archive" -C "$runtime_dir"
(cd "$runtime_dir/avm-sdk-linux-x86_64" && sha256sum -c SHA256SUMS)
echo "AVM SDK ready: $runtime_dir/avm-sdk-linux-x86_64"
