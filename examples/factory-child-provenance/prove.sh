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
report_method_le="ace9611695a2bc2c" # child.report 0x2cbca2951661e9ac, little-endian
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

# External call signed by the user, the owner_id of the factory and the relays.
# Sets $status to the contract status as 8 hex digits.
call() {
  local address="$1" method="$2" args="$3"
  tick
  run call --address "$address" --method "$method" --args-json "$args" --identity user \
    --extra-currencies '' "${timestamps[@]}" >"${work}/call.json" || [[ $? == 2 ]] || fail "call $method"
  # A contract status fails the call: exit status 2 with status_code.
  if [[ "$(jq -r .error "${work}/call.json")" == execution_failed ]]; then
    status="$(printf '%08x' "$(jq -r .status_code "${work}/call.json")" | fold -w2 | tac | tr -d '\n')"
    return
  fi
  [[ "$(jq -r '.raw_executor_output[0:8]' "${work}/call.json")" == 41434d4f ]] || fail "non-ACMO response"
  status="$(jq -r '.raw_executor_output[8:16]' "${work}/call.json")"
}

# Hex state of an account. Sets $hex.
read_state() {
  hex="$(run inspect --account "$1" | jq -r .raw_state)"
  [[ -n "$hex" && "$hex" != null ]] || fail "inspect $1"
}

# A child takes no external call: its report is sent by a relay. Sets $status.
report() {
  call "$1" relay.send "\"0x${2/:/}${report_method_le}\""
}

# Byte range [start, end) of a hex string.
slice() {
  echo "${1:$(($2 * 2)):$((($3 - $2) * 2))}"
}

# A deployed account has a code selection; a funded undeployed address is an
# ordinary Uninit account with a zero selection.
has_account() {
  run inspect --account "$1" 2>/dev/null | jq -e '.selected_code_id != ("0" * 64)' >/dev/null
}

# Account hash of $2 in state $1.
account_hash() {
  "$stand" inspect --state "$1" --wasmtime-profile "$profile" |
    jq -er --arg a "$2" '.accounts[] | select(.address == $a) | .account_hash'
}

