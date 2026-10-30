#!/usr/bin/env bash
set -euo pipefail
export TMPDIR TMP TEMP
root="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 "$root/scripts/check.py" "$@"
