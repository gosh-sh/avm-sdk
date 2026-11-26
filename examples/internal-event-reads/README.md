# Internal event reads

This example reads the internal events of another account. One contract plays
three roles, deployed as three accounts in three DApps:

- the emitter's `params.set` publishes a value as an internal event of an
  `event_class`;
- the caller's `params.ask` sends `params.read` to the reader and declares the
  emitter's rings that the message reads;
- the reader's `params.read` checks that its input answers exactly the declared
  rings and keeps, for each, the number of events and the newest value.

See [read internal events in a contract](../../docs/events.md#read-internal-events-in-a-contract)
for the rules. It uses only the SDK and the deterministic WASM interface.

Use matching builder, stand and profile files from one distribution or source
candidate, SDK 0.6.6 or later:

```bash
bash examples/internal-event-reads/reads.sh \
  --builder /path/to/avm-contract-build \
  --stand /path/to/avm-local-stand \
  --profile /path/to/wasmtime-release-profile.v1 \
  --out /path/to/new-reads-results
```

Python 3 and `jq` are required. The corresponding environment variables are
`AVM_CONTRACT_BUILD`, `AVM_LOCAL_STAND`, `AVM_WASMTIME_PROFILE` and
`AVM_EVENT_READS_WORK`. Without `--out`, the script chooses a temporary output
directory and prints its path. It builds a source copy there, leaving the SDK
checkout or installed source bundle unchanged. Remove that output when its
results are no longer needed.

The assertions cover declared reads of two rings of another DApp's account,
a ring without events, a ring that keeps only its last five events, unsorted
reads that the SDK refuses to write (status 43), an external call that carries
no reads (status 1603), an empty queue and replay. The example is a functional
regression, not an audited application contract or a performance benchmark.
