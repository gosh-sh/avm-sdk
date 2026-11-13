#![forbid(unsafe_code)]

//! Pure accumulator state machine for the staking-rewards example, as
//! specified in `docs/SPEC.md`. No SDK dependency: this crate knows
//! nothing about `OutboundIntent`/`CallbackIntent`/`Caller` or any other
//! platform type -- it only knows the byte-level state and the six
//! transitions (`STAKE`, `UNSTAKE`, `CLAIM`, `FUND_REWARDS`, payout
//! success, payout failure). `contracts/staking-rewards/` is the only
//! place that imports an SDK type.

/// `MAX_AMOUNT = 2^60` (see docs/SPEC.md's "Constants").
pub const MAX_AMOUNT: u128 = 1 << 60;
/// `MAX_ACC = 2^64` (see docs/SPEC.md's "Constants"). Deliberately one past
/// `u64::MAX`, which is exactly why `acc` is stored as `u128`, not `u64`.
pub const MAX_ACC: u128 = 1 << 64;
/// `PRECISION = 2^44` (see docs/SPEC.md's "Constants").
pub const PRECISION: u128 = 1 << 44;
/// Default `min_stake` used by callers that don't pick their own (see
/// docs/SPEC.md's "Constants").
/// A per-deployment init parameter, not a fixed constant -- stored in the
/// header, validated at `State::new`.
pub const DEFAULT_MIN_STAKE: u128 = 1_000_000;
/// Test-fixture default for `fuel_reserve`, not a platform guarantee for
/// every possible deployment. `1` is the measured floor on the real local
/// stand: a `UNSTAKE`/`CLAIM` payout's own `OutboundIntent.fuel_fee_shells
/// = 0` is rejected outright with `"local call charge failed:
/// RouteRequiresFee"` (tested against the in-repo `test-receiver` fixture
/// bound and accepting), while `fuel_fee_shells = 1` already succeeds
/// against that same fixture at every tested row count. A real deployer
/// should size their own `fuel_reserve` to their own expected payout
/// destinations' complexity, not assume `1` suffices universally -- this
/// is only the platform's own enforced minimum, confirmed by direct
/// measurement, not a sizing recommendation for arbitrary receivers.
pub const DEFAULT_FUEL_RESERVE: u128 = 1;
/// Row-table cap, chosen at or below half the lowest real measured
/// per-method ceiling. Measured on the real local stand with
/// `MAX_ROWS=5000` in this crate (so the real cap could not interfere)
/// by filling state until each method genuinely
/// fails, with `loop_iters=0` (no added dummy work -- the method's own
/// real cost alone): `STAKE`(new row)=522, `STAKE`(top-up)=522,
/// `UNSTAKE`=523, `CLAIM`=523, `FUND_REWARDS`=523, both callback fast
/// paths=287 (the binding constraint). All seven failures are genuine
/// `VM_WASM_OUT_OF_FUEL` (`fuel_used=10000000=fuel_budget=
/// max_message_execution_fuel`) -- the callback's own failure is silent
/// at the outer `call` boundary (status "ok" even though the callback's
/// requested update never applied; see `docs/SPEC.md`'s callback-budget
/// section) but is the same fuel exhaustion once verified by checking the
/// callback's own effect, not just the outer status. Half of 287 is
/// 143.5, so `MAX_ROWS <= 143`.
pub const MAX_ROWS: usize = 140;

/// SHELL is the platform's fee/rent currency (`fuel-and-fees.md:16`: stored
/// as extra currency `currency_id = 2`). `stake_currency_id`/
/// `reward_currency_id` must differ from it: a pool denominated in SHELL
/// would mix stake/reward accounting with the currency this contract's own
/// outbound payouts spend as `fuel_fee_shells`, and with whatever the
/// account's own rent debits (`runtime-limits.md:70-98`).
pub const SHELL_CURRENCY_ID: u32 = 2;

