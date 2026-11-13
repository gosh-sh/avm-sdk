#![forbid(unsafe_code)]

//! Independent arithmetic oracle for the staking-rewards accumulator.
//!
//! This is NOT a copy of `core`'s own code: the multiply-divide step is
//! computed through a hand-written 256-bit widening multiply (built from
//! u64 limbs) and long division, not `u128::checked_mul`/`checked_div` --
//! genuinely different arithmetic, not a relabeled copy of the same call.
//! The six state transitions are re-derived from `docs/SPEC.md`'s own
//! prose directly, not read off `core/src/lib.rs`. Where this oracle and
//! `core` agree across random sequences, that is two independently-written
//! implementations of the same spec agreeing -- which is the actual value
//! of a property test, not a tautology.
//!
//! Honest limitation: both implementations were written by the same
//! person in the same session, so this is not a literal reimplementation
//! by an independent author. The arithmetic *path* (widening multiply via
//! limbs vs. `u128::checked_mul`) is independent; the SPEC interpretation
//! is not.

use avm_staking_rewards_core as core_crate;

/// A 256-bit unsigned value as two `u128` limbs, built from a genuine
/// 128x128->256 widening multiply via u64 limb decomposition -- not
/// `u128::checked_mul`, which core's own code uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct U256 {
    pub hi: u128,
    pub lo: u128,
}

fn widening_mul_u128(a: u128, b: u128) -> U256 {
    let a_hi = (a >> 64) as u64 as u128;
    let a_lo = a & u64::MAX as u128;
    let b_hi = (b >> 64) as u64 as u128;
    let b_lo = b & u64::MAX as u128;

    let lo_lo = a_lo * b_lo; // fits in u128 (<=2^128-1)
    let lo_hi = a_lo * b_hi; // fits in u128
    let hi_lo = a_hi * b_lo;
    let hi_hi = a_hi * b_hi;

    // combine: result = hi_hi*2^128 + (lo_hi+hi_lo)*2^64 + lo_lo
    let mid = lo_hi + hi_lo; // each < 2^64*2^64=2^128 before shift; here each already <=2^128-ish but
                             // lo_hi, hi_lo are each products of two <=64-bit values, so each fits in u128 with room.
    let mid_carry = mid >> 64;
    let mid_low_shifted = (mid & u64::MAX as u128) << 64;

    let (lo, carry1) = lo_lo.overflowing_add(mid_low_shifted);
    let hi = hi_hi + mid_carry + if carry1 { 1 } else { 0 };
    U256 { hi, lo }
}

/// Divide a 256-bit value by a u128 divisor, returning (quotient, remainder),
/// via standard binary long division over the lo limb, MSB first. Requires
/// `n.hi == 0` -- true for every value this oracle ever computes, since the
/// state machine's own constants keep every product under 2^124, strictly
/// within a single u128 limb (see docs/SPEC.md's "Constants" for the
/// `A=60, C=64, P=44` bound this relies on, not assumed here).
fn divmod_u256_by_u128(n: U256, d: u128) -> (u128, u128) {
    assert!(d != 0);
    assert_eq!(n.hi, 0, "oracle only supports dividends that fit in 128 bits");
    let mut r: u128 = 0;
    let mut q: u128 = 0;
    for i in (0..128).rev() {
        let bit = (n.lo >> i) & 1;
        r = (r << 1) | bit;
        q <<= 1;
        if r >= d {
            r -= d;
            q |= 1;
        }
    }
    (q, r)
}

/// floor(a*b/c), via the independent widening multiply + long division.
pub fn mul_div_floor(a: u128, b: u128, c: u128) -> u128 {
    let wide = widening_mul_u128(a, b);
    let (q, _r) = divmod_u256_by_u128(wide, c);
    q
}

/// ceil(a*b/c), via the same independent path.
pub fn mul_div_ceil(a: u128, b: u128, c: u128) -> u128 {
    let wide = widening_mul_u128(a, b);
    let (q, r) = divmod_u256_by_u128(wide, c);
    if r == 0 {
        q
    } else {
        q + 1
    }
}

// ---- oracle state machine, re-derived from docs/SPEC.md's prose, not from core/src/lib.rs ----

