#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${1:-$root/.runtime/avm-sdk-linux-x86_64}"
optimizer="${2:-$root/.runtime/binaryen-version_133/bin/wasm-opt}"
sdk="$(cd "$sdk" && pwd)"
cd "$root"
mkdir -p evidence
rustc +1.93.0 --version
cargo +1.93.0 fmt --all -- --check
cargo +1.93.0 clippy --workspace --all-targets --locked --offline -- -D warnings 2>&1 | tee evidence/clippy-native.txt
cargo +1.93.0 test --workspace --locked --offline 2>&1 | tee evidence/native-tests.txt
cp "$sdk/BUILD-MANIFEST.json" evidence/runtime-build-manifest.json
bash scripts/build.sh "$sdk" "$optimizer"
for contract in vault receiver audit; do
  cargo +1.93.0 clippy -p "avm-revenue-$contract" --target wasm32-unknown-unknown --locked --offline -- -D warnings 2>&1 | tee "evidence/clippy-wasm-$contract.txt"
done
cargo +1.93.0 run -p avm-revenue-tests --bin batch_probe --locked --offline > evidence/batch-probe.csv
python3 scripts/e2e.py --sdk "$sdk"
python3 scripts/verify_artifacts.py
echo "All revenue hub checks passed."
