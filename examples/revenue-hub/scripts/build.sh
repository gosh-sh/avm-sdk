#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${1:-$root/.runtime/avm-sdk-linux-x86_64}"
optimizer="${2:-$root/.runtime/binaryen-version_133/bin/wasm-opt}"
sdk="$(cd "$sdk" && pwd)"
cd "$root"
mkdir -p evidence
# The official builder compiles in isolated manifests; pin its Cargo profile
# explicitly instead of relying on inheritance from this workspace.
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3
export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=symbols
# SDK panic diagnostics otherwise embed the absolute checkout path in WASM.
# The pinned builder supplies its own encoded rustflags; use Cargo's compiler
# wrapper to add this independent path remap without replacing its VM flags.
export RUSTC_WRAPPER="$root/scripts/remap-rustc.sh"
for contract in vault receiver audit; do
  "$sdk/bin/avm-contract-build" build --manifest-path "contracts/$contract/Cargo.toml" | tee "evidence/build-$contract.txt"
done
python3 scripts/sync_ids.py --check
python3 scripts/optimize.py --wasm-opt "$optimizer"
