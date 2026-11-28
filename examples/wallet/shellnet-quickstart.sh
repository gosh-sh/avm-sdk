#!/usr/bin/env bash
# Shellnet quickstart: gets test SHELL from the shellnet SHELL minter for a new
# wallet address, deploys the wallet example there, sends 100 SHELL from the
# wallet to itself with a callback and reads the wallet's balance and its
# wallet.callback event back from the shellnet Block Manager.
set -euo pipefail

example_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
build="${AVM_CONTRACT_BUILD:-avm-contract-build}"
stand="${AVM_LOCAL_STAND:-avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-${sdk_home}/current/profiles/wasmtime-release-profile.v1}"
node="${AVM_SHELLNET_NODE:-http://94.156.178.12:8602}"
bm="${AVM_SHELLNET_BM:-http://shellnet.ackinacki.org:80}"
reader="${AVM_SHELLNET_READER:-https://shellnet.ackinacki.org}"
# The work directory keeps the key files and every command's JSON output; a
# new directory gives new keys and so a new wallet address.
work="${AVM_SHELLNET_WORK:-$(mktemp -d)}"
minter=51f87c509122d9596d67f96e05f84bfdeb506ffa7cd70426a2d9109be3409896
minter="${minter}:${minter}"
mint_amount=10000000
receive_method_id=6078841212400898889 # wallet.receive, 0x545c8dab459b5349
artifacts="${example_dir}/target/avm"
read_tool=(python3 -I "${example_dir}/shellnet_read.py")

fail() {
  echo "shellnet quickstart failed: $*" >&2
  exit 1
}

mkdir -p "$work"
chmod 700 "$work"
# New random keys on the first run; keep them private.
for role in owner package-owner; do
  if [[ ! -s "${work}/${role}.key" ]]; then
    (umask 077 && python3 -c 'import secrets; print(secrets.token_hex(32))' >"${work}/${role}.key")
  fi
done

consumed() {
  [[ "$(jq -r .outcome.status "$1")" == consumed ]] || fail "${1##*/}: $(jq -c .outcome "$1")"
}

# Waits up to $2 seconds until the account matches the jq condition $3.
wait_account() {
  local deadline=$((SECONDS + $2))
  while ((SECONDS < deadline)); do
    "${read_tool[@]}" account "$reader" "$1" >"${work}/account.json"
    if jq -e "$3" "${work}/account.json" >/dev/null; then
      return 0
    fi
    sleep 5
  done
  fail "account $1 does not match $3: $(jq -c . "${work}/account.json")"
}

"$build" build --manifest-path "${example_dir}/Cargo.toml" >/dev/null

# 1. Quote the wallet address; the wallet's owner key is the public key of owner.key.
deploy=(network-deploy --endpoint "$node" --wasmtime-profile "$profile" --key "${work}/owner.key"
  --wasm "${artifacts}/contract.wasm" --acab "${artifacts}/contract.acab"
  --descriptor "${artifacts}/contract.descriptor" --salt-hex 01 --init-json null
  --fuel-fee-shells 100000)
publish=(--package-owner-key "${work}/package-owner.key")
"$stand" "${deploy[@]}" "${publish[@]}" --quote-address >"${work}/quote.json"
wallet="$(jq -r .address "${work}/quote.json")"
echo "wallet address ${wallet}"
"${read_tool[@]}" account "$reader" "$wallet" >"${work}/account.json"
[[ "$(jq -r .state "${work}/account.json")" != active ]] ||
  fail "the wallet of these keys is already deployed; use a new AVM_SHELLNET_WORK"

# 2. Mint test SHELL to the address. The minter takes a call signed with any key.
args="$(jq -cn --arg destination "$wallet" --argjson amount "$mint_amount" \
  --argjson method_id "$receive_method_id" \
  '{destination: $destination, amount: $amount, fuel_fee_shells: 1, method_id: $method_id}')"
