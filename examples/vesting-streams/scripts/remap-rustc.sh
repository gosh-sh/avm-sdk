#!/usr/bin/env bash
set -euo pipefail
sdk_root="$(cd "$(dirname "$0")/../../.." && pwd)"
exec "$@" "--remap-path-prefix=$sdk_root=/avm-sdk"
