# Lending Market validation on AVM SDK 0.6.5

The compatibility patch was validated against the pinned SDK 0.6.5 Linux
bundle with Rust 1.99.1. Two independent local deployment builds produced
byte-identical WASM, ACAB and descriptors for the market, test payee and wallet.
The native workspace passed 49 tests.

| Market artifact or resource | Measurement |
| --- | ---: |
| Deployment WASM | 72,460 bytes |
| Stack | 5,360 bytes |
| Serialization arena | 80,674 bytes |
| Peak linear memory including deployment | 196,608 bytes |
| Peak linear memory for the measured calls below | 131,072 bytes |

## Local stand

- The borrower repaid the loan, unlocked collateral and withdrew it. A receiver
  rejection refunded the full 100,000 ECC 7. A retry delivered it once, and a
  further request was rejected for insufficient balance.
- The borrower flow passed with all 24 account slots occupied. The 25th
  account was refused with status `5011`. The deposit was returned and the
  contract state was byte-identical.
- Valid `set_params` emitted `lend.params`. Unauthorized and invalid calls
  preserved the contract state. With space for an admin entry, reserve
  collection credited 31 ECC 9 and preserved lender net assets.
- With all 24 slots occupied, collection returned `5011` when no admin ledger
  entry existed. All 31 ECC 9 remained in reserves. With an existing admin
  entry, collection at full capacity credited 61 ECC 9 and preserved lender
  net assets.

| Market operation | Scenario | Compute FUEL |
| --- | --- | ---: |
| Repayment | 24 account slots occupied | 498,625 |
| Accepted withdrawal | 24 account slots occupied | 387,889 |
| Market callback for the accepted withdrawal | 24 account slots occupied | 365,620 |
| Parameter update | Admin scenario | 295,258 |
| Reserve collection | Space for a new admin ledger entry | 310,160 |
| Reserve collection | Full registry with an existing admin entry | 446,101 |

Compute FUEL covers the market's own WASM execution. The stand scenarios use
synthetic balances. Two new native integration tests verify payouts settled in
reverse callback order and the rejection of a second payout to a recipient
whose first payout is pending. The local stand interface provides no callback
ordering control. Runtime scheduling for concurrent payouts, migration of
existing funded accounts and Shellnet validation remain open.

Run [scripts/check-sdk065.sh](scripts/check-sdk065.sh) as described in the
[README](README.md#verify-on-avm-sdk-065) to reproduce the local results.