"$stand" network-call --endpoint "$node" --bm-endpoint "$bm" --wasmtime-profile "$profile" \
  --key "${work}/owner.key" --address "$minter" --acab "${example_dir}/shellnet-shell-minter.acab" \
  --method shellnet.mint --args-json "$args" --fuel-fee-shells 100000 >"${work}/mint.json"
consumed "${work}/mint.json"
wait_account "$wallet" 120 ".balances[\"2\"] == \"${mint_amount}\""
echo "minted ${mint_amount} SHELL"

# 3. Deploy the wallet. The wallet code of this release is already published
#    on shellnet, so the deploy names it (--committed) and completes at once;
#    a deploy that carries published code again is refused. For a changed
#    wallet, set AVM_SHELLNET_PUBLISH=1: the deploy publishes the new code in
#    three phases, and init runs 120 seconds of block time after the block
#    that published it is final.
if [[ "${AVM_SHELLNET_PUBLISH:-0}" == 1 ]]; then
  wait_seconds=600
else
  publish=(--committed)
  wait_seconds=120
fi
# With --bm-endpoint the deploy waits until the account is active and exits
# with status 2 when the network refused the deploy.
"$stand" "${deploy[@]}" "${publish[@]}" --bm-endpoint "$bm" >"${work}/deploy.json" ||
  fail "deploy not confirmed: $(jq -c '.confirmation // .outcome' "${work}/deploy.json")"
consumed "${work}/deploy.json"
[[ "$(jq -r .confirmation.status "${work}/deploy.json")" == active ]] ||
  fail "deploy not confirmed: $(jq -c .confirmation "${work}/deploy.json")"
wait_account "$wallet" "$wait_seconds" '.state == "active"'
[[ "$(jq -r .code_hash "${work}/account.json")" == "$(jq -r .code_hash "${work}/quote.json")" ]] ||
  fail "the wallet runs another code hash"
echo "deployed; balance $(jq -r '.balances["2"]' "${work}/account.json") SHELL"

# 4. Send 100 SHELL from the wallet to itself with a callback. wallet.send args:
#    destination, method id (u64 LE), message fuel_fee_shells (u128 LE),
#    callback flag, one extra entry (u32 LE id 2, u128 LE amount 100).
send_args="$(python3 -I -c 'import sys
dapp_id, account_id = sys.argv[1].split(":")
args = (bytes.fromhex(dapp_id + account_id) + (0x545c8dab459b5349).to_bytes(8, "little")
        + (0).to_bytes(16, "little") + bytes([1, 1]) + (2).to_bytes(4, "little")
        + (100).to_bytes(16, "little"))
print("\"0x" + args.hex() + "\"")' "$wallet")"
"$stand" network-call --endpoint "$node" --bm-endpoint "$bm" --wasmtime-profile "$profile" \
  --key "${work}/owner.key" --address "$wallet" --acab "${artifacts}/contract.acab" \
  --method wallet.send --args-json "$send_args" --fuel-fee-shells 100000 >"${work}/send.json"
consumed "${work}/send.json"

# 5. Read the wallet.callback event: the status code of the delivery and the
#    returned currencies.
deadline=$((SECONDS + 120))
until "${read_tool[@]}" events "$reader" "${wallet#*:}" wallet.callback >"${work}/events.json" &&
  [[ "$(jq length "${work}/events.json")" -ge 1 ]]; do
  ((SECONDS < deadline)) || fail "no wallet.callback event"
  sleep 5
done
[[ "$(jq -r '.[-1].fields[] | select(.name == "status_code") | .value' "${work}/events.json")" == 0 ]] ||
  fail "the callback reports a failed delivery: $(jq -c '.[-1]' "${work}/events.json")"
"${read_tool[@]}" account "$reader" "$wallet" >"${work}/account.json"
echo "callback status 0; balance $(jq -r '.balances["2"]' "${work}/account.json") SHELL"
echo "shellnet quickstart passed; outputs in ${work}"
