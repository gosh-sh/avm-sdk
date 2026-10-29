#!/usr/bin/env bash
# Reproducible local-stand E2E journey for the multi-currency vault.
#
# Covers: deposit; withdraw -> Settle; withdraw -> Restore; owner-gated
# pause; two-step owner rotation; a WITHDRAW rejected because the vault's
# own SHELL balance is below MIN_OPERATIONAL_SHELL. Every assertion reads
# the vault's own DECODED state (never a receipt or an actor's own reply)
# via `../e2e-actor/`'s `AccountAddress`-targeted `vault.topup` probe (an
# always-available, harmless real method used only to get the vault's own
# current state echoed back in a call response -- `avm-local-stand inspect`
# does not expose per-account state bytes, only hashes/metadata).
#
# Uses only documented `avm-local-stand`/`avm-contract-build` commands and
# flags (`docs/cli-reference.md`); no invented flags. Exits non-zero on any
# failed assertion (`set -euo pipefail` plus explicit `fail` calls).
#
# Style note: no other example in this repository ships its own E2E
# script (`examples/*/`'s own directories hold only `Cargo.toml`/
# `Cargo.lock`/`avm-contract.toml`/`src/`; the repository's own
# `scripts/tests/packaged_lifecycle.py` drives the four official examples
# instead, hardcoded to their names). This script follows that same
# general shape (`avm-local-stand` CLI, `--block-time`/`--created-at`
# strictly increasing every call, the two-step "deploy -> funding_required
# -> fund-user -> deploy" dance, `jq`/python3 for JSON/byte handling) translated
# into a standalone shell script scoped to this one example.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
EXAMPLE_DIR="$REPO_ROOT/examples/multi-currency-vault"
ACTOR_DIR="$EXAMPLE_DIR/e2e-actor"

SDK_HOME="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
export TMPDIR="${TMPDIR:-${SDK_HOME}/tmp}"
mkdir -p "$TMPDIR"
STAND="${AVM_LOCAL_STAND:-${SDK_HOME}/current/bin/avm-local-stand}"
PROFILE="${AVM_WASMTIME_PROFILE:-${SDK_HOME}/current/profiles/wasmtime-release-profile.v1}"

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
STATE="$WORK_DIR/e2e.state"
# A fixed, obviously-synthetic test dapp id -- never a real identity.
DAPP_ID="3333333333333333333333333333333333333333333333333333333333333333"
# `expire_at` is held far in the future; `block_time`/`created_at` must
# strictly increase on every single stand invocation in this same state,
# or the stand rejects later calls (observed directly: reusing the same
# timestamp across many calls in one session produced
# "InvalidQueueOrder"). A plain monotonically-increasing counter is used
# instead of wall-clock time so this script's own output stays
# reproducible.
TICK=0
next_tick() { TICK=$((TICK + 1)); echo "$TICK"; }

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL: $1" >&2; }

run_stand() {
  local cmd="$1"; shift
  "$STAND" "$cmd" --state "$STATE" --wasmtime-profile "$PROFILE" "$@"
}