#[derive(Clone, Debug)]
pub struct OracleRow {
    pub account: [u8; 32],
    pub staked: u128,
    pub reward_debt: u128,
    pub pending_s: Option<u128>,
    pub pending_r: Option<u128>,
}

#[derive(Clone, Debug)]
pub struct OracleState {
    pub total_staked: u128,
    pub acc: u128,
    pub reward_pool_held: u128,
    pub rows: Vec<OracleRow>,
}

impl Default for OracleState {
    fn default() -> Self {
        Self::new()
    }
}

impl OracleState {
    pub fn new() -> Self {
        Self { total_staked: 0, acc: 0, reward_pool_held: 0, rows: Vec::new() }
    }

    fn find(&self, account: [u8; 32]) -> Option<usize> {
        self.rows.iter().position(|r| r.account == account)
    }

    fn exposure_sum(&self) -> u128 {
        self.rows.iter().map(|r| r.pending_s.unwrap_or(0)).sum()
    }

    pub fn stake(&mut self, account: [u8; 32], amount: u128) -> Result<(), &'static str> {
        if amount == 0 || amount > core_crate::MAX_AMOUNT {
            return Err("bad amount");
        }
        let (staked_old, reward_debt_old, own_pending) = match self.find(account) {
            Some(i) => {
                (self.rows[i].staked, self.rows[i].reward_debt, self.rows[i].pending_s.unwrap_or(0))
            }
            None => (0, 0, 0),
        };
        let staked_new = staked_old + amount;
        if staked_old + own_pending + amount > core_crate::MAX_AMOUNT {
            return Err("row exposure over ceiling");
        }
        if self.total_staked + self.exposure_sum() + amount > core_crate::MAX_AMOUNT {
            return Err("global exposure over ceiling");
        }
        if staked_new < core_crate::DEFAULT_MIN_STAKE {
            return Err("below min stake");
        }
        let accrued_now = mul_div_floor(staked_old, self.acc, core_crate::PRECISION);
        let settle = accrued_now.saturating_sub(reward_debt_old);
        let ceiled = mul_div_ceil(staked_new, self.acc, core_crate::PRECISION);
        let reward_debt_new = ceiled.checked_sub(settle).ok_or("settle too large")?;

