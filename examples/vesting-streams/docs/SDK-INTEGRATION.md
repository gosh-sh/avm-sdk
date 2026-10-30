# In-tree SDK integration

The nested workspace contains the core, vesting and receiver contracts and
application tests. All depend directly on this repository's guest SDK. Both
guests export the MessageIsolated entrypoints without host imports or external
Rust libraries. Constructor data comes from `MessageInput::deploy().init_data()`;
the application does not parse a second deployment-envelope format.

The example chooses `PRINCIPAL_CURRENCY_ID = 0` solely for synthetic test
principal. Funding, outbound payments and failed-payment principal returns
carry that explicit ECC id. SHELL id 2 pays for execution and may appear in
native callbacks as an unused FUEL refund. Other currencies, missing principal
or incorrect principal amounts do not settle a lane. The choice of ECC0 is
neither a token identity assignment nor a conversion of an older main field.

Canonical method/event IDs are checked against official builder ACAB output by
`scripts/sync_ids.py --check`. `scripts/check.sh` requires an external
`AVM_EVIDENCE_DIR`, runs native tests and both guest checks, builds real WASM and
executes the complete local stand journey and replay. `AVM_LOCAL_STAND` and
`AVM_WASMTIME_PROFILE` may select an explicit matching stand/profile.

The journey independently expects the CLI deploy signer, Developer, to own
each receiver and checks that expectation against receiver state. Package
publication ownership and the external caller visible to `init` are different
roles. Real native callbacks settle payments; no callback is fabricated by
the journey.
