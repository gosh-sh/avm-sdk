#!/usr/bin/env bash
# Run application gates from a source tree or an isolated writable verified bundle.
set -euo pipefail
if [[ $# -lt 1 || $# -gt 3 ]]; then
    echo "usage: check-application-examples.sh <verified-sdk-directory> [wasm-opt] [example-name]" >&2
    exit 2
fi
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="$(cd "$1" && pwd)"
# Public binary checks may use a newer documentation head, but must use the
# same guest source and engine profile. This is not private-source qualification.
diff -ru "$root/crates/avm-contract-sdk" "$sdk/crates/avm-contract-sdk"
cmp "$root/profiles/wasmtime-release-profile.v1" "$sdk/profiles/wasmtime-release-profile.v1"
selected="${3:-}"
if [[ -n "$selected" && ! "$selected" =~ ^[a-z0-9-]+$ ]]; then
    echo "invalid example name" >&2; exit 2
fi
matched=0
for example in "$root"/examples/*; do
    [[ -z "$selected" || "$(basename "$example")" == "$selected" ]] || continue
    [[ -f "$example/scripts/check.sh" ]] || continue
    matched=$((matched + 1))
    if [[ -f "$example/scripts/install_binaryen.sh" ]]; then
        optimizer="${2:-$example/.runtime/binaryen-version_133/bin/wasm-opt}"
        [[ -x "$optimizer" ]] || { echo "Supply the verified Binaryen 133 wasm-opt for $example" >&2; exit 2; }
        bash "$example/scripts/check.sh" "$sdk" "$optimizer"
    else
        bash "$example/scripts/check.sh" "$sdk"
    fi
done
[[ "$matched" -gt 0 ]] || { echo "No application gates selected" >&2; exit 2; }