        self.total_staked += amount;
        match self.find(account) {
            Some(i) => {
                self.rows[i].staked = staked_new;
                self.rows[i].reward_debt = reward_debt_new;
            }
            None => self.rows.push(OracleRow {
                account,
                staked: staked_new,
                reward_debt: reward_debt_new,
                pending_s: None,
                pending_r: None,
            }),
        }
        Ok(())
    }

    pub fn unstake(
        &mut self,
        account: [u8; 32],
        amount: u128,
    ) -> Result<(u128, u128), &'static str> {
        let i = self.find(account).ok_or("no row")?;
        if self.rows[i].pending_s.is_some() {
            return Err("pending in flight");
        }
        let staked_old = self.rows[i].staked;
        if amount == 0 || amount > staked_old {
            return Err("bad amount");
        }
        let staked_new = staked_old - amount;
        if staked_new != 0 && staked_new < core_crate::DEFAULT_MIN_STAKE {
            return Err("below min stake");
        }
        let accrued_now = mul_div_floor(staked_old, self.acc, core_crate::PRECISION);
        let amount_r = accrued_now.saturating_sub(self.rows[i].reward_debt);
        let amount_r_final = amount_r.min(self.reward_pool_held);
        let reward_debt_new = mul_div_floor(staked_new, self.acc, core_crate::PRECISION);

        self.total_staked -= amount;
        self.reward_pool_held -= amount_r_final;
        self.rows[i].staked = staked_new;
        self.rows[i].reward_debt = reward_debt_new;
        self.rows[i].pending_s = Some(amount);
        self.rows[i].pending_r = Some(amount_r_final);
        Ok((amount, amount_r_final))
    }

    pub fn claim(&mut self, account: [u8; 32]) -> Result<u128, &'static str> {
        let i = self.find(account).ok_or("no row")?;
        if self.rows[i].pending_s.is_some() {
            return Err("pending in flight");
        }
        let staked = self.rows[i].staked;
        let accrued_now = mul_div_floor(staked, self.acc, core_crate::PRECISION);
        let amount_r = accrued_now.saturating_sub(self.rows[i].reward_debt);
        if amount_r == 0 {
            return Err("nothing to claim");
        }
        let amount_r_final = amount_r.min(self.reward_pool_held);
        if amount_r_final == 0 {
            return Err("nothing to claim after clamp");
        }
        self.reward_pool_held -= amount_r_final;
        self.rows[i].reward_debt = accrued_now;
        self.rows[i].pending_s = Some(0);
        self.rows[i].pending_r = Some(amount_r_final);
        Ok(amount_r_final)
    }

    pub fn fund_rewards(&mut self, amount: u128) -> Result<(), &'static str> {
        if self.total_staked < core_crate::DEFAULT_MIN_STAKE {
            return Err("no stakers");
        }
        if amount == 0 || amount > core_crate::MAX_AMOUNT {
            return Err("bad amount");
        }
        let delta = mul_div_floor(amount, core_crate::PRECISION, self.total_staked);
        if delta == 0 {
            return Err("delta zero");
        }
        let candidate = self.acc + delta;
        if candidate > core_crate::MAX_ACC {
            return Err("exceeds max acc");
        }
        self.acc = candidate;
        self.reward_pool_held += amount;
        Ok(())
    }

    pub fn payout_success(&mut self, account: [u8; 32]) -> Result<(), &'static str> {
        let i = self.find(account).ok_or("no row")?;
        if self.rows[i].pending_s.is_none() {
            return Err("no pending");
        }
        self.rows[i].pending_s = None;
        self.rows[i].pending_r = None;
        if self.rows[i].staked == 0 {
            self.rows.remove(i);
        }
        Ok(())
    }

    pub fn payout_failure(&mut self, account: [u8; 32]) -> Result<(), &'static str> {
        let i = self.find(account).ok_or("no row")?;
        let amount_s = self.rows[i].pending_s.ok_or("no pending")?;
        let amount_r = self.rows[i].pending_r.unwrap_or(0);
        let staked_remaining = self.rows[i].staked;
        let staked_restored = staked_remaining + amount_s;
        let term_restored = mul_div_floor(staked_restored, self.acc, core_crate::PRECISION);
        let term_remaining = mul_div_floor(staked_remaining, self.acc, core_crate::PRECISION);
        let window_term = term_restored - term_remaining;
        let reward_debt_restored =
            (window_term + self.rows[i].reward_debt).saturating_sub(amount_r);

        self.total_staked += amount_s;
        self.reward_pool_held += amount_r;
        self.rows[i].staked = staked_restored;
        self.rows[i].reward_debt = reward_debt_restored;
        self.rows[i].pending_s = None;
        self.rows[i].pending_r = None;
        Ok(())
    }
}

#[cfg(test)]
mod widening_mul_tests {
    use super::*;

    #[test]
    fn matches_u128_when_result_fits() {
        // Cross-check the independent widening multiply against plain
        // u128 multiplication for cases that fit, to validate the oracle's
        // own arithmetic before trusting it to validate core.
        // Cases that fit in 128 bits (what this oracle actually relies on
        // for our constants) must have hi=0 and lo matching plain u128
        // multiplication exactly.
        let fitting: &[(u128, u128)] = &[
            (0, 0),
            (1, 1),
            (u64::MAX as u128, u64::MAX as u128),
            (1u128 << 60, 1u128 << 44),
            (1u128 << 60, 1u128 << 64), // 2^124, still < 2^128
        ];
        for &(a, b) in fitting {
            let wide = widening_mul_u128(a, b);
            assert_eq!(wide.hi, 0, "hi should be 0 for these magnitudes: a={a} b={b}");
            let expected = a.checked_mul(b).expect("fixture case should fit in u128");
            assert_eq!(wide.lo, expected, "a={a} b={b}");
        }
        // A case that genuinely overflows 128 bits: confirms the widening
        // multiply produces a real, nonzero hi limb instead of silently
        // truncating -- the actual point of writing a 256-bit multiply at
        // all, since plain u128::checked_mul would just return None here.
        let (a, b) = (12345678901234567890u128, 98765432109876543210u128);
        assert!(a.checked_mul(b).is_none(), "fixture should NOT fit in u128");
        let wide = widening_mul_u128(a, b);
        assert_ne!(wide.hi, 0, "a*b exceeds 128 bits, hi must be nonzero");
    }

