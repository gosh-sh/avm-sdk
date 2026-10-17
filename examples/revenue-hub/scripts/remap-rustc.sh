#!/usr/bin/env bash
set -euo pipefail
sdk_source="$(cd "$(dirname "$0")/../../.." && pwd)"
exec "$@" "--remap-path-prefix=$sdk_source=/avm-sdk"
