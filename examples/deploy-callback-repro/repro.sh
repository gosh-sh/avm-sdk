#!/usr/bin/env bash
# Contract-initiated deploy with a callback request: the child is created and
# its init sees the callback request, and the parent must receive exactly one callback.
# Positive control: the same parent receives the callback of an ordinary call.
#
# Copy this directory to examples/deploy-callback-repro in an avm-sdk checkout
# so the relative SDK path dependency resolves, then run this script.
# Requires avm-contract-build, avm-local-stand, and jq.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
build="${AVM_CONTRACT_BUILD:-avm-contract-build}"
stand="${AVM_LOCAL_STAND:-avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
work="${AVM_DEPLOY_REPRO_WORK:-$(mktemp -d)}"
mkdir -p "$work"
if [[ -z ${AVM_DEPLOY_REPRO_WORK:-} ]]; then trap 'rm -rf "$work"' EXIT; fi
artifacts="${here}/target/avm"
dapp_id="1111111111111111111111111111111111111111111111111111111111111111"
ping_method_le="1c8422e2cde1ec96" # repro.ping 0x96ece1cde222841c, little-endian
logical_time=1800000000

fail() {
  echo "repro failed: $*" >&2
  exit 1
}

run() {
  local state="$1" command="$2"
  shift 2
  "$stand" "$command" --state "$state" --wasmtime-profile "$profile" "$@"
}

tick() {
  logical_time=$((logical_time + 10))
  timestamps=(--block-time "$logical_time" --created-at "$logical_time"
    --expire-at "$((logical_time + 3600))" --fuel-fee-shells 10000)
}

# Two-step CLI deploy: the unfunded attempt returns the address, then fund it.
# Sets $deployed.
cli_deploy() {
  local state="$1" salt_hex="$2"
  shift 2
  tick
  local args=("$@" --salt-hex "$salt_hex" --init-json null "${timestamps[@]}")
  run "$state" deploy "${args[@]}" >"${work}/funding.json" 2>/dev/null || true
  [[ "$(jq -r .error "${work}/funding.json")" == funding_required ]] || fail "deploy precondition"
  deployed="$(jq -r .address "${work}/funding.json")"
  run "$state" fund-user --address "$deployed" --shells 10000000 >/dev/null
  run "$state" deploy "${args[@]}" | jq -e '.status == "ok"' >/dev/null || fail "deploy"
}

call() {
  local state="$1" address="$2" method="$3" args="$4" receipt="$5"
  tick
  run "$state" call --address "$address" --method "$method" --args-json "$args" \
    --identity developer --value-main 0 --extra-currencies '' "${timestamps[@]}" >"$receipt"
  [[ "$(jq -r '.raw_executor_output[8:16]' "$receipt")" == 00000000 ]] ||
    fail "$method returned a non-zero status"
}

# Hex state of an account, without executing or mutating it. Sets $hex.
read_state() {
  hex="$(run "$1" inspect --account "$2" | jq -r .raw_state)"
}

# Byte range [start, end) of a hex string.
slice() {
  echo "${1:$(($2 * 2)):$((($3 - $2) * 2))}"
}

forward_args() {
  local destination="$1" method_le="$2" request_callback="$3" body="$4"
  printf '"0x%s%s%s%s"' "${destination/:/}" "$method_le" "$request_callback" "$body"
}

has_account() {
  run "$1" inspect | jq -e --arg address "$2" 'any(.accounts[]; .address == $address)' >/dev/null
}

"$build" build --manifest-path "${here}/Cargo.toml" >/dev/null

base="${work}/base.state"
run "$base" init --dapp-id "$dapp_id" >/dev/null
cli_deploy "$base" 706172656e74 \
  --wasm "${artifacts}/contract.wasm" --acab "${artifacts}/contract.acab" \
  --descriptor "${artifacts}/contract.descriptor"
parent="$deployed"
run "$base" inspect >"${work}/inspect.json"
selected_code_id="$(jq -r '.prepared_selection_ids[0]' "${work}/inspect.json")"
package_id="$(jq -r '.registry_entries[] | select(.key | startswith("01")) | .key[2:]' "${work}/inspect.json")"

