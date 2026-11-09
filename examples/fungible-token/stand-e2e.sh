#!/usr/bin/env bash
# Builds the fungible-token and wallet examples, deploys them on a fresh local
# stand, and checks the mint, transfers through holder wallets, rejected
# overdraft and capacity, the external-call rules, and the replay.
set -euo pipefail

example_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
wallet_dir="${example_dir}/../wallet"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
build="${AVM_CONTRACT_BUILD:-avm-contract-build}"
stand="${AVM_LOCAL_STAND:-avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
work="${AVM_TOKEN_WORK:-$(mktemp -d)}"
mkdir -p "$work"
if [[ -z ${AVM_TOKEN_WORK:-} ]]; then
  trap 'python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1])" "$work"' EXIT
fi
state="${work}/token.state"
artifacts="${example_dir}/target/avm"
wallet_artifacts="${wallet_dir}/target/avm"
dapp_id="1111111111111111111111111111111111111111111111111111111111111111"
transferred_name_hex="746f6b656e2e7472616e7366657272656400" # "token.transferred\0"
status_field_hex="7374617475735f636f6465" # "status_code", a field of wallet.callback only
transfer_method_id=0x2f7d5d4339128148 # token.transfer

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

# A stand address "dapp:account" as 128 hex digits.
address_hex() {
  printf '%s' "${1/:/}"
}

holder() {
  printf '%s%s' "$1" "$(le_hex "$2" 8)"
}

