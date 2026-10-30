# Source provenance

The application was authored by `h2so4nackl-code` in
[`avm-vesting-streams`](https://github.com/h2so4nackl-code/avm-vesting-streams/tree/e258b0e516519202bdd937445e38efddce8c4741),
commit `e258b0e516519202bdd937445e38efddce8c4741`, and contributed to the
SDK in [PR11](https://github.com/gosh-sh/avm-sdk/pull/11), head
`63699ef9d05a7ca13d47867d4ad1d711c26223cf`. Its MIT license, vesting
arithmetic, fixed state representation, bigint oracle and original regression
families are retained.

The in-tree adaptation uses the current ECC-only SDK, explicitly chooses test
ECC0 principal, reads constructor data through the SDK's borrowed deploy view
and permits native SHELL fee refunds in callbacks. Unknown currencies and
incorrect principal, source, receipt, nonce and time remain rejected. The
journey distinguishes package publication identity from the constructor caller.
The SDK source is a path dependency, with no copied SDK or external Rust guest
dependencies. Build with a tool distribution and profile matching that SDK.
