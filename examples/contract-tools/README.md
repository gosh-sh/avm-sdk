# Generated dispatch and constructor example

Build this example with the source candidate builder. It generates method IDs,
typed arguments and caller checks from `avm-contract.toml`. The constructor uses
the borrowed deploy view to decode its initialization bytes. An external
`tools.add` call sends an ordinary internal `tools.sum` message to the same
account; that handler checks the sender and stores the checked sum.

The example uses only the SDK and the fixed deterministic WASM interface. It
does not use host imports or event-read output extensions. It is a tooling
example, not a general authorization or financial application template.
