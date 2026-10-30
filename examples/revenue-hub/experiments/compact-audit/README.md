# Separate compact audit example

This optional AccountBatch ledger uses `44 + 96 * member_count` bytes of
application state. It is a separate code/schema for **new installations only**.
It retains the regular ledger's immutable owner/source bindings, exact receipt
amount/nonce checks, sorted patches, individual rejection statuses and event
indexes. No contract method authorizes conversion or upgrade of existing regular
ledger accounts. The offline `compact_legacy` / `expand_legacy` functions are
comparison helpers, not on-chain migration authority.

With three members, state has 332 bytes; with one SHELL balance entry the current
canonical account size is `39 + 20 + 332 = 391` bytes. This is byte accounting,
not a production rent quotation or performance claim. Regular state remains
65,540 bytes. The schemas and layouts are incompatible.

Run the complete [Revenue Hub check](../../README.md#run-checks) first; it builds
the regular vault/receiver and both audit guests. The nested entry point is:

```bash
bash examples/revenue-hub/experiments/compact-audit/scripts/verify.sh \
  --sdk /absolute/path/to/verified/avm-sdk-linux-x86_64 \
  --optimizer /absolute/path/to/tools/binaryen-version_133/bin/wasm-opt \
  --out /absolute/path/to/compact-check
```

Checks retain the ten authored regressions, including 512 differential batches,
malformed state, overflow, ownership, source authentication and serialized restart.
Actual WASM/native vectors cover both requested state modes and rejected batches.
The local journey reconciles `10010 = 5005 + 3003 + 2002` paid, audit totals,
empty queues, copied-state restart and replay. Local rent probes use canonical
byte sizes; three direct regular-to-compact upgrade attempts must reject with
unchanged application data. These synthetic checks establish no mainnet pricing,
production migration, audit-delivery recovery or crash-in-mid-write guarantee.

Original source and MIT attribution come from the [Revenue Hub import](../../README.md#source-and-license).
