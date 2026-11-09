#!/usr/bin/env bash
# Builds the wallet example, deploys two wallets on a fresh local stand and
# checks the owner key rule, the no-value rule for externals, value sends with
# their callback event, the internal-caller rejection, key rotation and replay.
set -euo pipefail

example_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
build="${AVM_CONTRACT_BUILD:-avm-contract-build}"
stand="${AVM_LOCAL_STAND:-avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
work="${AVM_WALLET_WORK:-$(mktemp -d)}"
mkdir -p "$work"
if [[ -z ${AVM_WALLET_WORK:-} ]]; then
  trap 'python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1])" "$work"' EXIT
fi
state="${work}/wallet.state"
artifacts="${example_dir}/target/avm"
dapp_id="2222222222222222222222222222222222222222222222222222222222222222"
time=1800000000

fail() {
  echo "wallet stand e2e failed: $*" >&2
  exit 1
}

run() {
  "$stand" "$1" --state "$state" --wasmtime-profile "$profile" "${@:2}"
}

# Little-endian hex of an unsigned integer with the given byte width.
le_hex() {
  printf "%0$(($2 * 2))x" "$1" | fold -w2 | tac | tr -d '\n'
}

method_le() {
  le_hex "$1" 8
}

# wallet.send args: destination, method id, fuel_fee_shells, callback flag,
# then "id:amount" extra entries in ascending id order. The stand adds the
# u32 length prefix of the ABI bytes type.
send_args() {
  local destination="$1" method="$2" fuel="$3" callback="$4" extra="" count=0 entry
  shift 4
  for entry in "$@"; do
    extra+="$(le_hex "${entry%%:*}" 4)$(le_hex "${entry##*:}" 16)"
    count=$((count + 1))
  done
  printf '"0x%s%s%s%s%s%s"' "${destination/:/}" "$(method_le "$method")" "$(le_hex "$fuel" 16)" \
    "$callback" "$(le_hex "$count" 1)" "$extra"
}

call() {
  local address="$1" method="$2" args="$3" identity="$4" value="${5:-}"
  time=$((time + 10))
  run call --address "$address" --method "$method" --args-json "$args" --identity "$identity" \
    --extra-currencies "$value" --block-time "$time" --created-at "$time" \
    --expire-at "$((time + 3600))" --fuel-fee-shells 100000
}

expect_status() {
  [[ "$(jq -r '.raw_executor_output[8:16]' "$1")" == "$(le_hex "$2" 4)" ]] ||
    fail "expected status $2 in ${1##*/}"
}

expect_rejected() {
  local receipt="$1" reason="$2"
  [[ "$(jq -r .status "$receipt")" == error ]] || fail "${receipt##*/} was not rejected"
  jq -r .error "$receipt" | grep -q "$reason" || fail "expected $reason in ${receipt##*/}"
}

expect_raw_state() {
  [[ "$(run inspect --account "$1" | jq -r .raw_state)" == "$2" ]] || fail "unexpected state of $1"
}

# The callback receipt of a send carries one wallet.callback event: the
# destination's exact status code and, when given, the returned currencies.
# In the stand's event bytes the u64 status follows the field name and a
# 13-byte field header; trailing zero bytes of the event are not stored.
expect_callback() {
  local receipt="$1" status_code="$2" returned="${3:-}" event value
  [[ "$(jq '.events | length' "$receipt")" == 1 ]] || fail "no wallet.callback event in ${receipt##*/}"
  event="$(jq -r '.events[0]' "$receipt")"
  [[ "$event" == *7374617475735f636f6465* ]] || fail "no status_code field in ${receipt##*/}"
  value="${event#*7374617475735f636f6465}"
  value="${value:26:16}0000000000000000"
  [[ "${value:0:16}" == "$(le_hex "$status_code" 8)" ]] ||
    fail "callback status $status_code in ${receipt##*/}"
  if [[ -n "$returned" ]]; then
    [[ "$event" == *"$(le_hex "${returned%%:*}" 4)$(le_hex "${returned##*:}" 16)"* ]] ||
      fail "callback did not return $returned in ${receipt##*/}"
  fi
}

account_root() {
  run inspect | jq -r .account_state_root
}

"$build" build --manifest-path "${example_dir}/Cargo.toml" >/dev/null

run init --dapp-id "$dapp_id" >"${work}/init.json"
developer="$(jq -r .identities.developer "${work}/init.json")"
user="$(jq -r .identities.user "${work}/init.json")"
timestamps=(--block-time "$time" --created-at "$time" --expire-at "$((time + 3600))" --fuel-fee-shells 10000)

# The user's wallet: a new package, funded with SHELL and 500 units of ECC 0.
deploy_a=(deploy --wasm "${artifacts}/contract.wasm" --acab "${artifacts}/contract.acab"
  --descriptor "${artifacts}/contract.descriptor" --salt-hex 01 --init-json null
  --owner-id "$user" "${timestamps[@]}")
wallet_a="$(run "${deploy_a[@]}" --quote-address | jq -r .address)"
run fund-user --address "$wallet_a" --shells 10000000 --extra-currencies 0:500 >/dev/null
[[ "$(run "${deploy_a[@]}" | jq -r .status)" == ok ]] || fail "deploy of the user wallet"
expect_raw_state "$wallet_a" "$user"

# The developer's wallet over the committed code of the same package.
run inspect >"${work}/inspect.json"
selected_code_id="$(jq -r '.prepared_selection_ids[0]' "${work}/inspect.json")"
package_id="$(jq -r '.registry_entries[] | select(.key | startswith("01")) | .key[2:]' "${work}/inspect.json")"
deploy_b=(deploy --package-id "$package_id" --selected-code-id "$selected_code_id" --salt-hex 02
  --init-json null --owner-id "$developer" "${timestamps[@]}")
wallet_b="$(run "${deploy_b[@]}" --quote-address | jq -r .address)"
run fund-user --address "$wallet_b" --shells 10000000 >/dev/null
[[ "$(run "${deploy_b[@]}" | jq -r .status)" == ok ]] || fail "deploy of the developer wallet"
expect_raw_state "$wallet_b" "$developer"

receive=0x545c8dab459b5349
send=0x4378f9971132d918
unknown=0x1122334455667788

# Only the owner key signs an external call, and an external carries no value.
root="$(account_root)"
call "$wallet_a" wallet.receive null developer >"${work}/foreign-key.json" 2>/dev/null || true
expect_rejected "${work}/foreign-key.json" AccountKeyNotAllowed
call "$wallet_a" wallet.receive null user 2:5 >"${work}/external-value.json" 2>/dev/null || true
expect_rejected "${work}/external-value.json" ExternalValueNotAllowed
[[ "$(account_root)" == "$root" ]] || fail "a rejected external changed the state"

# A send moves value by an internal message; the callback reports the status.
call "$wallet_a" wallet.send "$(send_args "$wallet_b" "$receive" 1000 01 0:100)" user >"${work}/send.json"
expect_status "${work}/send.json" 0
expect_callback "${work}/send.json" 0
expect_raw_state "$wallet_a" "$user"

# A failed delivery returns the value through the callback with the exact code.
call "$wallet_a" wallet.send "$(send_args "$wallet_b" "$unknown" 1000 01 0:100)" user >"${work}/send-failed.json"
expect_status "${work}/send-failed.json" 0
expect_callback "${work}/send-failed.json" 1704 0:100

# Malformed args fail the send itself; send and set_owner_key need an external caller.
call "$wallet_a" wallet.send "\"0x$(printf '%0176d' 0)0200\"" user >"${work}/send-invalid.json"
expect_status "${work}/send-invalid.json" 1702
call "$wallet_b" wallet.send \
  "$(send_args "$wallet_a" "$send" 1000 01)" developer >"${work}/internal-send.json"
expect_status "${work}/internal-send.json" 0
expect_callback "${work}/internal-send.json" 1701

# Rotation: the new key authorizes the next calls, the old one no longer does.
call "$wallet_a" wallet.set_owner_key "\"0x${developer}\"" user >"${work}/rotate.json"
expect_status "${work}/rotate.json" 0
expect_raw_state "$wallet_a" "$developer"
call "$wallet_a" wallet.receive null user >"${work}/old-key.json" 2>/dev/null || true
expect_rejected "${work}/old-key.json" AccountKeyNotAllowed
call "$wallet_a" wallet.receive null developer >"${work}/new-key.json"
expect_status "${work}/new-key.json" 0

# A zero key leaves the wallet keyless: every external call is rejected.
call "$wallet_a" wallet.set_owner_key "\"0x$(printf '%064d' 0)\"" developer >"${work}/keyless.json"
expect_status "${work}/keyless.json" 0
for identity in developer user package-owner; do
  call "$wallet_a" wallet.receive null "$identity" >"${work}/keyless-${identity}.json" 2>/dev/null || true
  expect_rejected "${work}/keyless-${identity}.json" AccountKeyNotAllowed
done

[[ "$(run replay | jq -r .status)" == ok ]] || fail "replay"
echo "wallet stand e2e passed"