# The token state read from the token account itself.
expect_state() {
  local name="$1" expected="$(le_hex 1000000 8)$(le_hex "$2" 4)00000000$3"
  if [[ ${#expected} -lt 1184 ]]; then
    expected+="$(printf '%0*d' "$((1184 - ${#expected}))" 0)"
  fi
  run inspect --account "$address" >"${work}/${name}-state.json"
  [[ "$(jq -r .raw_state "${work}/${name}-state.json")" == "$expected" ]] ||
    fail "unexpected state after ${name}"
}

# The token's exact status for the delivery, from the one wallet.callback
# event of the call: 13 bytes after the field name, trailing zeros not stored.
expect_status() {
  local events value
  events="$(jq -r --arg field "$status_field_hex" '[.events[] | select(contains($field))] | .[]' "${work}/$1.json")"
  [[ -n "$events" && "$events" != *$'\n'* ]] || fail "expected one wallet.callback event in $1"
  value="${events#*"$status_field_hex"}"
  value="${value:26:16}0000000000000000"
  [[ "${value:0:16}" == "$(le_hex "$2" 8)" ]] || fail "expected token status $2 in $1"
}

# Every external call goes to the signer's own wallet; the wallet sends
# token.transfer as an internal message with a callback request.
transfer() {
  local name="$1" identity="$2" recipient="$3" amount="$4" time="$5"
  local wallet="${wallets[$identity]}"
  local body="${recipient}$(le_hex "$amount" 8)"
  local send="$(address_hex "$address")$(le_hex "$transfer_method_id" 8)$(le_hex 10000 16)0100${body}"
  run call --address "$wallet" --method wallet.send --args-json "\"0x${send}\"" \
    --identity "$identity" --extra-currencies '' \
    --block-time "$time" --created-at "$time" --expire-at "$((time + 3600))" \
    --fuel-fee-shells 10000 >"${work}/${name}.json"
  [[ "$(jq -r .status "${work}/${name}.json")" == ok ]] || fail "wallet send ${name}"
}

# A rejected external returns the named admission error and changes nothing.
expect_rejected() {
  local name="$1" error="$2"
  shift 2
  if run call "$@" >"${work}/${name}.json"; then
    fail "${name} was accepted"
  fi
  jq -e --arg error "$error" '.status == "error" and (tostring | contains($error))' \
    "${work}/${name}.json" >/dev/null || fail "expected ${error} in ${name}"
}

"$build" build --manifest-path "${wallet_dir}/Cargo.toml" >/dev/null
"$build" build --manifest-path "${example_dir}/Cargo.toml" >/dev/null

run init --dapp-id "$dapp_id" >"${work}/init.json"

# Deploys new code from artifacts, or committed code given as
# "--package-id <id> --selected-code-id <id>".
deploy() {
  local code="$1" salt="$2" owner="$3" init="$4" source
  if [[ -d "$code" ]]; then
    source=(--wasm "${code}/contract.wasm" --acab "${code}/contract.acab" --descriptor "${code}/contract.descriptor")
  else
    read -r -a source <<<"$code"
  fi
  local options=(
    deploy
    "${source[@]}"
    --salt-hex "$salt" --owner-id "$owner"
    --init-json "$init"
    --block-time 1800000000 --created-at 1800000000 --expire-at 1800003600
    --fuel-fee-shells 10000
  )
  run "${options[@]}" --quote-address >"${work}/quote-${salt}.json"
  [[ "$(jq -r .quoted "${work}/quote-${salt}.json")" == true ]] || fail "deploy quote ${salt}"
  deployed="$(jq -r .address "${work}/quote-${salt}.json")"
  run fund-user --address "$deployed" --shells 10000000 >/dev/null
  run "${options[@]}" >"${work}/deploy-${salt}.json"
  [[ "$(jq -r .status "${work}/deploy-${salt}.json")" == ok ]] || fail "deploy ${salt}"
}

# One keyed wallet per holder identity; later wallets reuse the committed code.
declare -A wallets
wallet_code="$wallet_artifacts"
for identity in developer user package-owner; do
  key="$(jq -r ".identities[\"${identity/-/_}\"]" "${work}/init.json")"
  deploy "$wallet_code" "$(printf 'wallet-%s' "$identity" | od -An -tx1 | tr -d ' \n')" "$key" null
  wallets[$identity]="$deployed"
  if [[ $identity == developer ]]; then
    run inspect >"${work}/wallet-code.json"
    wallet_code="--package-id $(jq -r '.registry_entries[] | select(.key | startswith("01")) | .key[2:]' "${work}/wallet-code.json")"
    wallet_code+=" --selected-code-id $(jq -r '.prepared_selection_ids[0]' "${work}/wallet-code.json")"
  fi
done
developer="$(address_hex "${wallets[developer]}")"
user="$(address_hex "${wallets[user]}")"

# The token has no owner key; init credits the supply to the developer wallet.
deploy "$artifacts" 66756e6769626c652d746f6b656e \
  "$(jq -r .identities.developer "${work}/init.json")" "\"0x${developer}\""
address="$deployed"
expect_state minted 1 "$(holder "$developer" 1000000)"

run events >"${work}/events-after-deploy.json"
mint="$(le_hex 0 64)${developer}"
jq -e --arg name "$transferred_name_hex" --arg mint "$mint" \
  '[.events[] | select(contains($name) and contains($mint))] | length == 1' \
  "${work}/events-after-deploy.json" >/dev/null || fail "mint event"

transfer transfer developer "$user" 250000 1800000010
expect_status transfer 0
expect_state transfer 2 "$(holder "$developer" 750000)$(holder "$user" 250000)"

# An overdraft is rejected and leaves both balances unchanged.
transfer overdraft user "$developer" 250001 1800000020
expect_status overdraft 1303
expect_state overdraft 2 "$(holder "$developer" 750000)$(holder "$user" 250000)"

transfer transfer-back user "$developer" 50000 1800000030
expect_status transfer-back 0
expect_state transfer-back 2 "$(holder "$developer" 800000)$(holder "$user" 200000)"

# Self-transfer preserves balances. Zero is rejected without an event or state effect.
transfer self-transfer developer "$developer" 7 1800000040
expect_status self-transfer 0
expect_state self-transfer 2 "$(holder "$developer" 800000)$(holder "$user" 200000)"
transfer zero developer "$user" 0 1800000050
expect_status zero 1301
expect_state zero 2 "$(holder "$developer" 800000)$(holder "$user" 200000)"

# A wallet that holds no tokens cannot spend another holder's balance.
transfer foreign-wallet package-owner "$user" 1 1800000055
expect_status foreign-wallet 1303
expect_state foreign-wallet 2 "$(holder "$developer" 800000)$(holder "$user" 200000)"

# Six additional recipients fill the fixed eight-slot ledger.
additional=""
remaining=800000
for index in 1 2 3 4 5 6; do
  recipient="$(printf '%0128x' "$index")"
  transfer "holder-${index}" developer "$recipient" "$index" "$((1800000050 + index * 10))"
  expect_status "holder-${index}" 0
  remaining=$((remaining - index))
  additional+="$(holder "$recipient" "$index")"
  expect_state "holder-${index}" "$((2 + index))" "$(holder "$developer" "$remaining")$(holder "$user" 200000)$additional"
done
transfer capacity developer "$(printf '%0128x' 7)" 1 1800000120
expect_status capacity 1304
expect_state capacity 8 "$(holder "$developer" 799979)$(holder "$user" 200000)$additional"

# The node admits an external only under the destination's owner key and
# without value: another identity's wallet, the keyless token and an external
# carrying value are all rejected before execution.
run inspect >"${work}/before-rejections.json"
call_options=(--block-time 1800000130 --created-at 1800000130 --expire-at 1800003730 --fuel-fee-shells 10000)
expect_rejected foreign-key AccountKeyNotAllowed --address "${wallets[developer]}" \
  --method wallet.receive --args-json null --identity user --extra-currencies '' "${call_options[@]}"
expect_rejected direct-token AccountKeyNotAllowed --address "$address" --method token.transfer \
  --args-json "\"0x${user}$(le_hex 1 8)\"" --identity developer --extra-currencies '' "${call_options[@]}"
expect_rejected external-value ExternalValueNotAllowed --address "${wallets[developer]}" \
  --method wallet.receive --args-json null --identity developer --extra-currencies '2:1' "${call_options[@]}"
expect_state rejected-externals 8 "$(holder "$developer" 799979)$(holder "$user" 200000)$additional"
run inspect >"${work}/after-rejections.json"
jq -e --slurpfile before "${work}/before-rejections.json" \
  '.account_state_root == $before[0].account_state_root and .cumulative_fee_shells == $before[0].cumulative_fee_shells
   and .accepted_input_count == $before[0].accepted_input_count' \
  "${work}/after-rejections.json" >/dev/null || fail "a rejected external changed the stand"

run events >"${work}/events.json"
jq -e --arg name "$transferred_name_hex" '[.events[] | select(contains($name))] | length == 10' \
  "${work}/events.json" >/dev/null || fail "expected ten token.transferred events"

# The capacity receipt covers only the wallet's external; its internal
# deliveries run after it, so replay is compared with the final stand.
run replay >"${work}/replay.json"
jq -e --slurpfile last "${work}/after-rejections.json" \
  '.status == "ok" and .account_state_root == $last[0].account_state_root
   and .cumulative_fee_shells == $last[0].cumulative_fee_shells
   and .accepted_input_count == $last[0].accepted_input_count' \
  "${work}/replay.json" >/dev/null || fail "replay diverged from the recorded journal"

echo "fungible-token stand e2e passed"
