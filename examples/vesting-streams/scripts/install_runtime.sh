#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk_source="$(cd "$root/../.." && pwd)"
runtime_dir="${1:-$root/.runtime}"
tag="avm-dev-06e96ef71006-sdk-0748f4f0767a"
expected="8728497cf1e476d3d770a09b19929f7c196f2637654642727432b4c285c85143"
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
    echo "AVM SDK 0.2.0 requires Linux x86-64-v3 and glibc >= 2.39." >&2
    exit 2
fi
mkdir -p "$runtime_dir/download/$tag"
runtime_dir="$(cd "$runtime_dir" && pwd)"
archive="$runtime_dir/download/$tag/avm-sdk-linux-x86_64.tar.gz"
if [[ ! -f "$archive" ]]; then
    curl -fL --retry 2 --max-time 180 \
        "https://github.com/gosh-sh/avm-sdk/releases/download/$tag/avm-sdk-linux-x86_64.tar.gz" \
        -o "$archive"
fi
printf '%s  %s\n' "$expected" "$archive" | sha256sum -c -
# Pin the independently observed asset digest; use the trusted checkout's v2
# verifier/installer for bounded extraction, provenance and complete inventory.
checksum="$archive.sha256"
printf '%s  avm-sdk-linux-x86_64.tar.gz\n' "$expected" > "$checksum"
python3 -B "$sdk_source/scripts/release.py" install --archive "$archive" \
    --checksum "$checksum" --tag "$tag" \
    --sdk-home "$runtime_dir/home" --bin-dir "$runtime_dir/bin"
ln -sfn "home/releases/$tag/avm-sdk-linux-x86_64" "$runtime_dir/avm-sdk-linux-x86_64"
echo "Verified AVM SDK ready: $runtime_dir/avm-sdk-linux-x86_64"