pub const HEADER_BYTES: usize = 16 + 16 + 4 + 4 + 16 + 16 + 4 + 8 + 16; // 100
pub const ROW_BYTES: usize = 32 + 16 + 16 + 8 + 1 + 8 + 16 + 16; // 113 (added pending_id:u64)

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Overflow,
    Underflow,
    InvalidArgs,
    RowNotFound,
    RowAlreadyExists,
    PendingAlreadyInFlight,
    NoPendingPayout,
    BelowMinStake,
    ExceedsMaxAmount,
    FundRejectedNoStakers,
    FundExceedsMaxAcc,
    /// `FUND_REWARDS` must reject a funding amount too small to move the
    /// accumulator at all against the current `total_staked` -- it would
    /// contribute nothing and succeed silently.
    FundDeltaZero,
    NothingToClaim,
    TooManyRows,
    InvalidMinStake,
    /// `fuel_reserve` is validated at `State::new`/`decode` against
    /// `1..=MAX_AMOUNT` -- the lower bound is the measured platform floor
    /// (`fuel_fee_shells = 0` on an outbound is rejected outright with
    /// `RouteRequiresFee`, see `DEFAULT_FUEL_RESERVE`'s own doc comment),
    /// not an arbitrary choice.
    InvalidFuelReserve,
    /// `stake_currency_id`/`reward_currency_id` equal `SHELL_CURRENCY_ID`.
    CurrencyCannotBeShell,
    DecodeError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingOut {
    /// A monotonic correlation id, assigned from `State::next_pending_id`
    /// when the payout is created and echoed back as the callback's own
    /// `method_id`. Payout destinations must be deployed contracts, whose
    /// `account_id` is hash-derived and can never equal a caller's raw
    /// pubkey -- so the callback's `Caller::Internal(source)` cannot be
    /// used to locate the owning row; `pending_id` is the correlation key
    /// instead. See `docs/SPEC.md`.
    pub pending_id: u64,
    pub amount_s: u128,
    pub amount_r: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub account: [u8; 32],
    pub staked: u128,
    pub reward_debt: u128,
    pub seq: u64,
    pub pending: Option<PendingOut>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub total_staked: u128,
    pub acc_reward_per_share: u128,
    pub stake_currency_id: u32,
    pub reward_currency_id: u32,
    pub reward_pool_held: u128,
    /// Per-deployment init parameter, not a fixed constant.
    pub min_stake: u128,
    /// Per-deployment init parameter: the exact SHELL amount `UNSTAKE`/
    /// `CLAIM` must attach, used verbatim as the combined payout's own
    /// `OutboundIntent.fuel_fee_shells` -- a contract-controlled fee,
    /// never the caller's own declared value, mirroring `bounty-escrow`'s
    /// own `config.fuel_reserve`.
    pub fuel_reserve: u128,
    /// Monotonic counter for `PendingOut::pending_id`. Starts at 1 (0 is
    /// reserved: the SDK's own `CallbackIntent::handler_method_id == 0`
    /// is rejected).
    pub next_pending_id: u64,
    /// Sorted by `account`, matching the on-chain byte layout.
    pub rows: Vec<Row>,
}

fn ceil_div(n: u128, d: u128) -> Result<u128, Error> {
    let q = n.checked_div(d).ok_or(Error::Overflow)?;
    let r = n.checked_rem(d).ok_or(Error::Overflow)?;
    if r == 0 {
        Ok(q)
    } else {
        q.checked_add(1).ok_or(Error::Overflow)
    }
}

impl State {
    pub fn new(
        stake_currency_id: u32,
        reward_currency_id: u32,
        min_stake: u128,
        fuel_reserve: u128,
    ) -> Result<Self, Error> {
        if !(1..=MAX_AMOUNT).contains(&min_stake) {
            return Err(Error::InvalidMinStake);
        }
        if !(1..=MAX_AMOUNT).contains(&fuel_reserve) {
            return Err(Error::InvalidFuelReserve);
        }
        if stake_currency_id == SHELL_CURRENCY_ID || reward_currency_id == SHELL_CURRENCY_ID {
            return Err(Error::CurrencyCannotBeShell);
        }
        Ok(Self {
            total_staked: 0,
            acc_reward_per_share: 0,
            stake_currency_id,
            reward_currency_id,
            reward_pool_held: 0,
            min_stake,
            fuel_reserve,
            next_pending_id: 1,
            rows: Vec::new(),
        })
    }

    /// Assigns and advances the monotonic `pending_id` counter. Pure
    /// computation, no mutation: callers commit the returned
    /// `next_pending_id` themselves, alongside their own other mutations,
    /// so a later fallible step in the same transition cannot leave this
    /// field advanced while the rest of the transition did not happen.
    fn next_pending_id_pair(&self) -> Result<(u64, u64), Error> {
        let id = self.next_pending_id;
        let advanced = self.next_pending_id.checked_add(1).ok_or(Error::Overflow)?;
        Ok((id, advanced))
    }

