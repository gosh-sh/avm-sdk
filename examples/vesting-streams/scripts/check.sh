#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${1:-$root/.runtime/avm-sdk-linux-x86_64}"
optimizer="${2:-$root/.runtime/binaryen-version_133/bin/wasm-opt}"
sdk="$(cd "$sdk" && pwd)"
cd "$root"
mkdir -p evidence
rustc --version
cp "$sdk/BUILD-MANIFEST.json" evidence/runtime-build-manifest.json
sha256sum "$sdk/bin/avm-contract-build" "$sdk/bin/avm-local-stand" \
  "$sdk/profiles/wasmtime-release-profile.v1" "$optimizer" > evidence/tool-sha256.txt
cargo fmt --all -- --check
python3 scripts/oracle.py --check

cargo clippy --workspace --all-targets --locked --offline -- -D warnings 2>&1 | tee evidence/clippy-native.txt
cargo test --workspace --locked --offline 2>&1 | tee evidence/native-tests.txt
bash scripts/build.sh "$sdk" "$optimizer"
for contract in vesting receiver; do
  cargo clippy -p "avm-stream-$contract" --target wasm32-unknown-unknown --locked --offline -- -D warnings 2>&1 | tee "evidence/clippy-wasm-$contract.txt"
done
python3 scripts/e2e.py --sdk "$sdk"
python3 scripts/verify_artifacts.py
echo "All vesting stream checks passed."
