#!/usr/bin/env bash
# Contract-initiated deploy with a callback request: the child is created and
# its init sees the callback request, and the parent must receive exactly one callback.
# Positive control: the same parent receives the callback of an ordinary call.
#
# Run this script from a matching SDK source checkout or installed source bundle.
# Generated source and state stay under --out; the checkout is unchanged.
# Requires avm-contract-build, avm-local-stand, and jq.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
build="${AVM_CONTRACT_BUILD:-avm-contract-build}"
stand="${AVM_LOCAL_STAND:-avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
work="${AVM_DEPLOY_REPRO_WORK:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --builder|--stand|--profile|--out)
      [[ $# -ge 2 && -n $2 ]] || { echo "missing value for $1" >&2; exit 2; }
      case "$1" in
        --builder) build="$2" ;; --stand) stand="$2" ;;
        --profile) profile="$2" ;; --out) work="$2" ;;
      esac
      shift 2 ;;
    --help)
      echo "usage: repro.sh [--builder COMMAND] [--stand COMMAND] [--profile FILE] [--out DIRECTORY]"
      exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ -z $work ]]; then work="$(mktemp -d)"; fi
mkdir -p "$work"
[[ ! -e $work/source && ! -e $work/base.state ]] || { echo "choose a fresh --out directory" >&2; exit 2; }
# Build a writable copy, preserving the installed/source checkout unchanged.
python3 - "$here" "$work/source" <<'PY'
from pathlib import Path
import shutil
import sys
example, output = map(Path, sys.argv[1:])
sdk = example.parent.parent
output.mkdir()
for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rustfmt.toml", "LICENSE.md"):
    if (sdk / name).is_file():
        shutil.copy2(sdk / name, output / name)
shutil.copytree(sdk / "crates/avm-contract-sdk", output / "crates/avm-contract-sdk",
                ignore=shutil.ignore_patterns("target", "__pycache__"))
target = output / "examples/deploy-callback-repro"
target.mkdir(parents=True)
for name in ("Cargo.toml", "Cargo.lock", "avm-contract.toml"):
    shutil.copy2(example / name, target / name)
if (example / "avm-memory.json").is_file():
    shutil.copy2(example / "avm-memory.json", target / "avm-memory.json")