    fn row_index(&self, account: &[u8; 32]) -> Option<usize> {
        self.rows.binary_search_by(|r| r.account.as_slice().cmp(account.as_slice())).ok()
    }

    fn insert_row_sorted(&mut self, row: Row) {
        let pos = self
            .rows
            .binary_search_by(|r| r.account.as_slice().cmp(row.account.as_slice()))
            .unwrap_or_else(|p| p);
        self.rows.insert(pos, row);
    }

    fn global_pending_s(&self) -> Result<u128, Error> {
        self.rows.iter().try_fold(0u128, |acc, r| {
            let p = r.pending.map(|p| p.amount_s).unwrap_or(0);
            acc.checked_add(p).ok_or(Error::Overflow)
        })
    }

    /// `STAKE(account, amount)`: synchronous, no payout. See
    /// `docs/SPEC.md`'s "Per-method equations".
    pub fn stake(&mut self, account: [u8; 32], amount: u128) -> Result<(), Error> {
        if amount == 0 || amount > MAX_AMOUNT {
            return Err(Error::InvalidArgs);
        }
        let idx = self.row_index(&account);
        let (staked_old, reward_debt_old, own_pending_s) = match idx {
            Some(i) => {
                let r = &self.rows[i];
                (r.staked, r.reward_debt, r.pending.map(|p| p.amount_s).unwrap_or(0))
            }
            None => (0u128, 0u128, 0u128),
        };
        let staked_new = staked_old.checked_add(amount).ok_or(Error::Overflow)?;

        // Admit against total committed EXPOSURE (settled + every row's own
        // in-flight principal), not bare staked/total_staked -- a payout's
        // failure restore must never be blocked, and other rows staking into
        // a pending-unstake's temporary gap must not be able to make that
        // restore exceed MAX_AMOUNT. See `docs/SPEC.md`'s "Per-method
        // equations" and invariant 15.
        let row_exposure = staked_old.checked_add(own_pending_s).ok_or(Error::Overflow)?;
        if row_exposure.checked_add(amount).ok_or(Error::Overflow)? > MAX_AMOUNT {
            return Err(Error::ExceedsMaxAmount);
        }
        let global_pending_s = self.global_pending_s()?;
        let global_exposure =
            self.total_staked.checked_add(global_pending_s).ok_or(Error::Overflow)?;
        if global_exposure.checked_add(amount).ok_or(Error::Overflow)? > MAX_AMOUNT {
            return Err(Error::ExceedsMaxAmount);
        }
        // min_stake applies to STAKE too, not only the post-UNSTAKE remainder.
        if staked_new < self.min_stake {
            return Err(Error::BelowMinStake);
        }
        // Row cap, checked BEFORE any mutation below (a rejection here must
        // never leave total_staked or anything else partially updated).
        if idx.is_none() && self.rows.len() >= MAX_ROWS {
            return Err(Error::TooManyRows);
        }

        let accrued_now = staked_old
            .checked_mul(self.acc_reward_per_share)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let settle = accrued_now.saturating_sub(reward_debt_old);

        let fund_mul = staked_new.checked_mul(self.acc_reward_per_share).ok_or(Error::Overflow)?;
        let ceiled = ceil_div(fund_mul, PRECISION)?;
        let reward_debt_new = ceiled.checked_sub(settle).ok_or(Error::Underflow)?;
        let new_total_staked = self.total_staked.checked_add(amount).ok_or(Error::Overflow)?;
        // Every fallible step above is computed into a local before this
        // point; everything from here on is an infallible commit, so an
        // Err anywhere above leaves `self` byte-for-byte unchanged (see
        // `core_err_leaves_state_unchanged`).
        let new_seq = match idx {
            Some(i) => self.rows[i].seq.checked_add(1).ok_or(Error::Overflow)?,
            None => 1,
        };

        self.total_staked = new_total_staked;
        match idx {
            Some(i) => {
                self.rows[i].staked = staked_new;
                self.rows[i].reward_debt = reward_debt_new;
                self.rows[i].seq = new_seq;
            }
            None => {
                self.insert_row_sorted(Row {
                    account,
                    staked: staked_new,
                    reward_debt: reward_debt_new,
                    seq: new_seq,
                    pending: None,
                });
            }
        }
        Ok(())
    }

