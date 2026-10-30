#!/usr/bin/env bash
# What a factory can prove about the sender of an internal message.
#
# A factory deploys children and accepts reports only from addresses that
# derive from its package, itself as owner, a salt and the child constructor
# data. Each case prints OK when the stand behaves as a factory needs, and
# REPRODUCED when the factory is deceived.
#
# Run from an avm-sdk checkout or a writable development copy of a release, so
# the relative SDK path dependency resolves. Requires avm-contract-build,
# avm-local-stand, and jq.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
build="${AVM_CONTRACT_BUILD:-avm-contract-build}"
stand="${AVM_LOCAL_STAND:-avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
work_parent="${PROVENANCE_OUT:-${TMPDIR:-${sdk_home}/tmp}}"
mkdir -p "$work_parent"
work="$(mktemp -d "${work_parent}/provenance.XXXXXXXX")"
cleanup() {
  if [[ "${PROVENANCE_KEEP:-0}" != 1 ]]; then
    python3 - "$work" <<'PY'
from pathlib import Path
import shutil, sys
p = Path(sys.argv[1])
if p.name.startswith('provenance.') and (p / '.provenance-owned').is_file():
    shutil.rmtree(p)
PY
  fi
}
touch "${work}/.provenance-owned"
trap cleanup EXIT
state="${work}/stand.state"
dapp_id="1111111111111111111111111111111111111111111111111111111111111111"
accept_method_le="41ed76d491639b31" # factory.accept 0x319b6391d476ed41, little-endian
logical_time=1800000000
failures=0

fail() {
  echo "prove failed: $*" >&2
  exit 1
}

run() {
  local command="$1"
  shift
  local receipt
  receipt="$(mktemp "${work}/receipt.XXXXXXXX.json")"
  "$stand" "$command" --state "$state" --wasmtime-profile "$profile" "$@" | tee "$receipt"
}

tick() {
  logical_time=$((logical_time + 10))
  timestamps=(--block-time "$logical_time" --created-at "$logical_time"
    --expire-at "$((logical_time + 3600))" --fuel-fee-shells 10000)
}

# Address the stand derives for a deploy, from its unfunded attempt. Sets $quoted.
quote() {
  local command="$1"
  shift
  tick
  run "$command" "$@" "${timestamps[@]}" >"${work}/quote.json" 2>/dev/null || true
  [[ "$(jq -r .error "${work}/quote.json")" == funding_required ]] || fail "$command precondition"
  quoted="$(jq -r .address "${work}/quote.json")"
}

fund() {
  run fund-user --address "$1" --shells 10000000 >/dev/null
}

# Two-step CLI deploy or publication. Sets $deployed.
cli_create() {
  local command="$1"
  shift
  quote "$command" "$@"
  deployed="$quoted"
  fund "$deployed"
  tick
  run "$command" "$@" "${timestamps[@]}" | jq -e '.status == "ok"' >/dev/null || fail "$command"
}

# Signed external call. Sets $status to the contract status as 8 hex digits.
call() {
  local address="$1" method="$2" args="$3"
  tick
  run call --address "$address" --method "$method" --args-json "$args" --identity developer \
    --extra-currencies '' "${timestamps[@]}" >"${work}/call.json"
  [[ "$(jq -r '.raw_executor_output[0:8]' "${work}/call.json")" == 41434d4f ]] || fail "non-ACMO response"
  status="$(jq -r '.raw_executor_output[8:16]' "${work}/call.json")"
}

# Hex state of an account, read through a no-op call. Sets $hex.
read_state() {
  call "$1" provenance.ping null
  [[ "$status" == 00000000 ]] || fail "ping $1"
  hex="$(jq -r .raw_state "${work}/call.json")"
}

# Byte range [start, end) of a hex string.
slice() {
  echo "${1:$(($2 * 2)):$((($3 - $2) * 2))}"
}

has_account() {
  run inspect | jq -e --arg address "$1" 'any(.accounts[]; .address == $address)' >/dev/null
}

result() {
  local verdict="$1"
  shift
  printf '%-10s %s\n' "$verdict" "$*"
}

