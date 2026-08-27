#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
    echo "usage: package-release.sh <ackinacki-root> <sdk-root> <output-dir> <ackinacki-sha> <sdk-sha>" >&2
    exit 2
fi

source_root="$(cd "$1" && pwd)"
sdk_root="$(cd "$2" && pwd)"
output_dir="$3"
source_sha="$4"
sdk_sha="$5"
bundle_name="avm-sdk-linux-x86_64"
bundle_root="${output_dir}/${bundle_name}"

mkdir -p "${bundle_root}/bin" "${bundle_root}/profiles" "${bundle_root}/crates" \
    "${bundle_root}/examples" "${bundle_root}/docs" "${bundle_root}/scripts"

install -m 0755 "${source_root}/target/release/avm-local-stand" "${bundle_root}/bin/"
install -m 0755 "${source_root}/target/release/avm-contract-build" "${bundle_root}/bin/"
install -m 0644 "${sdk_root}/profiles/wasmtime-release-profile.v1" "${bundle_root}/profiles/"
cp -R "${sdk_root}/crates/avm-contract-sdk" "${bundle_root}/crates/"
cp -R "${sdk_root}/examples/." "${bundle_root}/examples/"
cp -R "${sdk_root}/docs/." "${bundle_root}/docs/"
install -m 0755 "${sdk_root}/scripts/install.sh" "${bundle_root}/scripts/"
install -m 0755 "${sdk_root}/scripts/smoke.sh" "${bundle_root}/scripts/"
install -m 0644 "${sdk_root}/README.md" "${bundle_root}/"
install -m 0644 "${sdk_root}/LICENSE.md" "${bundle_root}/"
install -m 0644 "${sdk_root}/Cargo.toml" "${bundle_root}/"
install -m 0644 "${sdk_root}/Cargo.lock" "${bundle_root}/"
install -m 0644 "${sdk_root}/rust-toolchain.toml" "${bundle_root}/"
install -m 0644 "${sdk_root}/rustfmt.toml" "${bundle_root}/"

profile_sha="$(sha256sum "${bundle_root}/profiles/wasmtime-release-profile.v1" | cut -d' ' -f1)"
cat > "${bundle_root}/BUILD-MANIFEST.json" <<EOF
{
  "schema": "gosh.avm-sdk.release.v1",
  "ackinacki_repository": "gosh-sh/acki-nacki",
  "ackinacki_commit": "${source_sha}",
  "sdk_repository": "gosh-sh/avm-sdk",
  "sdk_commit": "${sdk_sha}",
  "rust_toolchain": "1.93.0",
  "target": "x86_64-unknown-linux-gnu",
  "wasmtime_profile_sha256": "${profile_sha}"
}
EOF

(
    cd "$bundle_root"
    find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > SHA256SUMS
)

mkdir -p "$output_dir"
tar -C "$output_dir" -czf "${output_dir}/${bundle_name}.tar.gz" "$bundle_name"
(
    cd "$output_dir"
    sha256sum "${bundle_name}.tar.gz" > "${bundle_name}.tar.gz.sha256"
)