    /// `UNSTAKE(account, amount)`: request time, triggers a combined payout.
    /// See `docs/SPEC.md`'s "Per-method equations".
    pub fn unstake(&mut self, account: [u8; 32], amount: u128) -> Result<PendingOut, Error> {
        let idx = self.row_index(&account).ok_or(Error::RowNotFound)?;
        if self.rows[idx].pending.is_some() {
            return Err(Error::PendingAlreadyInFlight);
        }
        let staked_old = self.rows[idx].staked;
        let reward_debt_old = self.rows[idx].reward_debt;
        if amount == 0 || amount > staked_old {
            return Err(Error::InvalidArgs);
        }
        let staked_new = staked_old.checked_sub(amount).ok_or(Error::Underflow)?;
        // Invariant 11: a partial UNSTAKE must leave 0 or >= min_stake.
        if staked_new != 0 && staked_new < self.min_stake {
            return Err(Error::BelowMinStake);
        }

        let accrued_now = staked_old
            .checked_mul(self.acc_reward_per_share)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let amount_r = accrued_now.saturating_sub(reward_debt_old);
        // Principal independence: clamp, never block, on the reward side.
        let amount_r_final = amount_r.min(self.reward_pool_held);

        let reward_debt_new = staked_new
            .checked_mul(self.acc_reward_per_share)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let new_total_staked = self.total_staked.checked_sub(amount).ok_or(Error::Underflow)?;
        let new_reward_pool_held =
            self.reward_pool_held.checked_sub(amount_r_final).ok_or(Error::Underflow)?;
        let (pending_id, new_next_pending_id) = self.next_pending_id_pair()?;
        let new_seq = self.rows[idx].seq.checked_add(1).ok_or(Error::Overflow)?;
        let pending = PendingOut { pending_id, amount_s: amount, amount_r: amount_r_final };
        // Every fallible step is above this line; everything below is an
        // infallible commit.
        self.total_staked = new_total_staked;
        self.reward_pool_held = new_reward_pool_held;
        self.next_pending_id = new_next_pending_id;
        self.rows[idx].staked = staked_new;
        self.rows[idx].reward_debt = reward_debt_new;
        self.rows[idx].pending = Some(pending);
        self.rows[idx].seq = new_seq;
        Ok(pending)
    }

    /// `CLAIM(account)`: request time, reward-only payout. See
    /// `docs/SPEC.md`'s "Per-method equations".
    pub fn claim(&mut self, account: [u8; 32]) -> Result<PendingOut, Error> {
        let idx = self.row_index(&account).ok_or(Error::RowNotFound)?;
        if self.rows[idx].pending.is_some() {
            return Err(Error::PendingAlreadyInFlight);
        }
        let staked = self.rows[idx].staked;
        let reward_debt_old = self.rows[idx].reward_debt;
        let accrued_now = staked
            .checked_mul(self.acc_reward_per_share)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let amount_r = accrued_now.saturating_sub(reward_debt_old);
        if amount_r == 0 {
            return Err(Error::NothingToClaim);
        }
        let amount_r_final = amount_r.min(self.reward_pool_held);
        // Invariant 16: reject if the post-clamp amount is zero -- UNSTAKE
        // is exempt (principal independence), CLAIM is not.
        if amount_r_final == 0 {
            return Err(Error::NothingToClaim);
        }
        let reward_debt_new = accrued_now; // exact, zero slack (same acc, same staked)
        let new_reward_pool_held =
            self.reward_pool_held.checked_sub(amount_r_final).ok_or(Error::Underflow)?;
        let (pending_id, new_next_pending_id) = self.next_pending_id_pair()?;
        let new_seq = self.rows[idx].seq.checked_add(1).ok_or(Error::Overflow)?;
        let pending = PendingOut { pending_id, amount_s: 0, amount_r: amount_r_final };
        // Every fallible step is above this line; everything below is an
        // infallible commit.
        self.reward_pool_held = new_reward_pool_held;
        self.next_pending_id = new_next_pending_id;
        self.rows[idx].reward_debt = reward_debt_new;
        self.rows[idx].pending = Some(pending);
        self.rows[idx].seq = new_seq;
        Ok(pending)
    }