shutil.copytree(example / "src", target / "src")
PY
here="${work}/source/examples/deploy-callback-repro"
echo "Callback example output: $work"
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
    --identity user --extra-currencies '' "${timestamps[@]}" >"$receipt"
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
# That external deploy makes the capture account the first contract of its own
# DApp. The parent's internal deploys of the same body stay in the parent's
# DApp: the same account id with the parent's DApp id.
external_child="$deployed"
child="${parent%%:*}:${deployed#*:}"
read_state "$capture" "$external_child"
captured="$hex"
init_method_le="$(slice "$captured" 40 48)"
body_len=$((16#$(slice "$captured" 48 52 | fold -w2 | tac | tr -d '\n')))
[[ "$(slice "$captured" 52 53)" == 01 ]] || fail "child init body was not captured"
deploy_body="$(slice "$captured" 64 $((64 + body_len)))"
[[ "$deploy_body" == 41434450* ]] || fail "captured init body is not ACDP"

# Positive control: ordinary call to the existing capture account with a callback request.
control="${work}/control.state"
cp "$capture" "$control"
call "$control" "$parent" repro.forward "$(forward_args "$external_child" "$ping_method_le" 01 "")" \
  "${work}/control.json"
read_state "$control" "$parent"
control_parent="$hex"
[[ "$(slice "$control_parent" 0 1)" == 02 ]] || fail "control callback was not delivered"
[[ "$(slice "$control_parent" 3 4)" == 01 ]] || fail "control callback count"
echo "ordinary call, callback request 01: parent phase $(slice "$control_parent" 0 1)," \
  "callback status $(slice "$control_parent" 4 8)"

# The parent forwards the largest body the network allows, with a callback
# request: the external body is max_body_size (65,536 bytes), 77 of them the
# length prefix and the forward header. One byte more is rejected at ingress.
largest="${work}/largest-body.state"
cp "$capture" "$largest"
largest_args="$(forward_args "$external_child" "$ping_method_le" 01 "$(printf "%0$((2 * 65459))d" 0)")"
call "$largest" "$parent" repro.forward "$largest_args" "${work}/largest-body.json"
read_state "$largest" "$parent"
[[ "$(slice "$hex" 0 1)" == 02 && "$(slice "$hex" 3 4)" == 01 ]] || fail "largest body callback"
[[ "$(slice "$hex" 4 8)" == 00000000 ]] || fail "largest body delivery status"
tick
run "$largest" call --address "$parent" --method repro.forward --args-json "${largest_args%\"}00\"" \
  --identity user --extra-currencies '' "${timestamps[@]}" >"${work}/oversized-body.json" 2>/dev/null || true
jq -r .error "${work}/oversized-body.json" | grep -q "exceeds the configured canonical byte limit" ||
  fail "a body one byte over max_body_size was not rejected at ingress"
echo "largest body: 65459 bytes forwarded and delivered with a callback, one byte more rejected at ingress"

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
      # The deploy without a callback request above left the child active.
      cp "${work}/deploy-00.state" "$rejected"
    else
      cp "$base" "$rejected"
      if [[ "$failure" == malformed ]]; then body=41434450; fi
      if [[ "$failure" == wrong-address ]]; then
        target="${parent%%:*}:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
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

# Ordinary calls in the same DApp and across DApps, to a receiver that succeeds
# or rejects with status 1505, with and without a callback request. A requested
# callback returns the unused part of the 1000 SHELL cross-DApp reserve
# (docs/fuel-and-fees.md). The same-DApp child is created by an internal deploy
# from the parent; the remote one is the externally deployed capture account,
# the first contract of its own DApp.
remote="$external_child"
routes="${work}/routes.state"
cp "$capture" "$routes"
run "$routes" fund-user --address "$child" --shells 10000000 >/dev/null
call "$routes" "$parent" repro.forward \
  "$(forward_args "$child" "$init_method_le" 01 "$deploy_body")" "${work}/child-deploy.json"
has_account "$routes" "$child" || fail "same-DApp child was not created"
for destination in "$child" "$remote"; do
  route=same-dapp
  [[ "$destination" == "$child" ]] || route=cross-dapp
  for method_le in "$ping_method_le" 7b00000000000000; do
    expected_status=00000000
    [[ "$method_le" == "$ping_method_le" ]] || expected_status=e1050000
    for request_callback in 00 01; do
      scenario="${work}/${route}-${method_le}-${request_callback}.state"
      cp "$routes" "$scenario"
      read_state "$scenario" "$parent"
      callbacks_before=$((16#$(slice "$hex" 3 4)))
      call "$scenario" "$parent" repro.forward \
        "$(forward_args "$destination" "$method_le" "$request_callback" "")" \
        "${work}/${route}-${method_le}-${request_callback}.json"
      read_state "$scenario" "$parent"
      [[ $((16#$(slice "$hex" 3 4))) == $((callbacks_before + 10#$request_callback)) ]] ||
        fail "${route} callback count"
      if [[ "$request_callback" == 01 ]]; then
        [[ "$(slice "$hex" 4 8)" == "$expected_status" ]] || fail "${route} callback status"
        if [[ "$route" == cross-dapp ]]; then
          [[ "$(slice "$hex" 56 64)" != 0000000000000000 ]] ||
            fail "cross-DApp callback did not return the unused reserve"
        fi
      fi
      run "$scenario" inspect | jq -e '.queue.message_count == 0' >/dev/null || fail "${route} queue not empty"
      run "$scenario" replay | jq -e '.status == "ok"' >/dev/null || fail "${route} replay"
      echo "${route} call, status ${expected_status}, callback ${request_callback}: committed, queue empty, replay ok"
    done
  done
done
echo "PASS: same-DApp and cross-DApp calls commit with and without callbacks"
