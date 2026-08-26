#!/usr/bin/env bash
set -euo pipefail

repository="${AVM_SDK_REPOSITORY:-gosh-sh/avm-sdk}"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
bin_dir="${AVM_SDK_BIN_DIR:-${HOME}/.local/bin}"
requested_tag="${1:-}"

command -v gh >/dev/null || { echo "gh is required" >&2; exit 1; }
command -v tar >/dev/null || { echo "tar is required" >&2; exit 1; }

if [[ -z "$requested_tag" ]]; then
    requested_tag="$(gh release view --repo "$repository" --json tagName --jq .tagName)"
fi

archive="avm-sdk-linux-x86_64.tar.gz"
release_dir="${sdk_home}/releases/${requested_tag}"
mkdir -p "$release_dir" "$bin_dir"
gh release download "$requested_tag" --repo "$repository" --pattern "$archive" --dir "$release_dir" --clobber
tar -xzf "${release_dir}/${archive}" -C "$release_dir"

root="${release_dir}/avm-sdk-linux-x86_64"
ln -sfn "$root" "${sdk_home}/current"
ln -sfn "${sdk_home}/current/bin/avm-local-stand" "${bin_dir}/avm-local-stand"
ln -sfn "${sdk_home}/current/bin/avm-contract-build" "${bin_dir}/avm-contract-build"

echo "installed ${requested_tag} under ${root}"
echo "add ${bin_dir} to PATH"