    /// `FUND_REWARDS(amount)`: reject-on-result against `MAX_ACC`. See
    /// `docs/SPEC.md`'s "Per-method equations".
    pub fn fund_rewards(&mut self, amount: u128) -> Result<(), Error> {
        if self.total_staked < self.min_stake {
            return Err(Error::FundRejectedNoStakers);
        }
        if amount == 0 || amount > MAX_AMOUNT {
            return Err(Error::InvalidArgs);
        }
        let mul = amount.checked_mul(PRECISION).ok_or(Error::Overflow)?;
        let delta = mul.checked_div(self.total_staked).ok_or(Error::Overflow)?;
        // Reject delta == 0 outright -- a funding amount too small to
        // move the accumulator at all against the current total_staked
        // contributes nothing and must not silently succeed.
        if delta == 0 {
            return Err(Error::FundDeltaZero);
        }
        let candidate = self.acc_reward_per_share.checked_add(delta).ok_or(Error::Overflow)?;
        if candidate > MAX_ACC {
            return Err(Error::FundExceedsMaxAcc);
        }
        let new_reward_pool_held =
            self.reward_pool_held.checked_add(amount).ok_or(Error::Overflow)?;
        // Every fallible step is above this line.
        self.acc_reward_per_share = candidate;
        self.reward_pool_held = new_reward_pool_held;
        Ok(())
    }

    /// Finds the row currently holding `pending_id`, if any. Used by the
    /// contract's full-decode callback path (and by tests) to recover the
    /// `account` key that `payout_success`/`payout_failure` still take --
    /// a linear scan over already-decoded rows, no extra cost beyond the
    /// decode that path already paid.
    pub fn account_for_pending_id(&self, pending_id: u64) -> Option<[u8; 32]> {
        self.rows
            .iter()
            .find(|r| r.pending.is_some_and(|p| p.pending_id == pending_id))
            .map(|r| r.account)
    }

    /// Payout success: see `docs/SPEC.md`'s "Per-method equations". Deletes
    /// the row if it is now empty (see "State layout"'s row-deletion rule).
    pub fn payout_success(&mut self, account: [u8; 32]) -> Result<PendingOut, Error> {
        let idx = self.row_index(&account).ok_or(Error::RowNotFound)?;
        let pending = self.rows[idx].pending.take().ok_or(Error::NoPendingPayout)?;
        if self.rows[idx].staked == 0 {
            self.rows.remove(idx);
        }
        Ok(pending)
    }

    /// Payout failure: the "delta-rebase" restore (see `docs/SPEC.md`'s
    /// "Per-method equations"). Needs no
    /// stored "prior_reward_debt" -- only the row's current
    /// (unchanged-since-request) staked/reward_debt plus amount_s/amount_r.
    pub fn payout_failure(&mut self, account: [u8; 32]) -> Result<PendingOut, Error> {
        let idx = self.row_index(&account).ok_or(Error::RowNotFound)?;
        // A non-mutating read (`PendingOut` is `Copy`), not `.take()` --
        // several fallible steps below still need to succeed before
        // `pending` is actually cleared; clearing it up front would leave
        // the row's `pending` gone with none of the restore applied if
        // one of those later steps errored.
        let pending = self.rows[idx].pending.ok_or(Error::NoPendingPayout)?;
        let staked_remaining = self.rows[idx].staked;
        let staked_restored =
            staked_remaining.checked_add(pending.amount_s).ok_or(Error::Overflow)?;

        let term_restored = staked_restored
            .checked_mul(self.acc_reward_per_share)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let term_remaining = staked_remaining
            .checked_mul(self.acc_reward_per_share)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let window_term = term_restored.checked_sub(term_remaining).ok_or(Error::Underflow)?;

        let reward_debt_current = self.rows[idx].reward_debt;
        // Clamping reward_debt up to 0 here favors the staker, not the
        // pool -- a lower reward_debt means more future claimable. If
        // this triggers, the row is over-credited by exactly the clamped
        // (absolute) amount relative to the exact value. Measured
        // `restore_clamp_count == 0` both across a 100,000-sequence/
        // 2,500,729-step real-constant fuzz run (`python3
        // tools/staking_rewards_model.py`) and across 4,500,000
        // adversarial tiny-constant operations against one continuous
        // state (`run_adversarial_drift(4_500_000)` in the same file) --
        // not observed to trigger under either tested parameter range,
        // not proven unreachable in general.
        let reward_debt_restored = window_term
            .checked_add(reward_debt_current)
            .ok_or(Error::Overflow)?
            .saturating_sub(pending.amount_r);

        let new_total_staked =
            self.total_staked.checked_add(pending.amount_s).ok_or(Error::Overflow)?;
        let new_reward_pool_held =
            self.reward_pool_held.checked_add(pending.amount_r).ok_or(Error::Overflow)?;
        // Every fallible step is above this line; everything below
        // (including finally clearing `pending`) is an infallible commit.
        self.total_staked = new_total_staked;
        self.reward_pool_held = new_reward_pool_held;
        self.rows[idx].staked = staked_restored;
        self.rows[idx].reward_debt = reward_debt_restored;
        self.rows[idx].pending = None;
        Ok(pending)
    }

