# Releases and provenance

Each release has an immutable tag
`avm-dev-<source12>-sdk-<sdk12>`. `BUILD-MANIFEST.json` records the complete
40-character source and SDK commits. Published tags and assets are not replaced.

The guest SDK, examples and documentation come from the recorded SDK commit.
Host tools come from the recorded runtime source and its committed lockfile.
The bundle includes the matching Wasmtime profile. Keep the tools, profile and
local state from one release together; see [compatibility guidance](release-notes.md#compatibility).

## Package contents and trust

The v2 bundle contains:

- Host commands, guest source, examples and documentation.
- The runtime profile and `BUILD-MANIFEST.json`.
- `provenance/build.json`, `provenance/runtime.Cargo.lock`, Cargo artifact
  records and `SHA256SUMS`.
- The SDK's MIT license, the host tools' license in
  `licenses/Acki-Nacki-Node-LICENSE.md`, and dependency license notices.

The manifest and build receipt bind the shipped binaries, source identities,
compiler, profile and runtime lockfile. Release notes describe supported
platforms, compatibility changes and known limitations.

The installer checks the downloaded checksum files and identity bindings before
extracting or activating a bundle. It rejects unsafe archive entries and does
not silently treat a v1 archive as a v2 release. Use the verifier from a trusted
checkout or a previously verified installation, never from an unverified
download. SHA-256 checksums do not provide an independent signature beyond the
GitHub/HTTPS trust boundary.

## Verify a downloaded bundle

With the verifier from your trusted SDK checkout, validate the archive and its
downloaded checksum before installation:

```bash
python3 scripts/release.py verify \
  --archive /path/to/avm-sdk-linux-x86_64.tar.gz \
  --checksum /path/to/avm-sdk-linux-x86_64.tar.gz.sha256 \
  --tag "$RELEASE_TAG"
```

Set `RELEASE_TAG` to the exact tag from the release page. The verifier checks
the archive checksum, complete payload inventory and manifest/build identity
bindings. Verification does not rebuild or upgrade deployed contracts.

Follow [Getting Started](getting-started.md) for installation and local stand
initialization. Contract publication and upgrades remain explicit operations.