expect_ok() {
  local description="$1"
  shift
  if "$@"; then
    result OK "$description"
  else
    result FAILED "$description"
    failures=$((failures + 1))
  fi
}

zero_address="$(printf '0%.0s' {1..128})"
role_init() {
  printf '"0x%s%s"' "$1" "$2"
}

# Version 1 and, with PROVENANCE_PERMISSIVE_CHILD set, version 2 of the package.
mkdir -p "${work}/v1" "${work}/v2"
PROVENANCE_PERMISSIVE_CHILD=1 "$build" build --manifest-path "${here}/Cargo.toml" >/dev/null
cp "${here}"/target/avm/contract.* "${work}/v2/"
env -u PROVENANCE_PERMISSIVE_CHILD "$build" build --manifest-path "${here}/Cargo.toml" >/dev/null
cp "${here}"/target/avm/contract.* "${work}/v1/"
cmp -s "${work}/v1/contract.wasm" "${work}/v2/contract.wasm" && fail "both versions built the same WASM"
v1=(--wasm "${work}/v1/contract.wasm" --acab "${work}/v1/contract.acab"
  --descriptor "${work}/v1/contract.descriptor")
v2=(--wasm "${work}/v2/contract.wasm" --acab "${work}/v2/contract.acab"
  --descriptor "${work}/v2/contract.descriptor")

run init --dapp-id "$dapp_id" >/dev/null
cli_create deploy "${v1[@]}" --salt-hex 666163746f7279 --init-json "$(role_init 01 "$zero_address")"
factory="$deployed"
run inspect >"${work}/inspect.json"
package_id="$(jq -r '.registry_entries[] | select(.key | startswith("01")) | .key[2:]' \
  "${work}/inspect.json")"
selection_v1="$(jq -r '.prepared_selection_ids[0]' "${work}/inspect.json")"
cli_create deploy --package-id "$package_id" --selected-code-id "$selection_v1" \
  --salt-hex 72656c61792d31 --init-json "$(role_init 03 "$zero_address")"
relay_v1="$deployed"
child_init="$(role_init 02 "${factory/:/}")"

# Address the stand derives for a child of the factory with this salt and
# selection. Sets $quoted.
child_address() {
  quote deploy --package-id "$package_id" --selected-code-id "$2" --owner-id "${factory#*:}" \
    --salt-hex "$1" --init-json "$child_init"
}

