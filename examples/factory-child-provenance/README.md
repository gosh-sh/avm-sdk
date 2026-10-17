# Factory and child provenance

A factory deploys children and later accepts messages only from them. This
example checks what a contract can prove today about the sender of an internal
message, using address derivation alone.

## Roles

One package plays three roles. The role is chosen by the 65-byte constructor
data: role `u8`, then a factory address of 64 bytes.

- **Factory:**
  - `factory.spawn(salt)` builds a deploy envelope for a child and sends it.
  - `factory.accept(salt)` counts a report only when the internal sender is in
    the factory's DApp and its account id is
    `blake3("AVM_ACCOUNT_ID" | u16 0 | package id | factory account id | u16 salt length | salt | blake3(child constructor data))`.
- **Child:**
  - `init` refuses any deployer other than the factory named in its
    constructor data.
  - `child.report` sends its salt to that factory.
  - No version of the package has an upgrade path.
- **Relay:**
  - `relay.send` forwards a message.
  - `relay.forge` builds the child envelope a factory would build, and sends
    it from the relay.

The guest SDK has no deploy-envelope builder and no address helper, so the
contract builds both itself, including a single-chunk BLAKE3. The stand's own
address quote checks the result in case 1.

## Run

```bash
examples/factory-child-provenance/prove.sh
```

The script needs `avm-contract-build`, `avm-local-stand` and `jq`. It builds
two versions of the package. The second is built with
`PROVENANCE_PERMISSIVE_CHILD` set, and its child `init` accepts any deployer.

Output on release `avm-dev-06e96ef71006-sdk-0748f4f0767a` (SDK 0.2.0, guest
SDK 0.1.1), Rust 1.93.0, local stand:

```text
OK         1 spawn: the factory derives the stand's child address; child init saw the factory
OK         2 accept: the factory accepts its own child's report
OK         3 spoof: a report from an address that does not derive is rejected
OK         4 forged deploy, same version: the child refuses a deployer other than its factory
NOTE       5 the stand quotes the same child address for both code selections
REPRODUCED 5 forged deploy, version 2 of the same package: the factory accepts a child it never deployed
OK         6 upgrade: a child without an upgrade path keeps its version
replay ok
```

The script exits non-zero only when an `OK` case fails. Case 5 prints `OK`
instead of `REPRODUCED` in two situations:

- the stand no longer creates the forged child, for example if the address
  depended on the selected code or the owner had to be the deployer;
- the child is created but the factory rejects its report, which needs a
  factory that can check the sender's code. This example cannot do that on
  this release.

## What the cases show

- **The address does not fix the code version.** The address depends on the
  package, owner, salt and constructor data, but not on the selected code. In
  case 5, both versions quote the same child address.
- **The owner field does not name the deployer.** Any account can deploy a
  child that names the factory as owner (cases 4 and 5). On this release,
  `deploy --owner-id` also accepts any 32 bytes.
- **Only the child's own `init` ties it to the factory.** That check holds
  only in versions that perform it (case 4). Whoever can publish code into the
  package can deploy a child that the factory accepts (case 5), including
  after the factory is live.
- **The gap is at deploy time.** A child without an upgrade path keeps its
  version afterwards (case 6).
- **A failed `init` stops the deployer's whole call.** When a contract deploys
  a child whose `init` fails, the stand rejects the external call that caused
  it (case 4). This is the unsettled failed-init path of
  [#13](https://github.com/gosh-sh/avm-sdk/issues/13).

A guest API that proves the code of an internal sender would close case 5:
`factory.accept` could then also require that the sender's selected code at
emission is one the factory trusts. The SDK has no such API, and
[#13](https://github.com/gosh-sh/avm-sdk/issues/13) tracks it. An earlier
draft of [#17](https://github.com/gosh-sh/avm-sdk/pull/17) sketched one in
`docs/protocol-proposals.md`, since removed from the SDK. Until then, a
factory that needs this property must trust every code that reaches its
package.

## What a package covers on the local stand

Measured on this release, with the builder's own digests:

- The package id is `blake3("AVM_CODE_PACKAGE" || blake3(first WASM))`, the
  formula given in [#22](https://github.com/gosh-sh/avm-sdk/issues/22). Two
  fresh stands that deploy the two versions in opposite order take the
  package id of whichever WASM was deployed first.
- A stand holds exactly **one** package. The first deploy creates it. Every
  later code becomes a selection of that same package, including the SDK's own
  `contract-b`, an unrelated contract with its own ABI, whose formula id is
  never registered. `publish-version --package-id` for any other id is refused
  with `requested package_id is not present in the local Code Library`.
- So on the stand, a package is the whole code library, not the versions of
  one contract, and a selection may carry a different ABI and a different
  state schema from the one that named the package. A child address therefore
  pins the package, owner, salt and constructor data, and none of the code,
  the interface or the state schema.

Whether a network behaves this way is not something this script can show: the
stand never creates a second package, and it publishes under a single
identity, so it cannot say who may add code to someone else's package.

## Deploy envelope fields used

The ACDP header has 288 bytes. All integers are little-endian.

| Bytes | Field |
|---|---|
| 0..4 | `ACDP` |
| 4..6 | flags (0 for a deploy by package and selection) |
| 8..40 | selected code id |
| 40..72 | package id |
| 72..104 | WASM hash |
| 104..136 | descriptor hash |
| 136..168 | owner id |
| 168..200 | BLAKE3 of the constructor data |
| 200..232 | owner key (zero) |
| 264..266 | UI name length (`u16`) |
| 266..268 | salt length (`u16`) |
| 268..272 | constructor data length (`u32`) |

The tail holds the UI name, the salt and the constructor data. The factory
copies the package id and both hashes from the envelope that created it, and
uses the code selection it runs. To deploy, it sends the envelope with the
method id of its own deploy message.

## Limits

- Local stand only, with synthetic values. The script prefunds each child
  address, and the deploys carry no value.
- Salts have 8 bytes, and the state has 256 bytes.
- The contract's BLAKE3 takes one 1024-byte chunk. That covers the account
  preimage and the constructor data, and nothing larger: it cannot hash a
  module, so the package id must be derived from the digest the builder
  prints, not from the WASM bytes.
- The factory counts reports and keeps the last sender. It is a test fixture,
  not a registry.