# Hash of the Uninit image of address $1 funded once with $2 SHELL. Sets $uninit_hash.
uninit_reference() {
  local reference="${work}/uninit-reference-${1#*:}-$2.state"
  "$stand" init --state "$reference" --wasmtime-profile "$profile" --dapp-id "$dapp_id" >/dev/null
  "$stand" fund-user --state "$reference" --wasmtime-profile "$profile" --address "$1" \
    --shells "$2" >/dev/null
  uninit_hash="$(account_hash "$reference" "$1")"
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

# Address of a child of factory $3 (default: the factory) with salt $1 and
# selection $2: the named factory's DApp and the account id the stand derives,
# which does not depend on the DApp. Sets $quoted.
child_address() {
  local named="${3:-$factory}"
  quote deploy --package-id "$package_id" --selected-code-id "$2" --owner-id "${named#*:}" \
    --salt-hex "$1" --init-json "$(role_init 02 "${named/:/}")"
  quoted="${named%%:*}:${quoted#*:}"
}

# Relay $1 forges the deploy of a child of factory $2 with salt $3, to the
# prefunded address $4. Sets $forge_ok to yes when no child is committed, the
# target funding paid exactly $5 SHELL and the relay call stayed valid with its
# state unchanged.
forge() {
  local relay="$1" named="$2" salt="$3" target="$4" fee="$5" before after
  read_state "$relay"
  before="$hex"
  uninit_reference "$target" 10000000
  [[ "$(account_hash "$state" "$target")" == "$uninit_hash" ]] || fail "forged child funding precondition"
  tick
  run call --address "$relay" --method relay.forge --args-json "\"0x${named/:/}${salt}\"" \
    --identity user --extra-currencies '' "${timestamps[@]}" >"${work}/forge.json"
  after="$(account_hash "$state" "$target")"
  if [[ "$fee" != 0 ]]; then
    uninit_reference "$target" $((10000000 - fee))
  fi
  forge_ok=no
  if ! has_account "$target" && [[ "$after" == "$uninit_hash" ]] &&
    jq -e --arg before "$before" \
      '.raw_executor_output[0:16] == "41434d4f00000000" and .raw_state == $before and .queue.after == 0' \
      "${work}/forge.json" >/dev/null; then
    forge_ok=yes
  fi
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
report "$relay_v1" "$child_1"
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

# 4. A relay of the same version deploys a child named after the factory. The
#    relay is the first contract of its own DApp, so the node refuses this
#    internal deploy into the factory's DApp before init: no child and no init
#    fee. The relay call remains valid.
salt_4=0000000000000004
child_address "$salt_4" "$selection_v1"
child_4="$quoted"
fund "$child_4"
forge "$relay_v1" "$factory" "$salt_4" "$child_4" 0
case_4() {
  [[ "$forge_ok" == yes ]]
}
expect_ok "4 forged deploy, same version: the DApp boundary refuses it outside the relay's DApp" case_4
# In the relay's own DApp the forged deploy of a child named after the
# factory's account id reaches the child's init, which refuses a deployer other
# than the factory it names. No child is committed; the existing target funding
# pays the fixed 10 SHELL init fee once. The relay call remains valid.
named_4="${relay_v1%%:*}:${factory#*:}"
child_address "$salt_4" "$selection_v1" "$named_4"
child_4_relay="$quoted"
fund "$child_4_relay"
forge "$relay_v1" "$named_4" "$salt_4" "$child_4_relay" 10
expect_ok "4 forged deploy in the relay's DApp: the child refuses a deployer other than its factory" case_4

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
count_reports
reports_before="$reports"
forge "$relay_v2" "$factory" "$salt_5" "$child_5" 0
case_5() {
  count_reports
  [[ "$forge_ok" == yes && "$reports" == "$reports_before" ]]
}
expect_ok "5 forged deploy, another version: the DApp boundary refuses it outside the relay's DApp" case_5
# The permissive version still runs in the relay's own DApp: there its child
# takes the relay as its deployer.
named_5="${relay_v2%%:*}:${factory#*:}"
child_address "$salt_5" "$selection_v2" "$named_5"
child_5_relay="$quoted"
fund "$child_5_relay"
tick
run call --address "$relay_v2" --method relay.forge --args-json "\"0x${named_5/:/}${salt_5}\"" \
  --identity user --extra-currencies '' "${timestamps[@]}" >"${work}/different-version-deploy.json"
forged=no
if has_account "$child_5_relay"; then
  read_state "$child_5_relay"
  [[ "$(slice "$hex" 176 240)" == "${relay_v2/:/}" ]] && forged=yes
fi
[[ "$forged" == yes ]] || fail "same-package permissive child was not exercised"

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

# 8. A relay forwards the largest body the network allows to the factory's
# ping: the external body is max_body_size (65,536 bytes), 76 of them the
# length prefix and the send header. One byte more is rejected at ingress.
ping_method_le="7a3f34f925f077ca" # ping 0xca77f025f9343f7a, little-endian
largest="\"0x${factory/:/}${ping_method_le}$(printf "%0$((2 * 65460))d" 0)\""
call "$relay_v1" relay.send "$largest"
largest_status="$status"
tick
run call --address "$relay_v1" --method relay.send --args-json "${largest%\"}00\"" --identity user \
  --extra-currencies '' "${timestamps[@]}" >"${work}/oversized-body.json" 2>/dev/null || true
case_8() {
  [[ "$largest_status" == 00000000 ]] &&
    jq -r .error "${work}/oversized-body.json" | grep -q "exceeds the configured canonical byte limit"
}
expect_ok "8 largest body: a relay forwards 65,460 bytes; one byte more is rejected at ingress" case_8

state_hash() { python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$state"; }
before_replay="$(state_hash)"
run replay | jq -e '.status == "ok"' >/dev/null || fail "replay"
[[ "$(state_hash)" == "$before_replay" ]] || fail "replay mutated persisted state"
echo "replay ok"
printf 'evidence %s\n' "$work"
exit "$failures"