    // ---- raw-byte fast-path callback helpers ----
    //
    // A callback handler must touch only the target row, not decode every
    // row into owned `Row` structs (see `docs/SPEC.md`'s "Callback model"
    // and "Capacity and fuel"). These
    // operate directly on an encoded buffer and apply exactly the same
    // formulas as `payout_success`/`payout_failure` above -- differentially
    // tested against them in the contract's own test suite, not assumed
    // correct by construction.
    //
    // The target row is found by `pending_id` (see `docs/SPEC.md`'s
    // "Callback model"), which is not
    // sorted: this is a bounded linear scan over just the per-row
    // pending-flag byte and 8-byte id (10 bytes/row, no u128 parsing, no
    // struct allocation for non-matching rows), not a binary search. A
    // lookup keyed by `account` is not usable here, because the row's own
    // account can differ from the callback's own `Caller::Internal`
    // identity: a payout destination must be a deployed contract
    // (confirmed via `avm-local-stand`'s "account is absent" rejection of
    // a funded-but-undeployed address; `docs/getting-started.md`'s own
    // "repeating the same derivation after funding creates the account"
    // confirms deployment, not funding, is what creates an account), and a
    // deployed contract's `account_id` is hash-derived (`runtime-limits.md`'s
    // "Deterministic addresses" formula), never equal to a caller's raw
    // pubkey. Still far cheaper than decoding every row into an owned
    // `Row`, which is the property this fast path exists for.

    fn header_row_count(bytes: &[u8]) -> Result<usize, Error> {
        if bytes.len() < HEADER_BYTES {
            return Err(Error::DecodeError);
        }
        let row_count = u32::from_le_bytes(bytes[72..76].try_into().unwrap()) as usize;
        if bytes.len() != HEADER_BYTES + row_count * ROW_BYTES {
            return Err(Error::DecodeError);
        }
        Ok(row_count)
    }

    /// Linear scan for the row whose `pending` is occupied by `pending_id`,
    /// reading only the pending-flag byte and (when set) the 8-byte id of
    /// each row probed.
    fn find_row_base_by_pending_id(
        bytes: &[u8],
        row_count: usize,
        pending_id: u64,
    ) -> Result<usize, Error> {
        for i in 0..row_count {
            let base = HEADER_BYTES + i * ROW_BYTES;
            if bytes[base + 72] == 0 {
                continue;
            }
            let id = u64::from_le_bytes(bytes[base + 73..base + 81].try_into().unwrap());
            if id == pending_id {
                return Ok(base);
            }
        }
        Err(Error::RowNotFound)
    }

    /// Raw-byte fast path for a payout-success callback. Returns the new
    /// encoded state. Exactly mirrors `payout_success`: clears `pending`,
    /// and removes the row entirely if `staked == 0` (shifting only the
    /// bytes after it, not re-encoding the whole table).
    pub fn payout_success_fast(bytes: &[u8], pending_id: u64) -> Result<Vec<u8>, Error> {
        let row_count = Self::header_row_count(bytes)?;
        let base = Self::find_row_base_by_pending_id(bytes, row_count, pending_id)?;
        let staked = u128::from_le_bytes(bytes[base + 32..base + 48].try_into().unwrap());
        let mut out = bytes.to_vec();
        if staked == 0 {
            out.drain(base..base + ROW_BYTES);
            let new_count = (row_count - 1) as u32;
            out[72..76].copy_from_slice(&new_count.to_le_bytes());
        } else {
            out[base + 72] = 0;
            for b in &mut out[base + 73..base + ROW_BYTES] {
                *b = 0;
            }
        }
        Ok(out)
    }

