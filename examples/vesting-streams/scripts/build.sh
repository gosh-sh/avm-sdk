#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
sdk="${1:-${AVM_SDK_HOME:-$HOME/.local/share/avm-sdk}/current}"
optimizer="${2:?Pass the checksum-pinned Binaryen 133 wasm-opt executable}"
evidence="${AVM_EVIDENCE_DIR:?Set AVM_EVIDENCE_DIR to an external scratch directory}"
evidence="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$evidence")"
case "$evidence/" in "$root/"*) echo 'Evidence must be outside the example checkout' >&2; exit 2;; esac
mkdir -p "$evidence"
: "${TMPDIR:?Set TMPDIR to external scratch storage}"
export TMPDIR TMP="$TMPDIR" TEMP="$TMPDIR"
sdk="$(cd "$sdk" && pwd -P)"
builder="${AVM_CONTRACT_BUILD:-$sdk/bin/avm-contract-build}"
cd "$root"
export CARGO_PROFILE_RELEASE_OPT_LEVEL=s
export CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=symbols
for contract in vesting receiver; do
  "$builder" build --manifest-path "contracts/$contract/Cargo.toml" | tee "$evidence/build-$contract.txt"
done
python3 -B scripts/sync_ids.py --check
python3 -B scripts/optimize.py --wasm-opt "$optimizer" --output "$evidence/optimization.json"
