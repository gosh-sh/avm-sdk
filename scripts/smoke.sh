#!/usr/bin/env bash
set -euo pipefail

sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
stand="${AVM_LOCAL_STAND:-${sdk_home}/current/bin/avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
state_dir="$(mktemp -d)"
trap 'rm -rf "$state_dir"' EXIT
state="${state_dir}/smoke.state"
dapp_id="1111111111111111111111111111111111111111111111111111111111111111"

"$stand" init --state "$state" --wasmtime-profile "$profile" --dapp-id "$dapp_id" >/dev/null
"$stand" inspect --state "$state" --wasmtime-profile "$profile" >/dev/null
"$stand" replay --state "$state" --wasmtime-profile "$profile" >/dev/null

echo "AVM local stand smoke passed"