    /// Raw-byte fast path for a payout-failure callback. Exactly mirrors
    /// `payout_failure`'s delta-rebase restore, reading/writing only the
    /// target row's own bytes plus the header.
    pub fn payout_failure_fast(bytes: &[u8], pending_id: u64) -> Result<Vec<u8>, Error> {
        let row_count = Self::header_row_count(bytes)?;
        let base = Self::find_row_base_by_pending_id(bytes, row_count, pending_id)?;
        let acc = u128::from_le_bytes(bytes[16..32].try_into().unwrap());
        let staked_remaining = u128::from_le_bytes(bytes[base + 32..base + 48].try_into().unwrap());
        let reward_debt_current =
            u128::from_le_bytes(bytes[base + 48..base + 64].try_into().unwrap());
        let amount_s = u128::from_le_bytes(bytes[base + 81..base + 97].try_into().unwrap());
        let amount_r = u128::from_le_bytes(bytes[base + 97..base + 113].try_into().unwrap());

        let staked_restored = staked_remaining.checked_add(amount_s).ok_or(Error::Overflow)?;
        let term_restored = staked_restored
            .checked_mul(acc)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let term_remaining = staked_remaining
            .checked_mul(acc)
            .ok_or(Error::Overflow)?
            .checked_div(PRECISION)
            .ok_or(Error::Overflow)?;
        let window_term = term_restored.checked_sub(term_remaining).ok_or(Error::Underflow)?;
        let reward_debt_restored = window_term
            .checked_add(reward_debt_current)
            .ok_or(Error::Overflow)?
            .saturating_sub(amount_r);

        let total_staked = u128::from_le_bytes(bytes[0..16].try_into().unwrap());
        let new_total_staked = total_staked.checked_add(amount_s).ok_or(Error::Overflow)?;
        let reward_pool_held = u128::from_le_bytes(bytes[40..56].try_into().unwrap());
        let new_reward_pool_held = reward_pool_held.checked_add(amount_r).ok_or(Error::Overflow)?;

        let mut out = bytes.to_vec();
        out[0..16].copy_from_slice(&new_total_staked.to_le_bytes());
        out[40..56].copy_from_slice(&new_reward_pool_held.to_le_bytes());
        out[base + 32..base + 48].copy_from_slice(&staked_restored.to_le_bytes());
        out[base + 48..base + 64].copy_from_slice(&reward_debt_restored.to_le_bytes());
        out[base + 72] = 0;
        for b in &mut out[base + 73..base + ROW_BYTES] {
            *b = 0;
        }
        Ok(out)
    }

