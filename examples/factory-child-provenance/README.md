# Factory/child derived-address policy

This diagnostic example shows what an internal sender's derived address proves,
and what it does not. It is not a secure child registry or a sender-code proof.
The original example is [PR #26](https://github.com/gosh-sh/avm-sdk/pull/26),
by [alandocarmo](https://github.com/alandocarmo); its application roles and
address formula are retained.

One package has three constructor-selected roles:

- A factory deploys children and accepts a report when its internal sender is
  in the same DApp and derives from the factory's package, factory account id
  as owner, reported salt and child constructor data.
- A child checks its actual internal deployer against the factory named in
  its constructor, then can report its salt to that factory.
- A relay forwards a message or sends the same child deploy envelope itself.

The address commits package identity, owner, salt and constructor hash. It does
not commit the selected code version or prove who deployed that account. A
package owner can publish another code version whose child accepts any internal
deployer. Its report passes the factory's derived-address policy. This result
is an expected **limitation**, not a successful security guarantee or a fixed VM
bug. An application relying on this policy must trust all executable versions
that its package owner can authorize. The SDK supplies no sender-code proof.

The constructor uses borrowed `input.deploy()?.init_data()` and `.salt()`.
The example still constructs its own reference-only ACDP envelope and bounded
single-chunk BLAKE3 address preimage; there are no guest dependencies beyond the
in-tree SDK. Its 65-byte constructor and 8-byte salt fit that bound. This is not
a general-purpose hashing or deploy-envelope library.

## Run

Use a writable SDK checkout/export, matching `avm-contract-build`,
`avm-local-stand`, an explicit matching `AVM_WASMTIME_PROFILE`, Rust 1.93.0,
Python 3 and `jq`:

```bash
cargo test --locked --offline --manifest-path examples/factory-child-provenance/Cargo.toml
examples/factory-child-provenance/prove.sh
```

`AVM_CONTRACT_BUILD` and `AVM_LOCAL_STAND` select the binaries.
`PROVENANCE_OUT` selects an existing/writable evidence parent;
`PROVENANCE_KEEP=1` retains the task-owned temporary directory. The default
removes only the directory created by this invocation.

The script builds strict and permissive versions, then checks actual WASM:

1. Guest child derivation agrees with the stand's independent address quote,
   and the child records the factory as its actual init caller.
2. The factory accepts that child's report.
3. An unrelated relay's report leaves the factory count unchanged.
4. A same-version relay deploy fails the child's init check, creates no child,
   and retains exactly the existing 10 SHELL init fee from prefunded target
   balance. It does not roll back the valid relay transaction.
5. A permissive version in the same package has the same quoted child address;
   the factory accepts its report. The script requires this documented
   limitation, rather than treating an unrelated failure as a security fix.
6. A child without an upgrade path rejects upgrade.
7. An external report is rejected without factory state effects.
Native tests independently reject changed package, owner, salt and DApp without
effects. The script also requires replay to agree without changing persisted
state. A real multi-package deployment is not part of this script. Claims are limited to the
configured local stand; no network, production security or performance
qualification is implied.

All balances and transfers use explicit ECC entries. The script prefunds child
targets; their deploy messages transfer no value. The fixed init fee can be paid
from that target balance. FUEL reserves remain separate. No unnamed main balance
or implicit Main-to-ECC mapping is used. The script intentionally publishes two
versions into one explicit package; it does not test package reuse or general
multi-package publication.
