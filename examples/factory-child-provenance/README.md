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

The descriptor declares `owner_key_offset = 256`: the last 32 bytes of the
account data hold the owner key, which `init` writes from the deploy's
`owner_id` with `avm_contract_sdk::set_owner_key`. The factory and the relays
are deployed with the default `owner_id`, the user's key, which signs their
external calls. A child's `owner_id` is its factory's account id, which is no
signing key, so a child takes no external call; the script sends its report
through a relay.

The address commits package identity, owner, salt and constructor hash, not
the selected code version: the stand quotes the same child address for both
versions of the package. Only a contract in the factory's DApp can deploy an
account there: an external deploy creates the first contract of a new DApp,
and an internal deploy stays in the sender's DApp. In this example only the
factory deploys in its DApp, with the code version it runs, and no version of
the package upgrades, so a sender that passes the factory's check is one of its
own children. A relay of another version, whose child accepts any internal
deployer, can create that child only in the relay's own DApp, and the factory
refuses a sender outside its DApp. The check proves that a contract in the
factory's DApp deployed the sender, not which code version the sender runs: an
application whose DApp holds a contract that can deploy other code versions
must trust every version that contract can deploy. The SDK supplies no
sender-code proof.

The constructor uses borrowed `input.deploy()?.init_data()` and `.salt()`.
The example still constructs its own reference-only ACDP envelope and bounded
single-chunk BLAKE3 address preimage; there are no guest dependencies beyond the
in-tree SDK. Its 65-byte constructor and 8-byte salt fit that bound. This is not
a general-purpose hashing or deploy-envelope library.

## Run

Use a writable SDK checkout/export, matching `avm-contract-build`,
`avm-local-stand`, an explicit matching `AVM_WASMTIME_PROFILE`, Rust 1.99.1,
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
4. A same-version relay's child deploy into the factory's DApp is refused at
   the DApp boundary before init: no child and no init fee. In the relay's own
   DApp the same forged deploy fails the child's init check, creates no child,
   and retains exactly the existing 10 SHELL init fee from prefunded target
   balance. Neither rolls back the valid relay transaction.
5. A permissive version in the same package has the same quoted child address,
   but its relay's deploy into the factory's DApp is refused the same way and
   the factory count is unchanged. In the relay's own DApp the permissive child
   is created with the relay as its deployer.
6. A child without an upgrade path rejects upgrade.
7. An external report is rejected without factory state effects.
8. A relay forwards the largest body the network allows, 65,460 bytes, to the
   factory's `ping`; one byte more is rejected at ingress.
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
