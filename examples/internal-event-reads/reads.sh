#!/usr/bin/env bash
# Internal event reads across DApps: an emitter publishes parameters as
# internal events, a caller sends a message that declares reads of the
# emitter's rings, and the reader receives exactly those rings in its input.
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
work="${AVM_EVENT_READS_WORK:-}"
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
      echo "usage: reads.sh [--builder COMMAND] [--stand COMMAND] [--profile FILE] [--out DIRECTORY]"
      exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ -z $work ]]; then work="$(mktemp -d)"; fi
mkdir -p "$work"
[[ ! -e $work/source && ! -e $work/reads.state ]] || { echo "choose a fresh --out directory" >&2; exit 2; }
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
target = output / "examples/internal-event-reads"
target.mkdir(parents=True)
for name in ("Cargo.toml", "Cargo.lock", "avm-contract.toml"):
    shutil.copy2(example / name, target / name)
shutil.copytree(example / "src", target / "src")
PY
here="${work}/source/examples/internal-event-reads"
echo "Internal event reads output: $work"
artifacts="${here}/target/avm"
logical_time=1800000000

fail() {
  echo "reads failed: $*" >&2
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
# Each external deploy creates the first contract of a new DApp. Sets $deployed.
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

# A signed external call. With an expected status other than 0 the call must
# fail with exactly that status.
call() {
  local state="$1" address="$2" method="$3" args="$4" status="$5" receipt
  receipt="${work}/${method}-${logical_time}.json"
  tick
  if [[ "$status" == 0 ]]; then
    run "$state" call --address "$address" --method "$method" --args-json "$args" \
      --identity user --extra-currencies '' "${timestamps[@]}" >"$receipt" ||
      fail "$method failed"
  else
    run "$state" call --address "$address" --method "$method" --args-json "$args" \
      --identity user --extra-currencies '' "${timestamps[@]}" >"$receipt" 2>/dev/null &&
      fail "$method succeeded, expected status $status"
    jq -e --argjson status "$status" '.status_code == $status' "$receipt" >/dev/null ||
      fail "$method status $(jq -r .status_code "$receipt"), expected $status"
  fi
}

# Hex state of an account, without executing or mutating it. Sets $hex.
read_state() {
  hex="$(run "$1" inspect --account "$2" | jq -r .raw_state)"
}

# Byte range [start, end) of a hex string.
slice() {
  echo "${1:$(($2 * 2)):$((($3 - $2) * 2))}"
}

# A u64 as 16 little-endian hex digits.
le64() {
  printf '%016x' "$1" | fold -w2 | tac | tr -d '\n'
}

# One ring of the emitter: its address and an event_class.
ring() {
  echo "${emitter/:/}$(le64 "$1")"
}

# The reader state: reads received, then per read the event count and the
# newest value.
expect_reader() {
  local reads="$1" count0="$2" value0="$3" count1="$4" value1="$5"
  read_state "$state" "$reader"
  [[ "$(slice "$hex" 0 4)" == "$(le64 "$reads" | cut -c1-8)" ]] || fail "reads received"
  [[ "$(slice "$hex" 4 13)" == "$(printf '%02x' "$count0")$(le64 "$value0")" ]] || fail "first ring"
  [[ "$(slice "$hex" 13 22)" == "$(printf '%02x' "$count1")$(le64 "$value1")" ]] || fail "second ring"
}

settled() {
  run "$state" inspect | jq -e '.queue.message_count == 0' >/dev/null || fail "queue not empty"
  run "$state" replay | jq -e '.status == "ok"' >/dev/null || fail "replay"
}

"$build" build --manifest-path "${here}/Cargo.toml" >/dev/null

state="${work}/reads.state"
run "$state" init >/dev/null
cli_deploy "$state" 656d6974746572 \
  --wasm "${artifacts}/contract.wasm" --acab "${artifacts}/contract.acab" \
  --descriptor "${artifacts}/contract.descriptor"
emitter="$deployed"
run "$state" inspect >"${work}/inspect.json"
selected_code_id="$(jq -r '.prepared_selection_ids[0]' "${work}/inspect.json")"
package_id="$(jq -r '.registry_entries[] | select(.key | startswith("01")) | .key[2:]' "${work}/inspect.json")"
cli_deploy "$state" 63616c6c6572 --package-id "$package_id" --selected-code-id "$selected_code_id"
caller="$deployed"
cli_deploy "$state" 726561646572 --package-id "$package_id" --selected-code-id "$selected_code_id"
reader="$deployed"
[[ "${emitter%%:*}" != "${caller%%:*}" && "${caller%%:*}" != "${reader%%:*}" &&
  "${reader%%:*}" != "${emitter%%:*}" ]] || fail "the three accounts must be in three DApps"
echo "emitter, caller and reader deployed in three DApps"

# The emitter publishes two values of class 7 and one of class 9.
for event in "7 11" "7 22" "9 33"; do
  read -r class value <<<"$event"
  call "$state" "$emitter" params.set "\"0x$(le64 "$class")$(le64 "$value")\"" 0
done

# The caller declares reads of rings 7 and 9 for its message to the reader.
call "$state" "$caller" params.ask "\"0x${reader/:/}$(ring 7)$(ring 9)\"" 0
expect_reader 1 2 22 1 33
settled
echo "declared reads of rings 7 and 9: the reader received 2 and 1 events, newest 22 and 33, queue empty, replay ok"

# A ring without events is an empty result.
call "$state" "$caller" params.ask "\"0x${reader/:/}$(ring 8)$(ring 9)\"" 0
expect_reader 2 0 0 1 33
settled
echo "declared read of ring 8 without events: an empty result, replay ok"

# A ring keeps the last five events of its class, oldest first.
for value in 100 101 102 103 104; do
  call "$state" "$emitter" params.set "\"0x$(le64 7)$(le64 "$value")\"" 0
done
call "$state" "$caller" params.ask "\"0x${reader/:/}$(ring 7)$(ring 9)\"" 0
expect_reader 3 5 104 1 33
settled
echo "ring 7 after seven events: the last five, newest 104, replay ok"

# Reads must be strictly ascending: the SDK does not write such an output, and
# the caller's call ends with status 43 (VM_ABI_OUTPUT_MISSING).
call "$state" "$caller" params.ask "\"0x${reader/:/}$(ring 9)$(ring 7)\"" 43
expect_reader 3 5 104 1 33
settled
echo "unsorted reads: status 43, nothing sent, reader unchanged, replay ok"

# Reads arrive only with an internal message whose sender declared them.
call "$state" "$reader" params.read "\"0x$(ring 7)\"" 1603
expect_reader 3 5 104 1 33
settled
echo "PASS: declared internal event reads across DApps, empty and full rings, rejected unsorted reads, replay"
