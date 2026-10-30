#!/usr/bin/env bash
set -euo pipefail
export TMPDIR TMP TEMP
experiment="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 "$experiment/scripts/qualify_runtime.py" "$@"