# Reports the factory accepted. Sets $reports and $hex.
count_reports() {
  read_state "$factory"
  reports=$((16#$(slice "$hex" 112 116 | fold -w2 | tac | tr -d '\n')))
}

# 1. The factory deploys a child; its derivation matches the stand's.
salt_1=0000000000000001
child_address "$salt_1" "$selection_v1"
child_1="$quoted"
fund "$child_1"
call "$factory" factory.spawn "\"0x${salt_1}\""
spawn_status="$status"
read_state "$factory"
spawned="$(slice "$hex" 180 244)"
case_1() {
  [[ "$spawn_status" == 00000000 && "$spawned" == "${child_1/:/}" ]] && has_account "$child_1" &&
    read_state "$child_1" && [[ "$(slice "$hex" 176 240)" == "${factory/:/}" ]]
}
expect_ok "1 spawn: the factory derives the stand's child address; child init saw the factory" case_1

# 2. The child reports; the factory accepts it.
call "$child_1" child.report null
case_2() {
  count_reports
  [[ "$reports" == 1 && "$(slice "$hex" 116 180)" == "${child_1/:/}" ]]
}
expect_ok "2 accept: the factory accepts its own child's report" case_2

# 3. A relay sends the same report with the child's salt.
call "$relay_v1" relay.send "\"0x${factory/:/}${accept_method_le}${salt_1}\""
send_status="$status"
case_3() {
  count_reports
  [[ "$send_status" == 00000000 && "$reports" == 1 ]]
}
expect_ok "3 spoof: a report from an address that does not derive is rejected" case_3

# 4. A relay of the same version deploys a child named after the factory.
salt_4=0000000000000004
child_address "$salt_4" "$selection_v1"
child_4="$quoted"
fund "$child_4"
# The child's init fails without committing a child; the existing target
# funding pays the fixed 10 SHELL init fee once. The relay call remains valid.
read_state "$relay_v1"
relay_state_before="$hex"
funding_before="$(run inspect | jq -er --arg a "$child_4" '.funding[] | select(.address == $a) | .extra[] | select(.currency_id == 2) | .amount | tonumber')"
tick
run call --address "$relay_v1" --method relay.forge --args-json "\"0x${factory/:/}${salt_4}\"" \
  --identity developer --extra-currencies '' "${timestamps[@]}" >"${work}/failed-init.json"
funding_after="$(run inspect | jq -er --arg a "$child_4" '.funding[] | select(.address == $a) | .extra[] | select(.currency_id == 2) | .amount | tonumber')"
case_4() {
  ! has_account "$child_4" && [[ "$funding_after" == $((funding_before - 10)) ]] &&
    jq -e --arg before "$relay_state_before" \
      '.raw_executor_output[0:16] == "41434d4f00000000" and .raw_state == $before and .queue.after == 0' \
      "${work}/failed-init.json" >/dev/null
}
expect_ok "4 forged deploy, same version: the child refuses a deployer other than its factory" case_4

# 5. Version 2 of the package, whose child accepts any deployer, is published
#    as a relay. That relay deploys a child named after the factory, from the
#    code selection it runs.
cli_create publish-version "${v2[@]}" --package-id "$package_id" \
  --salt-hex 72656c61792d32 --init-json "$(role_init 03 "$zero_address")"
relay_v2="$deployed"
run inspect >"${work}/inspect.json"
selection_v2="$(jq -r --arg v1 "$selection_v1" \
  '[.prepared_selection_ids[] | select(. != $v1)][0] // empty' "${work}/inspect.json")"
[[ -n "$selection_v2" ]] || fail "version 2 has no selection"
salt_5=0000000000000005
child_address "$salt_5" "$selection_v1"
child_5_v1="$quoted"
child_address "$salt_5" "$selection_v2"
child_5="$quoted"
[[ "$child_5" == "$child_5_v1" ]] || fail "current address identity must not bind the code version"
result NOTE "5 the stand quotes the same child address for both code selections"
fund "$child_5"
tick
run call --address "$relay_v2" --method relay.forge --args-json "\"0x${factory/:/}${salt_5}\"" \
  --identity developer --extra-currencies '' "${timestamps[@]}" >"${work}/different-version-deploy.json"
count_reports
reports_before="$reports"
forged=no
if has_account "$child_5"; then
  read_state "$child_5"
  [[ "$(slice "$hex" 176 240)" == "${relay_v2/:/}" ]] && forged=yes
fi
[[ "$forged" == yes ]] || fail "same-package permissive child was not exercised"
call "$child_5" child.report null
count_reports
[[ "$reports" == $((reports_before + 1)) && "$(slice "$hex" 116 180)" == "${child_5/:/}" ]] || fail "documented derived-address limitation changed"
result LIMITATION "5 derived address accepts another code version's child; it does not prove current code or deployer"

# 6. The factory's child cannot be moved to version 2.
tick
upgrade_status=0
run upgrade --address "$child_1" --selected-code-id "$selection_v2" --migration-payload-hex 00 \
  --identity package-owner "${timestamps[@]}" >/dev/null 2>&1 || upgrade_status=$?
case_6() {
  [[ "$upgrade_status" != 0 ]]
}
expect_ok "6 upgrade: a child without an upgrade path keeps its version" case_6

count_reports
reports_before="$reports"
call "$factory" factory.accept "\"0x${salt_1}\""
[[ "$status" == 45060000 ]] || fail "external report must fail not_a_child1605"
count_reports
[[ "$reports" == "$reports_before" ]] || fail "external report changed factory state"
result OK "7 external report rejected without state effects"

state_hash() { python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$state"; }
before_replay="$(state_hash)"
run replay | jq -e '.status == "ok"' >/dev/null || fail "replay"
[[ "$(state_hash)" == "$before_replay" ]] || fail "replay mutated persisted state"
echo "replay ok"
printf 'evidence %s\n' "$work"
exit "$failures"