# Capture the canonical deploy body that the CLI delivers to a child's init.
capture="${work}/capture.state"
cp "$base" "$capture"
cli_deploy "$capture" 6368696c64 --package-id "$package_id" --selected-code-id "$selected_code_id"
child="$deployed"
read_state "$capture" "$child"
captured="$hex"
init_method_le="$(slice "$captured" 40 48)"
body_len=$((16#$(slice "$captured" 48 52 | fold -w2 | tac | tr -d '\n')))
[[ "$(slice "$captured" 52 53)" == 01 ]] || fail "child init body was not captured"
deploy_body="$(slice "$captured" 64 $((64 + body_len)))"
[[ "$deploy_body" == 41434450* ]] || fail "captured init body is not ACDP"

# Positive control: ordinary call to the existing child with a callback request.
control="${work}/control.state"
cp "$capture" "$control"
call "$control" "$parent" repro.forward "$(forward_args "$child" "$ping_method_le" 01 "")" \
  "${work}/control.json"
read_state "$control" "$parent"
control_parent="$hex"
[[ "$(slice "$control_parent" 0 1)" == 02 ]] || fail "control callback was not delivered"
[[ "$(slice "$control_parent" 3 4)" == 01 ]] || fail "control callback count"
echo "ordinary call, callback request 01: parent phase $(slice "$control_parent" 0 1)," \
  "callback status $(slice "$control_parent" 4 8)"

# Same parent forwards the captured deploy body, with and without a callback request.
for request_callback in 00 01; do
  scenario="${work}/deploy-${request_callback}.state"
  cp "$base" "$scenario"
  run "$scenario" fund-user --address "$child" --shells 10000000 >/dev/null
  call "$scenario" "$parent" repro.forward \
    "$(forward_args "$child" "$init_method_le" "$request_callback" "$deploy_body")" \
    "${work}/deploy-${request_callback}.json"
  has_account "$scenario" "$child" || fail "child was not created"
  read_state "$scenario" "$child"
  child_state="$hex"
  read_state "$scenario" "$parent"
  parent_state="$hex"
  [[ "$(slice "$child_state" 1 2)" == 02 ]] || fail "child init did not see an internal caller"
  [[ "$(slice "$child_state" 2 3)" == "$request_callback" ]] || fail "child callback flag"
  [[ "$(slice "$parent_state" 3 4)" == "$request_callback" ]] || fail "exact callback count"
  [[ "$(slice "$parent_state" 4 8)" == 00000000 ]] || fail "deploy callback status"
  run "$scenario" inspect | jq -e '.queue.message_count == 0' >/dev/null || fail "queue not empty"
  run "$scenario" replay | jq -e '.status == "ok"' >/dev/null || fail "replay"
  echo "deploy, callback request ${request_callback}: child created," \
    "init saw callback request ${request_callback}," \
    "parent phase $(slice "$parent_state" 0 1), queue empty, replay ok"
done

if [[ "$(slice "$parent_state" 0 1)" == 01 ]]; then
  fail "the deploy carried a callback request, but no callback reached the parent"
else
  [[ "$(slice "$parent_state" 0 1)" == 02 ]] || fail "deploy callback phase"
  echo "PASS: ordinary control, no-request deploy, exactly one deploy success callback, and replay"
fi

# Real deploy-precheck failures must retain the parent's forwarding transition,
# return its 17 SHELL, and never execute init or mutate the target. An ordinary
# callback control cannot establish any of these assertions.
for failure in active malformed wrong-address; do
  for request_callback in 00 01; do
    rejected="${work}/deploy-${failure}-${request_callback}.state"
    target="$child"
    body="$deploy_body"
    if [[ "$failure" == active ]]; then
      cp "$capture" "$rejected"
    else
      cp "$base" "$rejected"
      if [[ "$failure" == malformed ]]; then body=41434450; fi
      if [[ "$failure" == wrong-address ]]; then
        target="${dapp_id}:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
      fi
      run "$rejected" fund-user --address "$target" --shells 10000000 >/dev/null
    fi
    run "$rejected" inspect >"${work}/failure-before.json"
    call "$rejected" "$parent" repro.forward \
      "$(forward_args "$target" "$init_method_le" "$request_callback" "$body")" \
      "${work}/deploy-${failure}-${request_callback}.json"
    run "$rejected" inspect >"${work}/failure-after.json"
    jq -S --arg target "$target" '{target: [.accounts[] | select(.address == $target)],
      funding, registry_root, init_event_count, init_calls: .counters.init_executor_calls}' \
      "${work}/failure-before.json" >"${work}/target-before.json"
    jq -S --arg target "$target" '{target: [.accounts[] | select(.address == $target)],
      funding, registry_root, init_event_count, init_calls: .counters.init_executor_calls}' \
      "${work}/failure-after.json" >"${work}/target-after.json"
    cmp "${work}/target-before.json" "${work}/target-after.json" || fail "failure changed receiver/registry/init effects"
    before_calls="$(jq .counters.message_executor_calls "${work}/failure-before.json")"
    after_calls="$(jq .counters.message_executor_calls "${work}/failure-after.json")"
    [[ "$after_calls" == "$((before_calls + 1 + 10#$request_callback))" ]] || fail "failure executed unexpected guest calls"
    read_state "$rejected" "$parent"
    [[ "$(slice "$hex" 3 4)" == "$request_callback" ]] || fail "exact failure callback count"
    if [[ "$request_callback" == 01 ]]; then
      [[ "$(slice "$hex" 0 1)" == 02 ]] || fail "failure callback phase"
      [[ "$(slice "$hex" 4 8)" == 3f000000 ]] || fail "VM_DEPLOY_FAILED status"
      [[ "$(slice "$hex" 56 64)" == 1100000000000000 ]] || fail "failure did not refund exactly 17 SHELL"
    else
      [[ "$(slice "$hex" 0 1)" == 01 ]] || fail "no-handler refund executed callback code"
    fi
    jq -e '.queue.message_count == 0' "${work}/failure-after.json" >/dev/null || fail "failure queue not drained"
    run "$rejected" replay | jq -e '.status == "ok"' >/dev/null || fail "failure replay"
    echo "deploy ${failure}, callback ${request_callback}: parent retained, exact notification, target unchanged, replay ok"
  done
done
echo "PASS: deploy precheck failures deliver requested status 63 callbacks or no-handler refunds"
echo "KNOWN LIMITATION: failures after init starts still need fee/refund contract resolution and implementation"
