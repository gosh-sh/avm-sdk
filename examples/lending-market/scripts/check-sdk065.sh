#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "Usage: bash check-sdk065.sh SDK_BUNDLE NEW_OUTPUT_DIR" >&2
    exit 2
fi

example="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk="$(realpath "$1")"
out="$(realpath -m "$2")"
mkdir "$out"
tree="$out/consumer"
market="$tree/examples/lending-market"
wallet="$tree/examples/wallet"
mkdir -p "$market"

tar -C "$example" --exclude=target --exclude=__pycache__ -cf - . | tar -C "$market" -xf -
cp -a "$sdk/crates" "$tree/"
cp "$sdk/Cargo.toml" "$sdk/Cargo.lock" "$sdk/LICENSE.md" "$tree/"
cp -a "$sdk/examples/wallet" "$tree/examples/"
cp "$example/scripts/sdk065-wallet-memory.json" "$wallet/avm-memory.json"

for project in "$market/contracts/lending" "$market/contracts/payee" "$wallet"; do
    "$sdk/bin/avm-contract-build" build --manifest-path "$project/Cargo.toml" \
        > "$out/${project##*/}-build.log" 2>&1
    test ! -e "$project/target/avm/NOT-FOR-DEPLOYMENT"
done
cargo test --locked --manifest-path "$market/Cargo.toml" --workspace > "$out/native-tests.log" 2>&1

run() {
    local name="$1"
    shift
    python3 "$market/scripts/e2e-sdk065.py" --sdk "$sdk" --market "$market" \
        --wallet "$wallet" --output "$out/$name.json" "$@" > "$out/$name.log"
}
run borrower
run full-registry --full-registry
run admin --admin
run reserves-admin-missing --reserve-capacity missing
run reserves-admin-existing --reserve-capacity existing
