#!/usr/bin/env bash
set -euo pipefail
if [[ $# -ne 4 ]]; then
    echo "usage: package-release.sh <node-source> <sdk-source> <verified-build-directory> <new-output-directory>" >&2
    exit 2
fi
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 -B "$script_dir/build-release.py" package --source "$1" --sdk "$2" --build-root "$3" --output "$4"
