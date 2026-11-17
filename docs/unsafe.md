# Unsafe Code In The Contract SDK

The contract SDK crate `avm-contract-sdk` denies unsafe code at its root
(`#![deny(unsafe_code)]`). Exactly two private sites allow it. Contracts built
with the SDK keep `#![forbid(unsafe_code)]` and never call either site
directly. This page lists each site, the invariant it relies on, why the
invariant holds, and the tests that check it.

## Input Buffer View

- File and function:
  [`crates/avm-contract-sdk/src/runtime.rs`](../crates/avm-contract-sdk/src/runtime.rs),
  `host_written`.
- What it does: the `avm_guest_alloc` export returns a heap input buffer, the
  reserved capacity of a `Vec<u8>`, that is not zero-filled. The exports and the
  input path exist only on WASM, the one path the AVM host runs. When the entrypoint
  runs, `host_written` views the requested range of that buffer as `&[u8]`,
  and the SDK decodes the input in place.
- Why safe Rust cannot do it: safe Rust only hands out initialized bytes. A
  zero-filled buffer costs one `memory.fill` over the whole input, about 3 FUEL
  per input byte before contract code runs. `vec![0; n]` keeps that fill and
  turns an allocation failure into a trap, while `avm_guest_alloc` must return
  the zero pointer on failure.
- Invariant: every byte of an input allocation is written before the
  entrypoint reads it. The AVM host guarantees it by the executor call flow:
  it calls `avm_guest_alloc(input.len)`, copies the whole input into the
  returned range, and only then calls `init`, `process_message` or
  `process_batch` with that pointer and length. The guest never writes into an
  input buffer.
- How the invariant is enforced: contract code runs only inside an entrypoint
  dispatch. A dispatch started while another runs is refused (it returns the
  zero output) before any input view exists, so contract code that calls the
  hidden `GuestArena` and `dispatch_*` items itself cannot view a buffer the
  host did not write.
- Why it is actually safe: `host_written` is reached only from the entrypoint
  dispatch, after the host copy, and only for a range inside a live input
  allocation that the arena returned. `MaybeUninit<u8>` has the size,
  alignment and layout of `u8`, and the view borrows the buffer for no longer
  than the arena lock is held. A failed reservation returns the zero pointer
  without creating an allocation. Small inputs use the initialized 4 KiB
  static region and never reach this function.
- Safety tests: real WASM contracts built with the SDK, prepared and
  instantiated by the node code (`avm-wasm-code`); each test plays the host in
  the executor call order (`avm_guest_alloc`, host write, entrypoint). In the
  acki-nacki repository:
  - `node/libs/avm-contract-build/tests/sdk_input_wasm.rs::input_buffer_reads_back_host_bytes_at_every_boundary_size_and_the_maximum`
  - `node/libs/avm-contract-build/tests/sdk_input_wasm.rs::a_nested_dispatch_is_refused_before_any_input_read`
  - `node/libs/avm-contract-build/tests/sdk_input_wasm.rs::input_view_is_in_place_and_bounded_by_the_live_allocation`
  - `node/libs/avm-contract-build/tests/sdk_input_wasm.rs::small_allocations_stay_static_and_larger_ones_take_the_heap`
  - `node/libs/avm-contract-build/tests/sdk_input_wasm.rs::dispatch_keeps_the_live_input_and_places_the_output_after_it`
  - `node/libs/avm-contract-build/tests/sdk_input_wasm.rs::batch_dispatch_writes_the_public_serializer_bytes`
  - `node/libs/avm-contract-build/tests/sdk_arena_memory.rs::arena_capacity_lifo_and_exact_free_bounds_hold_in_real_wasm`
    (zero length, allocation failure, capacity, LIFO and exact free)
- Real-path tests: every bundled contract is built with this allocator and
  deployed and called on the local stand by its application gate, for example
  `wallet-application-checks` (`examples/wallet/stand-e2e.sh`). Each contract's
  committed `avm-memory.json` is measured on these builds, so an SDK change
  that alters a contract's stack use must be measured again
  ([contract memory](contract-memory.md)).
- CI: every acki-nacki CI run executes the safety tests in the step
  `debug-tests` of `.woodpecker/avm.yaml` with
  `cargo +1.98.0 test --locked -p avm-contract-build`, on the guest crate that
  is byte-identical to this one. The application gates of
  [`scripts/manual_gates.py`](../scripts/manual_gates.py) run on the local
  stand with tools compiled in both the stand and the node graph on every
  qualification run.

## Crypto Precompile Call

- File and function:
  [`crates/avm-contract-sdk/src/crypto.rs`](../crates/avm-contract-sdk/src/crypto.rs),
  module `import`, function `invoke` (WASM only).
- What it does: declares the one admitted host import,
  `avm.precompile(id, in_ptr, in_len, out_ptr) -> status`, and calls it for
  `crypto::blake3`, `crypto::sha256` and `crypto::ed25519_verify`.
- Why safe Rust cannot do it: calling a host import declared in an
  `extern` block is unsafe in Rust.
- Invariant: the host reads only `in_len` bytes at `in_ptr` and writes only the
  fixed output width of the id at `out_ptr`, and only after it has checked both
  complete ranges and their non-overlap. The call is synchronous, never
  re-enters the guest and never grows memory. The AVM host guarantees it by
  the crypto precompile ABI ([crypto helpers](crypto.md)).
- Why it is actually safe: `invoke` is private; the public wrappers pass a
  shared borrow of the input and an exclusive borrow of an output array of
  exactly the id's width (32 bytes for ids 1 and 2, one byte for id 3), so the
  ranges are live and disjoint for the whole call. `invoke` traps on any other
  id, on a wrong output width, on an Ed25519 input shorter than 96 bytes, on a
  length that does not fit `u32`, and on a non-zero status. No raw-pointer API
  is public. Off WASM the helpers panic instead of calling anything.
- Safety tests:
  - `crates/avm-contract-sdk/tests/crypto.rs::contract_crypto_public_types_are_safe_and_fixed`
  - `crates/avm-contract-sdk/tests/crypto.rs::native_blake3_fails_explicitly`
  - `crates/avm-contract-sdk/tests/crypto.rs::native_sha256_fails_explicitly`
  - `crates/avm-contract-sdk/tests/crypto.rs::native_ed25519_fails_explicitly`
  - [`examples/signature-verifier`](../examples/signature-verifier/README.md):
    real WASM calls of all three ids, with empty, 14-byte and 4096-byte
    payloads, known BLAKE3 results, valid signatures over the SHA-256 digest
    and invalid signatures that must return false, checked by
    `scripts/check.py`.
- CI: the SDK gate `sdk-tests` runs the native tests with
  `cargo test --locked --offline -- --test-threads=1`. The gates
  `signature-verifier-application-checks` (local stand) and
  `node-signature-verifier-application-checks` (tools compiled in the node
  graph) of [`scripts/manual_gates.py`](../scripts/manual_gates.py) run the
  WASM checks on every qualification run and require equal charges on both.