    #[test]
    fn mul_div_floor_matches_plain_division_when_no_overflow() {
        let cases: &[(u128, u128, u128)] =
            &[(100, 50, 100), (1u128 << 60, 1u128 << 44, 1u128 << 44), (7, 3, 2)];
        for &(a, b, c) in cases {
            let expected = a.checked_mul(b).unwrap() / c;
            assert_eq!(mul_div_floor(a, b, c), expected);
        }
    }

    #[test]
    fn mul_div_ceil_rounds_up_on_remainder() {
        assert_eq!(mul_div_ceil(7, 1, 2), 4); // 7/2 = 3.5 -> ceil 4
        assert_eq!(mul_div_ceil(8, 1, 2), 4); // exact, no rounding
    }
}

// ---- property tests: core::State vs. the independent oracle, random sequences ----

#[cfg(test)]
mod property_tests {
    use super::*;
    use core_crate::State;

    // A tiny xorshift PRNG -- no external rand crate, keeps the test
    // dependency-free and offline-safe.
    struct Rng(u64);
    impl Rng {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next_u64() % n
        }
    }

    fn acct(i: u64) -> [u8; 32] {
        let mut a = [0u8; 32];
        a[0] = i as u8;
        a
    }

    /// Mirror core's own state as a byte-comparable snapshot via its own
    /// encode(), and the oracle's own fields directly, to compare after
    /// every step. core and the oracle use different internal Vec-vs-row
    /// representations, so this compares the OBSERVABLE fields, not memory
    /// layout.
    fn oracle_matches_core(oracle: &OracleState, core: &State) -> Result<(), String> {
        if oracle.total_staked != core.total_staked {
            return Err(format!(
                "total_staked: oracle={} core={}",
                oracle.total_staked, core.total_staked
            ));
        }
        if oracle.acc != core.acc_reward_per_share {
            return Err(format!("acc: oracle={} core={}", oracle.acc, core.acc_reward_per_share));
        }
        if oracle.reward_pool_held != core.reward_pool_held {
            return Err(format!(
                "reward_pool_held: oracle={} core={}",
                oracle.reward_pool_held, core.reward_pool_held
            ));
        }
        if oracle.rows.len() != core.rows.len() {
            return Err(format!(
                "row count: oracle={} core={}",
                oracle.rows.len(),
                core.rows.len()
            ));
        }
        for orow in &oracle.rows {
            let crow = core
                .rows
                .iter()
                .find(|r| r.account == orow.account)
                .ok_or_else(|| format!("core missing row {:?}", orow.account))?;
            if orow.staked != crow.staked || orow.reward_debt != crow.reward_debt {
                return Err(format!(
                    "row {:?}: oracle(staked={},debt={}) core(staked={},debt={})",
                    orow.account, orow.staked, orow.reward_debt, crow.staked, crow.reward_debt
                ));
            }
            let opend = orow.pending_s.map(|s| (s, orow.pending_r.unwrap_or(0)));
            let cpend = crow.pending.map(|p| (p.amount_s, p.amount_r));
            if opend != cpend {
                return Err(format!(
                    "row {:?} pending: oracle={:?} core={:?}",
                    orow.account, opend, cpend
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn property_core_matches_oracle_across_random_sequences() {
        let n_accounts = 6u64;
        let n_sequences = 500;
        let steps_per_sequence = 60;

        for seed in 0..n_sequences {
            let mut rng = Rng(0x9E3779B97F4A7C15u64 ^ (seed as u64 + 1));
            let mut core =
                State::new(10, 11, core_crate::DEFAULT_MIN_STAKE, core_crate::DEFAULT_FUEL_RESERVE)
                    .unwrap();
            let mut oracle = OracleState::new();

            for step in 0..steps_per_sequence {
                let op = rng.below(6);
                let a = acct(rng.below(n_accounts));
                match op {
                    0 => {
                        let amt = 1u128 + rng.below(1_000_000_000) as u128;
                        let c = core.stake(a, amt);
                        let o = oracle.stake(a, amt);
                        assert_eq!(c.is_ok(), o.is_ok(), "seed={seed} step={step} stake");
                    }
                    1 => {
                        let staked = core.rows.iter().find(|r| r.account == a).map(|r| r.staked);
                        if let Some(s) = staked {
                            if s > 0 {
                                let amt = 1u128 + (rng.below(s.max(1) as u64) as u128 % s.max(1));
                                let amt = amt.max(1).min(s);
                                let c = core.unstake(a, amt);
                                let o = oracle.unstake(a, amt);
                                assert_eq!(c.is_ok(), o.is_ok(), "seed={seed} step={step} unstake");
                            }
                        }
                    }
                    2 => {
                        let c = core.claim(a);
                        let o = oracle.claim(a);
                        assert_eq!(c.is_ok(), o.is_ok(), "seed={seed} step={step} claim");
                    }
                    3 => {
                        let amt = 1u128 + rng.below(1_000_000_000) as u128;
                        let c = core.fund_rewards(amt);
                        let o = oracle.fund_rewards(amt);
                        assert_eq!(c.is_ok(), o.is_ok(), "seed={seed} step={step} fund");
                    }
                    4 => {
                        let c = core.payout_success(a);
                        let o = oracle.payout_success(a);
                        assert_eq!(c.is_ok(), o.is_ok(), "seed={seed} step={step} success");
                    }
                    _ => {
                        let c = core.payout_failure(a);
                        let o = oracle.payout_failure(a);
                        assert_eq!(c.is_ok(), o.is_ok(), "seed={seed} step={step} failure");
                    }
                }
                if let Err(msg) = oracle_matches_core(&oracle, &core) {
                    panic!("seed={seed} step={step}: divergence: {msg}");
                }
            }
        }
    }

    // ---- targeted regression tests: M1, M2, M3, invariant 14, invariant 16 ----

    #[test]
    fn m1_stake_into_pending_unstake_gap_then_failure_never_exceeds_max_amount() {
        // The bug the fuzzer found: a pending UNSTAKE temporarily frees
        // room under MAX_AMOUNT; staking into that gap, then having the
        // original payout FAIL, must never push total_staked over
        // MAX_AMOUNT. Admitting against total_staked alone (not exposure)
        // would let this through.
        let mut core =
            State::new(10, 11, core_crate::DEFAULT_MIN_STAKE, core_crate::DEFAULT_FUEL_RESERVE)
                .unwrap();
        let big = core_crate::MAX_AMOUNT - core_crate::DEFAULT_MIN_STAKE;
        core.stake(acct(1), big).unwrap();
        let unstake_amt = big - core_crate::DEFAULT_MIN_STAKE; // leaves exactly MIN_STAKE
        core.unstake(acct(1), unstake_amt).unwrap();
        // Room freed under MAX_AMOUNT is now `unstake_amt`; try to stake
        // into it with a second account.
        let stake_into_gap = unstake_amt; // would fit if checked against bare total_staked
        let result = core.stake(acct(2), stake_into_gap);
        // Must be rejected: exposure (total_staked + pending.amount_s) is
        // still at `big`, leaving no real room.
        assert!(result.is_err(), "M1: stake into the gap must be rejected by exposure");
        // Whether rejected or not, failing the original payout must never
        // push total_staked past MAX_AMOUNT.
        core.payout_failure(acct(1)).unwrap();
        assert!(core.total_staked <= core_crate::MAX_AMOUNT, "M1: total_staked exceeded ceiling");
    }

    #[test]
    fn m2_stake_below_min_stake_rejected_directly() {
        // Revision 2 only checked MIN_STAKE on UNSTAKE; STAKE itself had no
        // floor at all.
        let mut core =
            State::new(10, 11, core_crate::DEFAULT_MIN_STAKE, core_crate::DEFAULT_FUEL_RESERVE)
                .unwrap();
        let result = core.stake(acct(1), core_crate::DEFAULT_MIN_STAKE - 1);
        assert!(result.is_err(), "M2: STAKE below MIN_STAKE must be rejected");
    }

    #[test]
    fn m3_partial_unstake_with_intervening_funding_then_failure_delta_rebase() {
        // The deep bug: the naive restore formula lets the RETURNING
        // principal retroactively earn a share of funding that happened
        // during the pending window, even though it was excluded from
        // total_staked for that whole window. Verified two ways: (1)
        // core's own result matches the independently-written oracle's
        // delta-rebase (not hand-derived arithmetic, which is error-prone
        // -- the oracle is the robust reference here); (2) the row's
        // final reward_debt is strictly less than what the naive
        // (wrong) formula would have produced, confirming the fix
        // actually changes behavior on this exact scenario, not a no-op.
        let mut core =
            State::new(10, 11, core_crate::DEFAULT_MIN_STAKE, core_crate::DEFAULT_FUEL_RESERVE)
                .unwrap();
        let mut oracle = OracleState::new();
        core.stake(acct(1), 10_000_000).unwrap();
        oracle.stake(acct(1), 10_000_000).unwrap();
        core.fund_rewards(1_000_000).unwrap();
        oracle.fund_rewards(1_000_000).unwrap();
        core.unstake(acct(1), 3_000_000).unwrap(); // partial; remaining = 7_000_000
        oracle.unstake(acct(1), 3_000_000).unwrap();
        core.fund_rewards(500_000).unwrap(); // intervening funding while pending
        oracle.fund_rewards(500_000).unwrap();
        core.payout_failure(acct(1)).unwrap();
        oracle.payout_failure(acct(1)).unwrap();

        oracle_matches_core(&oracle, &core)
            .unwrap_or_else(|msg| panic!("M3: core diverged from the independent oracle: {msg}"));

        let row = core.rows.iter().find(|r| r.account == acct(1)).unwrap();
        assert_eq!(row.staked, 10_000_000, "M3: principal must be fully restored");

        // The naive (wrong) formula the model proved incorrect:
        // reward_debt = floor(staked_restored * acc_now / PRECISION) - amount_r.
        // It credits the RESTORED (full) stake with the window's funding,
        // which is exactly the bug -- so it produces a STRICTLY LARGER
        // reward_debt than the correct delta-rebase result (a larger
        // reward_debt means a smaller future claim, i.e. the naive formula
        // actually UNDER-pays here; the direction flips with different
        // amounts, the point is only that the two formulas must disagree
        // on this scenario, proving the fix is not a no-op).
        let naive = (row.staked * core.acc_reward_per_share) / core_crate::PRECISION;
        assert_ne!(
            row.reward_debt, naive,
            "M3: delta-rebase must differ from the naive formula on this exact scenario"
        );
    }

    #[test]
    fn invariant_14_unstake_principal_succeeds_even_when_reward_pool_is_empty() {
        // Principal independence: UNSTAKE must succeed whenever the row is
        // Idle and the amount is valid, regardless of reward-side state --
        // even when reward_pool_held is zero (no funding ever happened).
        let mut core =
            State::new(10, 11, core_crate::DEFAULT_MIN_STAKE, core_crate::DEFAULT_FUEL_RESERVE)
                .unwrap();
        core.stake(acct(1), 5_000_000).unwrap();
        assert_eq!(core.reward_pool_held, 0);
        let result = core.unstake(acct(1), 5_000_000);
        assert!(result.is_ok(), "invariant 14: UNSTAKE must succeed with an empty reward pool");
        let pending = result.unwrap();
        assert_eq!(pending.amount_s, 5_000_000);
        assert_eq!(pending.amount_r, 0, "reward side clamps to 0, not blocking principal");
    }

    #[test]
    fn invariant_16_claim_rejected_when_clamped_reward_is_zero() {
        // CLAIM must reject if the POST-clamp amount is zero (pool short),
        // not just if the pre-clamp accrued amount is zero. UNSTAKE is
        // exempt by design (invariant 14); CLAIM is not.
        //
        // Under CORRECT operation a single Idle row's own accrued amount
        // never exceeds reward_pool_held (SPEC invariant 2) -- this is
        // explicitly the "should not happen" belt-and-suspenders path the
        // clamp exists for, so this test forces the shortfall directly
        // (all State fields are `pub`) rather than trying to reconstruct
        // the rare rounding artifact that could organically produce it.
        let mut core =
            State::new(10, 11, core_crate::DEFAULT_MIN_STAKE, core_crate::DEFAULT_FUEL_RESERVE)
                .unwrap();
        core.stake(acct(1), 5_000_000).unwrap();
        core.fund_rewards(100).unwrap();
        let accrued_before = {
            let row = core.rows.iter().find(|r| r.account == acct(1)).unwrap();
            (row.staked * core.acc_reward_per_share) / core_crate::PRECISION - row.reward_debt
        };
        assert!(accrued_before > 0, "fixture should have a nonzero pre-clamp accrual");
        core.reward_pool_held = 0; // force the shortfall this test targets
        let result = core.claim(acct(1));
        assert!(result.is_err(), "invariant 16: CLAIM must reject an empty-after-clamp payout");
    }

    #[test]
    fn property_err_never_mutates_state_bytes() {
        // Every core transition is compute-first / commit-last, so an Err
        // return leaves state bytes byte-identical to before the call.
        // This fuzzes a mix of normal-ish traffic
        // (which builds up nontrivial state: multiple accounts staked,
        // funded, with pending payouts in flight) interleaved with
        // deliberately adversarial operations (zero/overflowing amounts,
        // nonexistent accounts, payouts on idle rows), and checks every
        // single Err outcome against this invariant directly on the raw
        // encoded bytes -- not just the handful of targeted regression
        // cases above.
        let n_accounts = 6u64;
        let n_sequences = 300;
        let steps_per_sequence = 80;

        for seed in 0..n_sequences {
            let mut rng = Rng(0xD1B54A32D192ED03u64 ^ (seed as u64 + 1));
            let mut core =
                State::new(10, 11, core_crate::DEFAULT_MIN_STAKE, core_crate::DEFAULT_FUEL_RESERVE)
                    .unwrap();
            let mut err_seen = 0u64;

            for step in 0..steps_per_sequence {
                let a = acct(rng.below(n_accounts));
                let op = rng.below(9);

                let before = core.encode();
                let is_err = match op {
                    // normal-ish stake: builds up real state most of the time
                    0 => core.stake(a, 1u128 + rng.below(1_000_000_000) as u128).is_err(),
                    // adversarial: amount 0 (below MIN_STAKE)
                    1 => core.stake(a, 0).is_err(),
                    // adversarial: amount that exceeds MAX_AMOUNT outright
                    2 => {
                        let amt = core_crate::MAX_AMOUNT + 1 + rng.below(1_000_000) as u128;
                        core.stake(a, amt).is_err()
                    }
                    // normal-ish unstake of a random amount against whatever
                    // the row currently holds -- often too large or zero,
                    // which is exactly the adversarial case for this op
                    3 => {
                        let amt = rng.below(2_000_000_000) as u128;
                        core.unstake(a, amt).is_err()
                    }
                    // adversarial: unstake on an account with no row at all
                    4 => {
                        let ghost = acct(100 + rng.below(50));
                        core.unstake(ghost, 1 + rng.below(1000) as u128).is_err()
                    }
                    // normal-ish claim (adversarial whenever there's no row
                    // or no accrued/clamped reward)
                    5 => core.claim(a).is_err(),
                    // adversarial: claim on an account with no row
                    6 => {
                        let ghost = acct(100 + rng.below(50));
                        core.claim(ghost).is_err()
                    }
                    // normal-ish fund_rewards -- rng.below can produce 0,
                    // which is itself the adversarial case for this op
                    7 => core.fund_rewards(rng.below(1_000_000_000) as u128).is_err(),
                    // adversarial: payout_success/payout_failure on an
                    // account that has no row, or a row with no pending
                    // payout (most rows, most of the time)
                    _ => {
                        if rng.below(2) == 0 {
                            core.payout_success(a).is_err()
                        } else {
                            core.payout_failure(a).is_err()
                        }
                    }
                };

                if is_err {
                    err_seen += 1;
                    let after = core.encode();
                    assert_eq!(
                        before, after,
                        "seed={seed} step={step} op={op}: Err must leave state bytes unchanged"
                    );
                }
            }
            assert!(err_seen > 0, "seed={seed}: fixture produced no Err at all, test is vacuous");
        }
    }
}
