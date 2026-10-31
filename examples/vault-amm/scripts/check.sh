#!/usr/bin/env sh
# Full local verification. Needs `pip install blake3` for the id checks.
set -eu
cd "$(dirname "$0")/.."
python3 scripts/gen_ids.py --selftest
python3 scripts/gen_ids.py --check
python3 scripts/gen_vectors.py --check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
