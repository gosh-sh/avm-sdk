#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${1:-$root/.runtime/avm-sdk-linux-x86_64}"
sdk="$(cd "$sdk" && pwd)"
cd "$root"
cargo fmt --manifest-path tests/fixtures/state-boundary/Cargo.toml -- --check
cargo clippy --manifest-path tests/fixtures/state-boundary/Cargo.toml \
  --target wasm32-unknown-unknown --locked --offline -- -D warnings
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3
export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=symbols
"$sdk/bin/avm-contract-build" build --manifest-path tests/fixtures/state-boundary/Cargo.toml \
  | tee evidence/build-state-boundary.txt
python3 scripts/state_limits.py --sdk "$sdk"
vectors="$(mktemp)"
trap 'rm -f "$vectors"' EXIT
cargo run -p avm-revenue-tests --bin audit_mode_vectors --locked --offline > "$vectors"
node scripts/guest_modes.mjs contracts/audit/target/avm/contract.wasm "$vectors" evidence/audit-guest-modes.json
