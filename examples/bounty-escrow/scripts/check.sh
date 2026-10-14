#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${1:-$root/.runtime/avm-sdk-linux-x86_64}"
sdk="$(cd "$sdk" && pwd)"
export PATH="$sdk/bin:$PATH"
cd "$root"
mkdir -p evidence
rustc +1.93.0 --version
python3 --version
cp "$sdk/BUILD-MANIFEST.json" evidence/runtime-build-manifest.json
sha256sum "$sdk/bin/avm-contract-build" "$sdk/bin/avm-local-stand" "$sdk/profiles/wasmtime-release-profile.v1" > evidence/runtime-sha256.txt
cargo +1.93.0 fmt -p avm-bounty-escrow -p avm-bounty-receiver -p avm-bounty-tests -- --check
cargo +1.93.0 clippy --workspace --all-targets --locked --offline -- -D warnings 2>&1 | tee evidence/clippy-native.txt
cargo +1.93.0 test --workspace --locked --offline 2>&1 | tee evidence/native-tests.txt
avm-contract-build build --manifest-path contracts/escrow/Cargo.toml 2>&1 | tee evidence/build-escrow.txt
avm-contract-build build --manifest-path contracts/receiver/Cargo.toml 2>&1 | tee evidence/build-receiver.txt
python3 scripts/sync_ids.py --check
cargo +1.93.0 clippy -p avm-bounty-escrow --target wasm32-unknown-unknown --locked --offline -- -D warnings 2>&1 | tee evidence/clippy-wasm-escrow.txt
cargo +1.93.0 clippy -p avm-bounty-receiver --no-default-features --target wasm32-unknown-unknown --locked --offline -- -D warnings 2>&1 | tee evidence/clippy-wasm-receiver.txt
python3 scripts/e2e.py --sdk "$sdk"
python3 scripts/verify_artifacts.py
echo "All local checks passed."
