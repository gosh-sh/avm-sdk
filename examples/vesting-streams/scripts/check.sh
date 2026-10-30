#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
sdk="${1:-${AVM_SDK_HOME:-$HOME/.local/share/avm-sdk}/current}"
optimizer="${2:?Pass the checksum-pinned Binaryen 133 wasm-opt executable}"
evidence="${AVM_EVIDENCE_DIR:?Set AVM_EVIDENCE_DIR to an external scratch directory}"
evidence="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$evidence")"
case "$evidence/" in "$root/"*) echo 'Evidence must be outside the example checkout' >&2; exit 2;; esac
mkdir -p "$evidence"
export AVM_EVIDENCE_DIR="$evidence"
: "${TMPDIR:?Set TMPDIR to external scratch storage}"
export TMPDIR TMP="$TMPDIR" TEMP="$TMPDIR"
sdk="$(cd "$sdk" && pwd -P)"
builder="${AVM_CONTRACT_BUILD:-$sdk/bin/avm-contract-build}"
stand="${AVM_LOCAL_STAND:-$sdk/bin/avm-local-stand}"
profile="${AVM_WASMTIME_PROFILE:-$sdk/profiles/wasmtime-release-profile.v1}"
cd "$root"
rustc --version
sha256sum "$builder" "$stand" "$profile" "$optimizer" > "$evidence/tool-sha256.txt"
cargo fmt --all -- --check
python3 -B scripts/oracle.py --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings 2>&1 | tee "$evidence/clippy-native.txt"
cargo test --workspace --locked --offline 2>&1 | tee "$evidence/native-tests.txt"
bash scripts/build.sh "$sdk" "$optimizer"
for contract in vesting receiver; do
  cargo clippy -p "avm-stream-$contract" --target wasm32-unknown-unknown --locked --offline -- -D warnings 2>&1 | tee "$evidence/clippy-wasm-$contract.txt"
done
python3 -B scripts/verify_artifacts.py --output "$evidence/artifacts.json"
python3 -B scripts/e2e.py --sdk "$sdk" --output "$evidence/avm-e2e.json"
echo "All vesting stream checks passed."
