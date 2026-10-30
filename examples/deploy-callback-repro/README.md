# Deploy callbacks

This example uses one contract as both parent and child. The parent forwards an
ordinary message or deploy envelope and records the VM-generated callback. The
child records the caller kind and the incoming callback-request flag in `init`.
It uses only the SDK and the deterministic WASM interface.

Use matching builder, stand and profile files from one distribution or source
candidate. The callback behavior requires SDK 0.3.0 or matching candidate tools;
older SDK 0.2.0 binaries do not provide all of these paths.

```bash
bash examples/deploy-callback-repro/repro.sh \
  --builder /path/to/avm-contract-build \
  --stand /path/to/avm-local-stand \
  --profile /path/to/wasmtime-release-profile.v1 \
  --out /path/to/new-callback-results
```

Python 3 and `jq` are required. The corresponding environment variables are
`AVM_CONTRACT_BUILD`, `AVM_LOCAL_STAND`, `AVM_WASMTIME_PROFILE` and
`AVM_DEPLOY_REPRO_WORK`. Without `--out`, the script chooses a temporary output
directory and prints its path. It builds a source copy there, leaving the SDK
checkout or installed source bundle unchanged. Remove that output when its
results are no longer needed.

The assertions cover ordinary callback delivery, deploy without a request,
exactly one requested successful-deploy callback, an empty queue and replay.
They also cover active-target, malformed and wrong-address pre-init rejections,
with and without callbacks: status 63 when requested, unchanged target/registry
and init effects, and returned transferred SHELL. These funded cases cover
incurred fees independently of the transferred value. The example is a
functional regression, not a financial authorization template or performance
benchmark; it does not prove the sender's current code identity.

The original reproducer was proposed by @alandocarmo in SDK issue #13, commit
`3e44f176c690639c6cccbcd3074f364cf38feac1`; the executable regression was
developed in SDK PR #17.
