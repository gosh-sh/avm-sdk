#!/usr/bin/env bash
set -euo pipefail
experiment="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$experiment/evidence"
cargo fmt --manifest-path "$experiment/Cargo.toml" -- --check
COMPACT_VECTORS="$experiment/evidence/guest-vectors.tsv" cargo test --offline --locked --manifest-path "$experiment/Cargo.toml" > "$experiment/evidence/native-tests.txt" 2>&1
cargo clippy --offline --locked --manifest-path "$experiment/Cargo.toml" --all-targets -- -D warnings > "$experiment/evidence/clippy.txt" 2>&1
python3 "$experiment/scripts/qualify_runtime.py" --sdk "$1" --optimizer "$2"
