#!/usr/bin/env sh
# Full local verification. Needs `pip install blake3` for the ABI checks.
set -eu
cd "$(dirname "$0")/.."
python3 scripts/gen_abi.py --selftest
python3 scripts/gen_abi.py --check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -p avm-multisig-treasury -p avm-treasury-payee -- -D warnings
cargo test --workspace