    // ---- byte-level codec, matching docs/SPEC.md's "Golden vectors" layout exactly ----

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_BYTES + self.rows.len() * ROW_BYTES];
        out[0..16].copy_from_slice(&self.total_staked.to_le_bytes());
        out[16..32].copy_from_slice(&self.acc_reward_per_share.to_le_bytes());
        out[32..36].copy_from_slice(&self.stake_currency_id.to_le_bytes());
        out[36..40].copy_from_slice(&self.reward_currency_id.to_le_bytes());
        out[40..56].copy_from_slice(&self.reward_pool_held.to_le_bytes());
        out[56..72].copy_from_slice(&self.min_stake.to_le_bytes());
        out[72..76].copy_from_slice(&(self.rows.len() as u32).to_le_bytes());
        out[76..84].copy_from_slice(&self.next_pending_id.to_le_bytes());
        out[84..100].copy_from_slice(&self.fuel_reserve.to_le_bytes());
        for (i, row) in self.rows.iter().enumerate() {
            let base = HEADER_BYTES + i * ROW_BYTES;
            out[base..base + 32].copy_from_slice(&row.account);
            out[base + 32..base + 48].copy_from_slice(&row.staked.to_le_bytes());
            out[base + 48..base + 64].copy_from_slice(&row.reward_debt.to_le_bytes());
            out[base + 64..base + 72].copy_from_slice(&row.seq.to_le_bytes());
            match row.pending {
                None => out[base + 72] = 0,
                Some(p) => {
                    out[base + 72] = 1;
                    out[base + 73..base + 81].copy_from_slice(&p.pending_id.to_le_bytes());
                    out[base + 81..base + 97].copy_from_slice(&p.amount_s.to_le_bytes());
                    out[base + 97..base + 113].copy_from_slice(&p.amount_r.to_le_bytes());
                }
            }
        }
        out
    }

    /// Validates, beyond basic length/shape checks, that
    /// (a) accounts are strictly sorted and unique (the invariant every
    /// other method relies on via `binary_search_by`), (b) `row_count <=
    /// MAX_ROWS`, (c) every row's `staked <= MAX_AMOUNT`. A state that
    /// fails any of these could not have been produced by this crate's own
    /// transitions, so decoding it is treated as corrupt input, not a
    /// recoverable case.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_BYTES {
            return Err(Error::DecodeError);
        }
        let total_staked = u128::from_le_bytes(bytes[0..16].try_into().unwrap());
        let acc_reward_per_share = u128::from_le_bytes(bytes[16..32].try_into().unwrap());
        let stake_currency_id = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        let reward_currency_id = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
        let reward_pool_held = u128::from_le_bytes(bytes[40..56].try_into().unwrap());
        let min_stake = u128::from_le_bytes(bytes[56..72].try_into().unwrap());
        if !(1..=MAX_AMOUNT).contains(&min_stake) {
            return Err(Error::InvalidMinStake);
        }
        if stake_currency_id == SHELL_CURRENCY_ID || reward_currency_id == SHELL_CURRENCY_ID {
            return Err(Error::CurrencyCannotBeShell);
        }
        let row_count = u32::from_le_bytes(bytes[72..76].try_into().unwrap()) as usize;
        if row_count > MAX_ROWS {
            return Err(Error::TooManyRows);
        }
        let next_pending_id = u64::from_le_bytes(bytes[76..84].try_into().unwrap());
        let fuel_reserve = u128::from_le_bytes(bytes[84..100].try_into().unwrap());
        if !(1..=MAX_AMOUNT).contains(&fuel_reserve) {
            return Err(Error::InvalidFuelReserve);
        }
        if bytes.len() != HEADER_BYTES + row_count * ROW_BYTES {
            return Err(Error::DecodeError);
        }
        let mut rows = Vec::with_capacity(row_count);
        for i in 0..row_count {
            let base = HEADER_BYTES + i * ROW_BYTES;
            let account: [u8; 32] = bytes[base..base + 32].try_into().unwrap();
            let staked = u128::from_le_bytes(bytes[base + 32..base + 48].try_into().unwrap());
            if staked > MAX_AMOUNT {
                return Err(Error::ExceedsMaxAmount);
            }
            let reward_debt = u128::from_le_bytes(bytes[base + 48..base + 64].try_into().unwrap());
            let seq = u64::from_le_bytes(bytes[base + 64..base + 72].try_into().unwrap());
            let pending = match bytes[base + 72] {
                0 => None,
                1 => {
                    let pending_id =
                        u64::from_le_bytes(bytes[base + 73..base + 81].try_into().unwrap());
                    // `pending_id` is assigned from `next_pending_id` before
                    // it advances (see `next_pending_id_pair`), so no row can
                    // hold a value that has not yet been handed out, and 0
                    // is reserved (the SDK rejects a zero callback method_id).
                    if pending_id == 0 || pending_id >= next_pending_id {
                        return Err(Error::DecodeError);
                    }
                    let amount_s =
                        u128::from_le_bytes(bytes[base + 81..base + 97].try_into().unwrap());
                    let amount_r =
                        u128::from_le_bytes(bytes[base + 97..base + 113].try_into().unwrap());
                    Some(PendingOut { pending_id, amount_s, amount_r })
                }
                _ => return Err(Error::DecodeError),
            };
            if let Some(prev) = rows.last() {
                let prev: &Row = prev;
                if prev.account.as_slice() >= account.as_slice() {
                    return Err(Error::DecodeError); // not strictly sorted / duplicate
                }
            }
            rows.push(Row { account, staked, reward_debt, seq, pending });
        }
        Ok(State {
            total_staked,
            acc_reward_per_share,
            stake_currency_id,
            reward_currency_id,
            reward_pool_held,
            min_stake,
            fuel_reserve,
            next_pending_id,
            rows,
        })
    }
}