# Deploys `$1`=wasm `$2`=acab `$3`=descriptor `$4`=salt-hex `$5`=init-json
# (or `null`), returning the deployed address on stdout. Implements the
# documented two-step "funding_required -> fund-user -> redeploy" dance
# (`docs/cli-reference.md`; `scripts/tests/packaged_lifecycle.py`'s own
# `deploy` helper uses the identical pattern).
deploy_funded() {
  local wasm="$1" acab="$2" descriptor="$3" salt="$4" init_json="$5" shells="$6"
  local t1 out addr t2
  t1=$(next_tick)
  out=$(run_stand deploy --wasm "$wasm" --acab "$acab" --descriptor "$descriptor" \
    --salt-hex "$salt" --init-json "$init_json" \
    --block-time "$t1" --created-at "$t1" --expire-at 999999999 --fuel-fee-shells 1 2>/dev/null || true)
  addr=$(python3 -c "
import json, sys
try:
    d = json.loads(sys.argv[1])
    print(d.get('address', ''))
except Exception:
    print('')
" "$out")
  if [ -z "$addr" ]; then
    echo "deploy_funded: no address in first attempt: $out" >&2
    return 1
  fi
  run_stand fund-user --address "$addr" --shells "$shells" >/dev/null
  t2=$(next_tick)
  out=$(run_stand deploy --wasm "$wasm" --acab "$acab" --descriptor "$descriptor" \
    --salt-hex "$salt" --init-json "$init_json" \
    --block-time "$t2" --created-at "$t2" --expire-at 999999999 --fuel-fee-shells 1 2>/dev/null || true)
  if ! echo "$out" | python3 -c "
import json, sys
d = json.load(sys.stdin)
sys.exit(0 if d.get('status') == 'ok' else 1)
" <<<"$out"; then
    echo "deploy_funded: second deploy did not succeed: $out" >&2
    return 1
  fi
  echo "$addr"
}

# Reads the vault's own current decoded state (header fields this script
# needs) by calling its real `vault.topup` method with 1 SHELL -- a
# harmless, always-available real method whose response always carries
# `raw_state`, the vault's own post-call state bytes. Prints
# "next_withdrawal_id ledger_count pending_count admin_seq
# deposits_paused" space-separated.
vault_state_fields() {
  local vault_addr="$1" t out
  t=$(next_tick)
  out=$(run_stand call --address "$vault_addr" --method vault.topup --args-json null \
    --identity user --value-main 1 --extra-currencies "" \
    --block-time "$t" --created-at "$t" --expire-at 999999999 --fuel-fee-shells 1 2>/dev/null)
  python3 -c "
import json, sys
d = json.loads(sys.argv[1])
raw = bytes.fromhex(d['raw_state'])
flags = raw[69]
print(
    int.from_bytes(raw[78:82], 'little'),
    int.from_bytes(raw[82:86], 'little'),
    int.from_bytes(raw[86:90], 'little'),
    int.from_bytes(raw[74:78], 'little'),
    flags & 1,
)
" "$out"
}

forward_deposit_body() {
  python3 -c "
import sys
dapp = bytes.fromhex(sys.argv[1])
acct = bytes.fromhex(sys.argv[2])
method_id = int(sys.argv[3], 0)
body = dapp + acct + method_id.to_bytes(8, 'little') + (0).to_bytes(8, 'little')
print(body.hex())
" "$1" "$2" "$3"
}

forward_withdraw_body() {
  python3 -c "
import sys
dapp = bytes.fromhex(sys.argv[1])
acct = bytes.fromhex(sys.argv[2])
method_id = int(sys.argv[3], 0)
currency_tag = int(sys.argv[4])
currency_id = int(sys.argv[5])
amount = int(sys.argv[6])
body = (
    dapp + acct + method_id.to_bytes(8, 'little') + (0).to_bytes(8, 'little')
    + bytes([currency_tag]) + currency_id.to_bytes(4, 'little') + amount.to_bytes(16, 'little')
)
print(body.hex())
" "$1" "$2" "$3" "$4" "$5" "$6"
}

admin_body_propose_owner() {
  python3 -c "
import sys
seq = int(sys.argv[1])
new_owner = bytes.fromhex(sys.argv[2])
print((seq.to_bytes(4, 'little') + new_owner).hex())
" "$1" "$2"
}

admin_body_accept_owner() {
  python3 -c "import sys; print(int(sys.argv[1]).to_bytes(4, 'little').hex())" "$1"
}

admin_body_set_pause() {
  python3 -c "
import sys
seq = int(sys.argv[1])
flags = int(sys.argv[2])
print((seq.to_bytes(4, 'little') + bytes([flags])).hex())
" "$1" "$2"
}

echo "== Vault/actor E2E journey =="
echo "work dir: $WORK_DIR"

echo "-- init --"
INIT_JSON=$(run_stand init --dapp-id "$DAPP_ID")
DEVELOPER_PUBKEY=$(echo "$INIT_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['identities']['developer'])")
USER_PUBKEY=$(echo "$INIT_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['identities']['user'])")
echo "developer pubkey: $DEVELOPER_PUBKEY"
echo "user pubkey:      $USER_PUBKEY"

echo "-- build (vault A: owner = developer identity; vault B: default demo key, a genuinely different wasm -- the stand rejects a second deploy of the SAME wasm+acab+descriptor combo even under a new salt, observed directly as \"selection is already attached immutably to the package\"; actor, accept + reject variants) --"
AVM_EXAMPLE_VAULT_OWNER_PUBKEY="$DEVELOPER_PUBKEY" \
  avm-contract-build build --manifest-path "$EXAMPLE_DIR/Cargo.toml" >/dev/null
VAULT_A_WASM="$WORK_DIR/vault_a_wasm"; VAULT_A_ACAB="$WORK_DIR/vault_a_acab"; VAULT_A_DESCRIPTOR="$WORK_DIR/vault_a_descriptor"
cp "$EXAMPLE_DIR/target/avm/contract.wasm" "$VAULT_A_WASM"
cp "$EXAMPLE_DIR/target/avm/contract.acab" "$VAULT_A_ACAB"
cp "$EXAMPLE_DIR/target/avm/contract.descriptor" "$VAULT_A_DESCRIPTOR"

avm-contract-build build --manifest-path "$EXAMPLE_DIR/Cargo.toml" >/dev/null
VAULT_B_WASM="$WORK_DIR/vault_b_wasm"; VAULT_B_ACAB="$WORK_DIR/vault_b_acab"; VAULT_B_DESCRIPTOR="$WORK_DIR/vault_b_descriptor"
cp "$EXAMPLE_DIR/target/avm/contract.wasm" "$VAULT_B_WASM"
cp "$EXAMPLE_DIR/target/avm/contract.acab" "$VAULT_B_ACAB"
cp "$EXAMPLE_DIR/target/avm/contract.descriptor" "$VAULT_B_DESCRIPTOR"

avm-contract-build build --manifest-path "$ACTOR_DIR/Cargo.toml" >/dev/null
ACCEPT_WASM="$WORK_DIR/accept_wasm"; ACCEPT_ACAB="$WORK_DIR/accept_acab"; ACCEPT_DESCRIPTOR="$WORK_DIR/accept_descriptor"
cp "$ACTOR_DIR/target/avm/contract.wasm" "$ACCEPT_WASM"
cp "$ACTOR_DIR/target/avm/contract.acab" "$ACCEPT_ACAB"
cp "$ACTOR_DIR/target/avm/contract.descriptor" "$ACCEPT_DESCRIPTOR"
AVM_E2E_ACTOR_REJECT_PAYOUT=1 avm-contract-build build --manifest-path "$ACTOR_DIR/Cargo.toml" >/dev/null
REJECT_WASM="$WORK_DIR/reject_wasm"; REJECT_ACAB="$WORK_DIR/reject_acab"; REJECT_DESCRIPTOR="$WORK_DIR/reject_descriptor"
cp "$ACTOR_DIR/target/avm/contract.wasm" "$REJECT_WASM"
cp "$ACTOR_DIR/target/avm/contract.acab" "$REJECT_ACAB"
cp "$ACTOR_DIR/target/avm/contract.descriptor" "$REJECT_DESCRIPTOR"

echo "-- deploy vault A (well-funded) --"
VAULT_A_ADDR=$(deploy_funded "$VAULT_A_WASM" "$VAULT_A_ACAB" "$VAULT_A_DESCRIPTOR" "01" "null" 3000000)
VAULT_A_ACCT="${VAULT_A_ADDR##*:}"
echo "vault A: $VAULT_A_ADDR"

echo "-- deploy vault B (underfunded, below MIN_OPERATIONAL_SHELL = 1,500,000) --"
VAULT_B_ADDR=$(deploy_funded "$VAULT_B_WASM" "$VAULT_B_ACAB" "$VAULT_B_DESCRIPTOR" "02" "null" 50000)
VAULT_B_ACCT="${VAULT_B_ADDR##*:}"
echo "vault B: $VAULT_B_ADDR"

echo "-- deploy actor instances --"
ACCEPT_ADDR=$(deploy_funded "$ACCEPT_WASM" "$ACCEPT_ACAB" "$ACCEPT_DESCRIPTOR" "10" "null" 1000000)
echo "accept actor: $ACCEPT_ADDR"
REJECT_ADDR=$(deploy_funded "$REJECT_WASM" "$REJECT_ACAB" "$REJECT_DESCRIPTOR" "11" "null" 1000000)
echo "reject actor: $REJECT_ADDR"

DEPOSIT_METHOD_ID="0x4D43565400000001"
WITHDRAW_METHOD_ID="0x4D43565400000002"
CURRENCY_TAG_EXTRA=1

echo "-- step 1: DEPOSIT (accept actor -> vault A, currency 7, 5000 units) --"
BODY=$(forward_deposit_body "$DAPP_ID" "$VAULT_A_ACCT" "$DEPOSIT_METHOD_ID")
BEFORE=$(vault_state_fields "$VAULT_A_ADDR")
T=$(next_tick)
run_stand call --address "$ACCEPT_ADDR" --method actor.forward_deposit --args-json "\"0x${BODY}\"" \
  --identity user --value-main 0 --extra-currencies "7:5000" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null
AFTER=$(vault_state_fields "$VAULT_A_ADDR")
read -r _ LEDGER_BEFORE _ _ _ <<<"$BEFORE"
read -r _ LEDGER_AFTER _ _ _ <<<"$AFTER"
if [ "$LEDGER_AFTER" -gt "$LEDGER_BEFORE" ]; then
  pass "deposit: vault A ledger_count grew ($LEDGER_BEFORE -> $LEDGER_AFTER)"
else
  fail "deposit: vault A ledger_count did not grow ($LEDGER_BEFORE -> $LEDGER_AFTER)"
fi

echo "-- step 2: WITHDRAW (accept actor, 1000 of currency 7) -> expect Settle --"
BODY=$(forward_withdraw_body "$DAPP_ID" "$VAULT_A_ACCT" "$WITHDRAW_METHOD_ID" "$CURRENCY_TAG_EXTRA" 7 1000)
BEFORE=$(vault_state_fields "$VAULT_A_ADDR")
T=$(next_tick)
run_stand call --address "$ACCEPT_ADDR" --method actor.forward_withdraw --args-json "\"0x${BODY}\"" \
  --identity user --value-main 0 --extra-currencies "" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null
AFTER=$(vault_state_fields "$VAULT_A_ADDR")
read -r NWID_BEFORE _ PENDING_BEFORE _ _ <<<"$BEFORE"
read -r NWID_AFTER _ PENDING_AFTER _ _ <<<"$AFTER"
if [ "$NWID_AFTER" -eq "$((NWID_BEFORE + 1))" ] && [ "$PENDING_AFTER" -eq "$PENDING_BEFORE" ]; then
  pass "withdraw->Settle: next_withdrawal_id $NWID_BEFORE->$NWID_AFTER, pending_count stayed $PENDING_AFTER (resolved)"
else
  fail "withdraw->Settle: next_withdrawal_id $NWID_BEFORE->$NWID_AFTER, pending_count $PENDING_BEFORE->$PENDING_AFTER (expected +1 / unchanged)"
fi

echo "-- step 3: DEPOSIT (reject actor -> vault A, currency 9, 2000 units, funds the Restore test) --"
BODY=$(forward_deposit_body "$DAPP_ID" "$VAULT_A_ACCT" "$DEPOSIT_METHOD_ID")
T=$(next_tick)
run_stand call --address "$REJECT_ADDR" --method actor.forward_deposit --args-json "\"0x${BODY}\"" \
  --identity user --value-main 0 --extra-currencies "9:2000" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null

echo "-- step 4: WITHDRAW (reject actor, 500 of currency 9) -> expect Restore --"
BODY=$(forward_withdraw_body "$DAPP_ID" "$VAULT_A_ACCT" "$WITHDRAW_METHOD_ID" "$CURRENCY_TAG_EXTRA" 9 500)
BEFORE=$(vault_state_fields "$VAULT_A_ADDR")
T=$(next_tick)
run_stand call --address "$REJECT_ADDR" --method actor.forward_withdraw --args-json "\"0x${BODY}\"" \
  --identity user --value-main 0 --extra-currencies "" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null
AFTER=$(vault_state_fields "$VAULT_A_ADDR")
read -r NWID_BEFORE LEDGER_BEFORE PENDING_BEFORE _ _ <<<"$BEFORE"
read -r NWID_AFTER LEDGER_AFTER PENDING_AFTER _ _ <<<"$AFTER"
if [ "$NWID_AFTER" -eq "$((NWID_BEFORE + 1))" ] && [ "$PENDING_AFTER" -eq "$PENDING_BEFORE" ] && [ "$LEDGER_AFTER" -ge "$LEDGER_BEFORE" ]; then
  pass "withdraw->Restore: next_withdrawal_id $NWID_BEFORE->$NWID_AFTER, pending_count stayed $PENDING_AFTER, ledger_count $LEDGER_BEFORE->$LEDGER_AFTER (restored)"
else
  fail "withdraw->Restore: next_withdrawal_id $NWID_BEFORE->$NWID_AFTER, pending_count $PENDING_BEFORE->$PENDING_AFTER, ledger_count $LEDGER_BEFORE->$LEDGER_AFTER (unexpected)"
fi

echo "-- step 5: SET_PAUSE (owner-only, as developer identity) --"
FIELDS=$(vault_state_fields "$VAULT_A_ADDR")
read -r _ _ _ SEQ PAUSED_BEFORE <<<"$FIELDS"
BODY=$(admin_body_set_pause "$SEQ" 1)
T=$(next_tick)
run_stand call --address "$VAULT_A_ADDR" --method vault.set_pause --args-json "\"0x${BODY}\"" \
  --identity developer --value-main 0 --extra-currencies "" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null
FIELDS=$(vault_state_fields "$VAULT_A_ADDR")
read -r _ _ _ SEQ_AFTER PAUSED_AFTER <<<"$FIELDS"
if [ "$PAUSED_AFTER" -eq 1 ] && [ "$SEQ_AFTER" -eq "$((SEQ + 1))" ]; then
  pass "set_pause: deposits_paused 0->1, admin_seq $SEQ->$SEQ_AFTER"
else
  fail "set_pause: deposits_paused ended $PAUSED_AFTER, admin_seq $SEQ->$SEQ_AFTER"
fi
SEQ="$SEQ_AFTER"

echo "-- step 6: owner rotation (PROPOSE_OWNER as developer, ACCEPT_OWNER as user) --"
BODY=$(admin_body_propose_owner "$SEQ" "$USER_PUBKEY")
T=$(next_tick)
run_stand call --address "$VAULT_A_ADDR" --method vault.propose_owner --args-json "\"0x${BODY}\"" \
  --identity developer --value-main 0 --extra-currencies "" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null
FIELDS=$(vault_state_fields "$VAULT_A_ADDR")
read -r _ _ _ SEQ_AFTER_PROPOSE _ <<<"$FIELDS"

BODY=$(admin_body_accept_owner "$SEQ_AFTER_PROPOSE")
T=$(next_tick)
run_stand call --address "$VAULT_A_ADDR" --method vault.accept_owner --args-json "\"0x${BODY}\"" \
  --identity user --value-main 0 --extra-currencies "" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null
FIELDS=$(vault_state_fields "$VAULT_A_ADDR")
read -r _ _ _ SEQ_FINAL _ <<<"$FIELDS"
if [ "$SEQ_FINAL" -eq "$((SEQ + 2))" ]; then
  pass "owner rotation: admin_seq $SEQ->$SEQ_AFTER_PROPOSE->$SEQ_FINAL (propose then accept)"
else
  fail "owner rotation: admin_seq ended $SEQ_FINAL, expected $((SEQ + 2))"
fi

echo "-- step 7: DEPOSIT into vault B (underfunded), then WITHDRAW -> expect pre-write rejection (I17) --"
BODY=$(forward_deposit_body "$DAPP_ID" "$VAULT_B_ACCT" "$DEPOSIT_METHOD_ID")
T=$(next_tick)
run_stand call --address "$ACCEPT_ADDR" --method actor.forward_deposit --args-json "\"0x${BODY}\"" \
  --identity user --value-main 0 --extra-currencies "7:5000" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null

BEFORE=$(vault_state_fields "$VAULT_B_ADDR")
BODY=$(forward_withdraw_body "$DAPP_ID" "$VAULT_B_ACCT" "$WITHDRAW_METHOD_ID" "$CURRENCY_TAG_EXTRA" 7 1000)
T=$(next_tick)
run_stand call --address "$ACCEPT_ADDR" --method actor.forward_withdraw --args-json "\"0x${BODY}\"" \
  --identity user --value-main 0 --extra-currencies "" \
  --block-time "$T" --created-at "$T" --expire-at 999999999 --fuel-fee-shells 1 >/dev/null
AFTER=$(vault_state_fields "$VAULT_B_ADDR")
read -r NWID_BEFORE _ PENDING_BEFORE _ _ <<<"$BEFORE"
read -r NWID_AFTER _ PENDING_AFTER _ _ <<<"$AFTER"
if [ "$NWID_AFTER" -eq "$NWID_BEFORE" ] && [ "$PENDING_AFTER" -eq "$PENDING_BEFORE" ]; then
  pass "underfunded vault: withdraw rejected pre-write (next_withdrawal_id/pending_count unchanged at $NWID_AFTER/$PENDING_AFTER)"
else
  fail "underfunded vault: state changed (next_withdrawal_id $NWID_BEFORE->$NWID_AFTER, pending_count $PENDING_BEFORE->$PENDING_AFTER) -- expected no write at all"
fi

echo "== Summary: $PASS passed, $FAIL failed =="
if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
