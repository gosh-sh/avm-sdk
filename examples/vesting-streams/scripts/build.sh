#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${1:-$root/.runtime/avm-sdk-linux-x86_64}"
optimizer="${2:-$root/.runtime/binaryen-version_133/bin/wasm-opt}"
sdk="$(cd "$sdk" && pwd)"
cd "$root"
mkdir -p evidence
export CARGO_PROFILE_RELEASE_OPT_LEVEL=s
export CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=symbols
export RUSTC_WRAPPER="$root/scripts/remap-rustc.sh"
for contract in vesting receiver; do
  "$sdk/bin/avm-contract-build" build --manifest-path "contracts/$contract/Cargo.toml" | tee "evidence/build-$contract.txt"
done
python3 scripts/sync_ids.py --check
python3 scripts/optimize.py --wasm-opt "$optimizer"
