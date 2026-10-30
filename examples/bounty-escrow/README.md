# Bounty Escrow regression example

A milestone escrow and a bound accepting/rejecting receiver fixture, using the
in-tree guest SDK. The example covers approvals of exact work hashes,
asynchronous payout callbacks, exact failed-delivery principal return, expiry
refunds, retry, and replay/accounting checks. It is a local synthetic-funds
example, with no production deployment or audit claim.

## Principal and fees

`PRINCIPAL_CURRENCY_ID = 0` is an explicit application choice for this new
ECC-only example. It does not name a live token, reserve that ID, identify the
former unnamed main asset, or convert existing accounts. Funding supplies one
ECC entry with this ID and the exact configured total, for example the local
stand call option `--extra-currencies 0:1000`. Wrong currencies and mixed
funding are rejected. Payouts and refunds carry this same ID and exact amount.

SHELL (ECC 2) funds execution separately. A fee donation uses the topup method
with `--extra-currencies 2:10000`. A successful callback may return unused FUEL
fees as SHELL; a failed callback must return exact principal plus optional
SHELL. The operating-reserve precheck is not a complete fee/rent estimator;
the runtime enforces actual fees and route requirements.

## Run checks

Use Rust 1.93.0 with rustfmt, Clippy and `wasm32-unknown-unknown`, plus Python 3.
The workspace has no external Rust dependencies. From the SDK root:

```bash
cargo +1.93.0 test --manifest-path examples/bounty-escrow/Cargo.toml --workspace --locked --offline
bash examples/bounty-escrow/scripts/check.sh \
  --sdk /absolute/path/to/verified/avm-sdk-linux-x86_64 \
  --out /absolute/path/to/bounty-check
```

For explicit tools, replace `--sdk` with `--builder`, `--stand` and `--profile`
paths. Use a fresh writable `--out` outside the source tree. Logs, hashes and
temporary stand state are local working data and are not source artifacts.
The selected distribution/tools must support the current ECC-only guest ABI;
the historical SDK 0.2.0 distribution is incompatible with these guests.

The existing entry point checks formatting, locked native Clippy and all 37
regressions, both WASM guests, the official builder's two clean builds, canonical
method/event IDs, required exports and zero host imports. Its actual local-stand
journey uses quote/fund/deploy and exercises unauthorized calls, wrong currency,
rejected payout/refund with exact return, retries, and payment after expiry.
The fixed outcome is `1000 = 700 paid + 300 refunded`, five attempts, no pending
transfer and an empty queue. Journal replay must reproduce roots, accepted input
count and cumulative SHELL fees. These are correctness checks, not throughput,
network, production pricing or release qualification.

## Contract boundaries

See [the specification](docs/SPEC.md) for fixed state layouts, authority,
accounting and error codes. Init reads the constructor through the SDK's
borrowed deployment view. The configured owner must equal the actual external
init caller; the local journey selects its developer identity for that role.
The worker is a distinct configured identity. Both receiver bindings are
immutable and owner-authorized.

There are at most eight milestones and 64 outbound attempts. A missing callback
or exhausted attempt bound can leave principal locked. There is no timeout
unlock, arbitration, partial funding, administrative recovery, arbitrary call,
code upgrade, wallet withdrawal or automatic expiry execution. Callback trust
comes from authentic AVM-native provenance and queue replay protection; the
example does not authenticate arbitrarily forged native callback frames.

## Source and license

Imported from [h2so4nackl-code/avm-bounty-escrow at 054300d](https://github.com/h2so4nackl-code/avm-bounty-escrow/tree/054300d719445a0f3c1229f63e033cc59a8c0d80)
via SDK PR #9 at `c0130e0017ea8eee690d84f4a63ad07c9d5b643d`.
The original 35 financial/authorization/replay regressions are retained; guest
frames, the explicit application currency and constructor/tool interfaces are
adapted for the current SDK. The original MIT notice is retained in [LICENSE](LICENSE).
Historical validation for the original ABI is available on that PR and is not
current qualification of this port.
