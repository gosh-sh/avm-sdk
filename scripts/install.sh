#!/usr/bin/env bash
set -euo pipefail

repository="${AVM_SDK_REPOSITORY:-gosh-sh/avm-sdk}"
sdk_home="${AVM_SDK_HOME:-${HOME}/.local/share/avm-sdk}"
export TMPDIR="${TMPDIR:-${sdk_home}/tmp}"
export TMP="$TMPDIR" TEMP="$TMPDIR"
bin_dir="${AVM_SDK_BIN_DIR:-${HOME}/.local/bin}"
requested_tag="${1:-}"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

command -v gh >/dev/null || { echo "gh is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "Python 3.11 or newer is required" >&2; exit 1; }
python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else "Python 3.11 or newer is required")'
mkdir -p "$TMPDIR"

if [[ -z "$requested_tag" ]]; then
    requested_tag="$(gh release view --repo "$repository" --json tagName --jq .tagName)"
fi

[[ "$requested_tag" =~ ^avm-dev-[0-9a-f]{12}-sdk-[0-9a-f]{12}$ ]] || {
    echo "invalid immutable release tag" >&2; exit 1;
}
mkdir -p "$sdk_home"
download="$(mktemp -d "${sdk_home}/.download.XXXXXXXX")"
trap 'rm -rf -- "$download"' EXIT
archive=avm-sdk-linux-x86_64.tar.gz
gh release download "$requested_tag" --repo "$repository" \
    --pattern "$archive" --pattern "${archive}.sha256" --dir "$download"
# Use this trusted checkout's verifier, never downloaded unverified code.
python3 "${script_dir}/release.py" install --archive "$download/$archive" \
    --checksum "$download/${archive}.sha256" --tag "$requested_tag" \
    --sdk-home "$sdk_home" --bin-dir "$bin_dir"
echo "add ${bin_dir} to PATH"
