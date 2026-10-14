#!/usr/bin/env bash
# Builds the fungible-token example, deploys it on a fresh local stand, and
# checks the mint, one transfer, one rejected overdraft, and the replay.
set -euo pipefail

example_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
build="${AVM_CONTRACT_BUILD:-avm-contract-build}"
stand="${AVM_LOCAL_STAND:-avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
state="${work}/token.state"
artifacts="${example_dir}/target/avm"
dapp_id="1111111111111111111111111111111111111111111111111111111111111111"
transferred_name_hex="746f6b656e2e7472616e7366657272656400" # "token.transferred\0"

fail() {
  echo "fungible-token stand e2e failed: $*" >&2
  exit 1
}

run() {
  "$stand" "$1" --state "$state" --wasmtime-profile "$profile" "${@:2}"
}

# Little-endian hex of an unsigned integer with the given byte width.
le_hex() {
  printf "%0$(($2 * 2))x" "$1" | fold -w2 | tac | tr -d '\n'
}

holder() {
  printf '%s%s' "$1" "$(le_hex "$2" 8)"
}

expect_state() {
  local receipt="$1" expected="0x$(le_hex 1000000 8)$(le_hex "$2" 4)00000000$3"
  [[ "$(jq -r .state "$receipt")" == "${expected}"* ]] || fail "unexpected state in ${receipt##*/}"
}

expect_status() {
  [[ "$(jq -r '.raw_executor_output[8:16]' "$1")" == "$(le_hex "$2" 4)" ]] ||
    fail "expected status $2 in ${1##*/}"
}

transfer() {
  local identity="$1" recipient="$2" amount="$3" time="$4"
  run call --address "$address" --method token.transfer \
    --args-json "\"0x${recipient}$(le_hex "$amount" 8)\"" \
    --identity "$identity" --value-main 0 --extra-currencies '' \
    --block-time "$time" --created-at "$time" --expire-at "$((time + 3600))" \
    --fuel-fee-shells 1000
}

"$build" build --manifest-path "${example_dir}/Cargo.toml" >/dev/null

run init --dapp-id "$dapp_id" >"${work}/init.json"
developer="$(jq -r .identities.developer "${work}/init.json")"
user="$(jq -r .identities.user "${work}/init.json")"

deploy=(
  deploy
  --wasm "${artifacts}/contract.wasm"
  --acab "${artifacts}/contract.acab"
  --descriptor "${artifacts}/contract.descriptor"
  --salt-hex 66756e6769626c652d746f6b656e
  --init-json null
  --block-time 1800000000 --created-at 1800000000 --expire-at 1800003600
  --fuel-fee-shells 1000
)
set +e
run "${deploy[@]}" >"${work}/funding-required.json" 2>"${work}/funding-required.stderr"
deploy_status=$?
set -e
[[ "$deploy_status" -eq 2 ]] || fail "first deploy should require funding"
address="$(jq -r .address "${work}/funding-required.json")"
run fund-user --address "$address" --shells 100000 >/dev/null
run "${deploy[@]}" >"${work}/deploy.json"
[[ "$(jq -r .status "${work}/deploy.json")" == ok ]] || fail "deploy"

# init credits the whole supply to the deployer's signing key.
run events >"${work}/events-after-deploy.json"
mint="$(le_hex 0 32)${developer}"
jq -e --arg name "$transferred_name_hex" --arg mint "$mint" \
  '[.events[] | select(contains($name) and contains($mint))] | length == 1' \
  "${work}/events-after-deploy.json" >/dev/null || fail "mint event"

transfer developer "$user" 250000 1800000010 >"${work}/transfer.json"
expect_status "${work}/transfer.json" 0
expect_state "${work}/transfer.json" 2 "$(holder "$developer" 750000)$(holder "$user" 250000)"

# An overdraft is rejected and leaves both balances unchanged.
transfer user "$developer" 250001 1800000020 >"${work}/overdraft.json"
expect_status "${work}/overdraft.json" 1303
expect_state "${work}/overdraft.json" 2 "$(holder "$developer" 750000)$(holder "$user" 250000)"

transfer user "$developer" 50000 1800000030 >"${work}/transfer-back.json"
expect_status "${work}/transfer-back.json" 0
expect_state "${work}/transfer-back.json" 2 "$(holder "$developer" 800000)$(holder "$user" 200000)"

run events >"${work}/events.json"
jq -e --arg name "$transferred_name_hex" '[.events[] | select(contains($name))] | length == 3' \
  "${work}/events.json" >/dev/null || fail "expected three token.transferred events"

run replay >"${work}/replay.json"
jq -e --slurpfile last "${work}/transfer-back.json" \
  '.status == "ok" and .account_state_root == $last[0].receipt.account_state_root
   and .cumulative_fee_shells == $last[0].receipt.cumulative_fee_shells' \
  "${work}/replay.json" >/dev/null || fail "replay diverged from the recorded journal"

echo "fungible-token stand e2e passed"
