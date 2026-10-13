# Revenue Hub integration validation

Validated locally on 2026-10-13 against SDK baseline
`72ee42dbf923f081bf29b4a1a1b6ea012b6af54f` and standalone source
`9ee2ede44af5f76515a5ab42dcddc8a7299c6354`.

## Completed source checks

- Root SDK formatting, native all-target Clippy under `-D warnings`, and all
  12 SDK tests (8 unit + 4 canonical I/O): PASS.
- Formatting and WASM Clippy for all four existing fixtures and the new nested
  workspace: PASS. SDK source, root workspace/lockfile, and standard fixtures
  remain byte-for-byte unchanged against the baseline.
- 74 application regressions, including 4,500 generated action steps,
  deposit fragmentation equivalence, full-u128 cases, 100 failed claims followed
  by retry, exact proposal voting, partial batch failure, semantic duplicate
  rejection, and isolated/batch equivalence: PASS.
- Native all-target and three separate guest Clippy checks: PASS.
- Three official reproducible builds, canonical ACAB IDs, two byte-identical
  pinned optimizer runs per guest, required exports, and zero host imports: PASS.
- Actual optimized-WASM stand journey and replay: PASS. Final synthetic
  principal is **10,010 = 5,005 + 3,003 + 2,002**, with zero claimable, pending,
  and dust. Receiver and audit totals agree. Audit counts are `[2, 1, 1]` despite
  the second receiver emitting duplicate report intents. The queue is empty.
  Replay matches account/registry roots, accepted-input count, and execution fees.
- Native output-byte probe for 1, 8, 64, and 256 messages: PASS. This measures
  serialized bytes, not FUEL, CPU, or throughput.

## Integrated artifact hashes

| Contract | Optimized WASM SHA-256 | ACAB SHA-256 | Descriptor SHA-256 |
| --- | --- | --- | --- |
| vault | `0dac9c37ef116c0746f1a590af9e78067082778417a2031b3783ce05286829b7` | `0f0c41a84393ee45535fde072b2fb55883aed14444ddbaf41662256285b7797c` | `5657064adfa9d91dbcdce1700071b7e397ffb68c4632a48df1eddf48205c9f56` |
| receiver | `45a62e6d8f8e1edfb95a195006adede0a789c3f1a0c6ee9b30edeedfda50f25d` | `e0502b21192aa653ec920eb92641d3b413873e6987a0082de8d600a9fb49977b` | `874538c8bad1b28321b3547aea8a5d92066b66eefa0b7f4239961e34123633a6` |
| audit | `da407bb0cb1bb68f5c1b38d01e62c18f6b643c3548d90ae4ad06b32be9704df8` | `97a2c23f225da59eb76c3aedc781d610745e85b5f8e45cfe960e914dfbc8e6ed` | `4f7e751e266c9718e05687da20f21baf19e53d99bdea696ca3cf7aa77ce9907d` |

These are actual optimized artifacts from the in-tree reference-runtime gate.
Original compiler WASM and deterministic optimization hashes are regenerated in
`evidence/optimization.json`; final export/hash inspection is `artifacts.json`.
The complete journey is `evidence/avm-e2e.json`, and byte measurements are
`evidence/batch-probe.csv`. Generated evidence is ignored by Git and uploaded by
`.github/workflows/revenue-hub.yml` on hosted runs.

## Packaging rehearsal

The current `scripts/package-release.sh` was run with the integrated SDK source
and the verified reference builder/stand binaries. Both archive and extracted
file SHA-256 checks passed. The existing smoke test and the complete Revenue Hub
gate passed from the extracted distribution, proving that relative SDK paths,
workspace metadata, scripts, optimized artifacts, and the stand journey resolve
inside the packaged layout. This reused public reference binaries; it was not a
fresh private-runtime compilation. After remapping SDK diagnostic paths with
Cargo's compiler wrapper, all three final WASM/ACAB/descriptor hash triples
matched the source-tree gate exactly across the two different locations. The
wrapper preserves the official builder's VM flags. Validation/provenance
documentation was updated after this rehearsal; contract sources were unchanged.

## Runtime limits

The verified public reference runtime is
`avm-dev-335a0f09fdbc-sdk-0cbca40fad9a`, runtime source
`335a0f09fdbc98eb7a1056c0f6b84cd4f39d889e`, packaged SDK
`0cbca40fad9a8f11863197cbcbaf6208651752b9`. Binaryen is version 133.
Archive hashes and source provenance are in `PROVENANCE.json`.

A new private `gosh-sh/acki-nacki` runtime-source build and its focused source tests
were not run locally. The original upstream release gates remain required.
Validation uses same-DApp local stand fixtures and runtime-authentic callbacks;
no mainnet, cross-DApp, independent audit, acceptance, or reward is claimed.
