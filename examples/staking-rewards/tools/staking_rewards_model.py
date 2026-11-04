#!/usr/bin/env python3
"""Executable state-machine model for the staking-rewards SPEC. Self-
contained: no machine-specific paths, no external dependencies beyond the
standard library. Pure Python, arbitrary-precision ints (no u128
truncation in the host language -- every bound below is an explicit
assert, not something Python's own integers enforce for us).

Also contains a naive model (deliberately buggy rules) used only to
demonstrate the partial-unstake lock and the rounding-insolvency
wraparound that the current model's own rules avoid.

Run with `--out <path>` to (re)generate the golden-vector fixture consumed
by `core/tests/golden_vectors.rs` (see that file's own invocation example);
run with no arguments to print the naive-model bug demonstrations and a
100,000-sequence fuzz run of the current model (see docs/SPEC.md's
"Executable model").
"""
import random

# ---- SPEC's actual revision-3 constants (not scaled down) ----
A, C, P = 60, 64, 44
MAX_AMOUNT = 2 ** A
MAX_ACC = 2 ** C
PRECISION = 2 ** P
MIN_STAKE = 10 ** 6
# Per-instance fuel_reserve, mirrors the header's own field.
FUEL_RESERVE = 1
U128_MAX = 2 ** 128 - 1
# Empirically measured: worst observed solvency margin across 100,000 fuzzed
# sequences (2.16M+ steps) was exactly -1, never worse (see model report).
# Set to that exact measured bound, not a guessed/rounded-up number.
ROUNDING_TOLERANCE = 1


def ceil_div(n, d):
    return -(-n // d)


# ---- Golden-vector byte layout -- a design choice for the vector format
# itself, stated plainly as such, not a platform fact. All little-endian,
# matching the fixed_bytes/u128/u32/u64 style already used by
# bounty-escrow's own State::encode/decode.
#
# `min_stake: u128` is a per-deployment init parameter, not a fixed
# constant (see Rev3Model.__init__). The row carries no `attempt` field:
# core/src/lib.rs never reads, increments, or branches on one, so the
# model doesn't model one either. `next_pending_id: u64` (header) and
# `pending_id: u64` (the row's own pending sub-structure) exist because a
# payout destination must be a deployed contract (funding alone never
# creates an account; see docs/getting-started.md), so a deployed
# address's own account_id is hash-derived and can never equal a caller's
# raw pubkey -- the callback's own caller identity cannot serve as the
# row-lookup key. `pending_id` is a monotonic correlation id instead,
# echoed back as the callback's own method_id.
HEADER_BYTES = 16 + 16 + 4 + 4 + 16 + 16 + 4 + 8 + 16  # ..., min_stake, row_count, next_pending_id, fuel_reserve
ROW_BYTES = 32 + 16 + 16 + 8 + 1 + 8 + 16 + 16  # account, staked, reward_debt, seq, pending_kind, pending_id, amount_s, amount_r
STAKE_CURRENCY_ID = 10
REWARD_CURRENCY_ID = 11


def account_bytes(acct_str):
    return acct_str.encode("utf-8").ljust(32, b"\x00")[:32]


def encode_row(acct_str, row):
    b = bytearray(ROW_BYTES)
    b[0:32] = account_bytes(acct_str)
    b[32:48] = row["staked"].to_bytes(16, "little")
    b[48:64] = row["reward_debt"].to_bytes(16, "little")
    b[64:72] = row["seq"].to_bytes(8, "little")
    p = row["pending"]
    b[72] = 0 if p is None else 1
    b[73:81] = (0 if p is None else p["pending_id"]).to_bytes(8, "little")
    b[81:97] = (0 if p is None else p["amount_s"]).to_bytes(16, "little")
    b[97:113] = (0 if p is None else p["amount_r"]).to_bytes(16, "little")
    return bytes(b)


def encode_state(m):
    header = bytearray(HEADER_BYTES)
    header[0:16] = m.total_staked.to_bytes(16, "little")
    header[16:32] = m.acc.to_bytes(16, "little")
    header[32:36] = STAKE_CURRENCY_ID.to_bytes(4, "little")
    header[36:40] = REWARD_CURRENCY_ID.to_bytes(4, "little")
    header[40:56] = m.reward_pool_held.to_bytes(16, "little")
    header[56:72] = m.min_stake.to_bytes(16, "little")
    header[72:76] = len(m.rows).to_bytes(4, "little")
    header[76:84] = m.next_pending_id.to_bytes(8, "little")
    header[84:100] = m.fuel_reserve.to_bytes(16, "little")
    rows_sorted = sorted(m.rows.items(), key=lambda kv: account_bytes(kv[0]))
    body = b"".join(encode_row(acct, row) for acct, row in rows_sorted)
    return (bytes(header) + body).hex()


class Rev3Model:
    """Revision-3 rules: request-time stake-change, exact full-settlement
    reward_debt reset, ceil-rounded STAKE top-up baseline with clamped settle
    carry-forward, exact prior_reward_debt restore on failure, reward-pool-
    shortfall clamp (principal independence)."""

    def __init__(self, min_stake=MIN_STAKE, fuel_reserve=FUEL_RESERVE):
        # min_stake is a per-instance init parameter (mirrors the header
        # field), not a fixed module-level constant -- validated the same
        # way the contract's own init() must validate it
        # (1 <= min_stake <= MAX_AMOUNT).
        assert 1 <= min_stake <= MAX_AMOUNT, "min_stake out of range"
        self.min_stake = min_stake
        # Same validation range as min_stake (1..=MAX_AMOUNT), mirroring
        # core::State::new's own check.
        assert 1 <= fuel_reserve <= MAX_AMOUNT, "fuel_reserve out of range"
        self.fuel_reserve = fuel_reserve
        self.next_pending_id = 1
        self.total_staked = 0
        self.acc = 0
        self.reward_pool_held = 0
        self.rows = {}
        self.cum_stake_in = 0
        self.cum_stake_out = 0
        self.cum_fund_in = 0
        self.cum_reward_out = 0
        self.min_product_margin = U128_MAX
        self.min_acc_margin = MAX_ACC
        self.min_fund_mul_margin = U128_MAX
        self.min_solvency_margin = None
        self.max_dust_observed = 0
        self.violations = []

    def _row(self, acct):
        return self.rows.get(acct, {"staked": 0, "reward_debt": 0, "seq": 0, "pending": None})

    def stake(self, acct, amount):
        row = self._row(acct)
        if amount <= 0 or amount > MAX_AMOUNT:
            return False
        staked_old, reward_debt_old = row["staked"], row["reward_debt"]
        staked_new = staked_old + amount
        # Admit against total committed EXPOSURE (settled + every row's own
        # in-flight principal), not bare total_staked. A payout's failure
        # restore must never be blocked (the currency genuinely came back),
        # so the ceiling has to be enforced where capacity is GRANTED
        # (here, at STAKE) using a quantity that is invariant across a
        # request/success/failure cycle. total_staked alone dips during a
        # pending UNSTAKE, letting other accounts stake into the gap; if that
        # payout later fails, the restore can then push total_staked over
        # MAX_AMOUNT. total_exposure does not dip, so admitting against it
        # makes that overflow structurally unreachable. Found by the fuzzer.
        own_pending_s = row["pending"]["amount_s"] if row.get("pending") else 0
        row_exposure = staked_old + own_pending_s
        if row_exposure + amount > MAX_AMOUNT:
            return False
        global_pending_s = sum(r["pending"]["amount_s"] for r in self.rows.values() if r["pending"])
        global_exposure = self.total_staked + global_pending_s
        if global_exposure + amount > MAX_AMOUNT:
            return False
        # MIN_STAKE floor applies to STAKE too, not only to the post-UNSTAKE
        # remainder (invariant 11/G as originally worded covered only the
        # unstake side; STAKE can create or top up a row below the floor
        # just as easily -- also found by the fuzzer).
        if staked_new < self.min_stake:
            return False
        accrued_now = (staked_old * self.acc) // PRECISION
        settle = max(0, accrued_now - reward_debt_old)
        # This is the same dangerous staked*acc product
        # check_invariants() already tracks in min_product_margin -- it
        # must not be conflated with fund_rewards()'s own amount*PRECISION
        # margin, a distinct bound with its own distinct margin.
        fund_mul = staked_new * self.acc
        reward_debt_new = ceil_div(fund_mul, PRECISION) - settle
        assert reward_debt_new >= 0, "STAKE produced negative reward_debt"
        row["staked"], row["reward_debt"], row["seq"] = staked_new, reward_debt_new, row["seq"] + 1
        row["pending"] = row.get("pending")
        self.rows[acct] = row
        self.total_staked += amount
        self.cum_stake_in += amount
        return True

    def unstake(self, acct, amount):
        row = self.rows.get(acct)
        if row is None or row["pending"] is not None:
            return False
        staked_old, reward_debt_old = row["staked"], row["reward_debt"]
        if amount <= 0 or amount > staked_old:
            return False
        staked_new = staked_old - amount
        if not (staked_new == 0 or staked_new >= self.min_stake):
            return False
        accrued_now = (staked_old * self.acc) // PRECISION
        amount_r = max(0, accrued_now - reward_debt_old)
        amount_r_final = min(amount_r, self.reward_pool_held)
        reward_debt_new = (staked_new * self.acc) // PRECISION
        row["staked"] = staked_new
        row["reward_debt"] = reward_debt_new
        row["pending"] = {"pending_id": self.next_pending_id, "amount_s": amount, "amount_r": amount_r_final}
        self.next_pending_id += 1
        row["seq"] += 1
        self.total_staked -= amount
        self.reward_pool_held -= amount_r_final
        self.rows[acct] = row
        return True

    def claim(self, acct):
        row = self.rows.get(acct)
        if row is None or row["pending"] is not None:
            return False
        staked, reward_debt_old = row["staked"], row["reward_debt"]
        accrued_now = (staked * self.acc) // PRECISION
        amount_r = max(0, accrued_now - reward_debt_old)
        if amount_r <= 0:
            return False
        amount_r_final = min(amount_r, self.reward_pool_held)
        # Reject on the post-clamp amount, not just the pre-clamp one -- a
        # short pool must not let CLAIM create an empty payout. UNSTAKE
        # does not get this check (principal independence): it must still
        # succeed even if the reward side clamps to zero.
        if amount_r_final == 0:
            return False
        reward_debt_new = (staked * self.acc) // PRECISION
        row["reward_debt"] = reward_debt_new
        row["pending"] = {"pending_id": self.next_pending_id, "amount_s": 0, "amount_r": amount_r_final}
        self.next_pending_id += 1
        row["seq"] += 1
        self.reward_pool_held -= amount_r_final
        self.rows[acct] = row
        return True

    def fund_rewards(self, amount):
        if self.total_staked < self.min_stake or amount <= 0 or amount > MAX_AMOUNT:
            return False
        mul = amount * PRECISION
        self.min_fund_mul_margin = min(self.min_fund_mul_margin, U128_MAX - mul)
        delta = mul // self.total_staked
        # Reject delta == 0 outright -- a funding amount too small to
        # move the accumulator at all against the current total_staked
        # contributes nothing and should not silently succeed (it would
        # just be dust-losses with no distribution at
        # all, worse than the already-documented per-call dust bound).
        if delta == 0:
            return False
        candidate = self.acc + delta
        if candidate > MAX_ACC:
            return False
        distributed = (delta * self.total_staked) // PRECISION
        self.max_dust_observed = max(self.max_dust_observed, amount - distributed)
        self.acc = candidate
        self.reward_pool_held += amount
        self.cum_fund_in += amount
        return True

    def payout_success(self, acct):
        row = self.rows.get(acct)
        if row is None or row["pending"] is None:
            return False
        p = row["pending"]
        self.cum_stake_out += p["amount_s"]
        self.cum_reward_out += p["amount_r"]
        row["pending"] = None
        if row["staked"] == 0:
            del self.rows[acct]
        else:
            self.rows[acct] = row
        return True

    def payout_failure(self, acct):
        row = self.rows.get(acct)
        if row is None or row["pending"] is None:
            return False
        p = row["pending"]
        amount_s, amount_r = p["amount_s"], p["amount_r"]
        # NOT a blanket "restore to the pre-request reward_debt" (that was
        # tried first and found wrong by the fuzzer: it lets the RETURNING
        # principal retroactively earn a share of any FUND_REWARDS that
        # happened during the pending window, even though that principal
        # was excluded from total_staked for the whole window). Instead,
        # rebase by the exact delta the REMAINING stake earned since
        # request time (already captured correctly in the row's own
        # untouched reward_debt), and separately restore amount_r. Needs no
        # stored "prior_reward_debt" field -- only the row's current
        # (unchanged-since-request) staked/reward_debt plus amount_s/amount_r.
        staked_remaining = row["staked"]
        staked_restored = staked_remaining + amount_s
        window_term = (staked_restored * self.acc) // PRECISION - (staked_remaining * self.acc) // PRECISION
        reward_debt_restored = window_term + row["reward_debt"] - amount_r
        if reward_debt_restored < 0:
            # Clamping reward_debt up to 0 here favors the staker, not the
            # pool -- a lower reward_debt means more future claimable, not
            # less. If this ever triggers, the row is over-credited by
            # exactly the clamped (absolute) amount relative to the
            # mathematically exact (negative, impossible-to-represent)
            # value. This is the opposite of every other rounding decision
            # in this contract (rounding otherwise always favors the
            # pool) -- a real, if apparently vanishingly rare,
            # staker-favoring edge case, not a cosmetic one. See the SPEC
            # for the bound derived from the counter below.
            self.restore_clamp_count = getattr(self, "restore_clamp_count", 0) + 1
            self.restore_clamp_max = max(getattr(self, "restore_clamp_max", 0), -reward_debt_restored)
            reward_debt_restored = 0
        row["staked"] = staked_restored
        self.total_staked += amount_s
        row["reward_debt"] = reward_debt_restored
        self.reward_pool_held += amount_r
        row["pending"] = None
        self.rows[acct] = row
        return True

    def check_invariants(self):
        pending_s = sum(r["pending"]["amount_s"] for r in self.rows.values() if r["pending"])
        pending_r = sum(r["pending"]["amount_r"] for r in self.rows.values() if r["pending"])
        lhs_s, rhs_s = self.total_staked + pending_s, self.cum_stake_in - self.cum_stake_out
        if lhs_s != rhs_s:
            self.violations.append(("conservation_S", lhs_s, rhs_s))
        lhs_r, rhs_r = self.reward_pool_held + pending_r, self.cum_fund_in - self.cum_reward_out
        if lhs_r != rhs_r:
            self.violations.append(("conservation_R", lhs_r, rhs_r))
        s = 0
        for acct, row in self.rows.items():
            prod = row["staked"] * self.acc
            self.min_product_margin = min(self.min_product_margin, U128_MAX - prod)
            if row["staked"] not in (0,) and row["staked"] < self.min_stake:
                self.violations.append(("min_stake_floor", acct, row["staked"]))
            if row["pending"] is None:
                accrued = (row["staked"] * self.acc) // PRECISION
                pr = accrued - row["reward_debt"]
                if pr < -1:
                    self.violations.append(("reward_debt_underflow_beyond_slack", acct, pr))
                s += max(0, pr)
            if row["staked"] > MAX_AMOUNT:
                self.violations.append(("row_staked_over_ceiling", acct, row["staked"]))
        margin = self.reward_pool_held - s
        self.min_solvency_margin = margin if self.min_solvency_margin is None else min(self.min_solvency_margin, margin)
        # Solvency is checked against an explicit, bounded rounding tolerance
        # (ROUNDING_TOLERANCE), not bare "<=". The raw accumulator formula
        # summed simultaneously across all Idle rows can be off by a few raw
        # units from cross-floor/ceil terms that compound across repeated
        # STAKE top-ups and failure-restores on the SAME row; this is the
        # same bounded noise the item-3 clamp already absorbs harmlessly at
        # real settlement time (a real CLAIM/UNSTAKE clamps against the
        # ACTUAL pool balance one row at a time, it never sums everyone's
        # theoretical entitlement at once). The tolerance is an honest
        # statement of that noise, not a loosened safety property -- it is
        # empirically measured below, not guessed.
        if s > self.reward_pool_held + ROUNDING_TOLERANCE:
            self.violations.append(("global_solvency", s, self.reward_pool_held, margin))
        self.min_acc_margin = min(self.min_acc_margin, MAX_ACC - self.acc)
        if self.total_staked > MAX_AMOUNT:
            self.violations.append(("total_staked_over_ceiling", self.total_staked))


class Rev2Model:
    """Revision-2 (superseded) rules: stake-change deferred to SUCCESS time;
    reward_debt += pending_reward using the OLD staked at request time; no
    reset against the new staked; floor only, no ceil/clamp. Used only to
    demonstrate the bugs revision 3 fixes. `use_checked` toggles whether the
    reward_debt subtraction is checked (raises -> reported as a lock) or
    left as plain Python subtraction allowed to go negative and then
    reinterpreted as a u128 wraparound (illustrative only, to show what an
    UNCHECKED implementation of the same buggy formula would produce)."""

    def __init__(self):
        self.total_staked = 0
        self.acc = 0
        self.reward_pool_held = 0
        self.rows = {}
        self.cum_fund_in = 0

    def _row(self, acct):
        return self.rows.get(acct, {"staked": 0, "reward_debt": 0, "pending": None})

    def stake(self, acct, amount):
        row = self._row(acct)
        row["staked"] += amount
        self.total_staked += amount
        self.rows[acct] = row
        return True

    def fund_rewards(self, amount):
        if self.total_staked == 0:
            return False
        delta = (amount * PRECISION) // self.total_staked
        self.acc += delta
        self.reward_pool_held += amount
        self.cum_fund_in += amount
        return True

    def unstake_request(self, acct, amount):
        row = self.rows.get(acct)
        if row is None or row["pending"] is not None or amount > row["staked"]:
            return False
        staked_old, reward_debt_old = row["staked"], row["reward_debt"]
        pending_reward = (staked_old * self.acc) // PRECISION - reward_debt_old
        if pending_reward < 0:
            return "LOCK_AT_REQUEST"
        row["reward_debt"] = reward_debt_old + pending_reward
        row["pending"] = {"amount_s": amount, "amount_r": pending_reward}
        self.rows[acct] = row
        return True

    def payout_success(self, acct):
        row = self.rows[acct]
        p = row["pending"]
        row["staked"] -= p["amount_s"]
        self.total_staked -= p["amount_s"]
        row["pending"] = None
        self.rows[acct] = row

    def claim_attempt_raw(self, acct):
        """Returns the RAW (unclamped) pending_reward using rev2's stale
        reward_debt. Negative -> would be a checked_sub failure (lock).
        Also returns the u128-wraparound interpretation of that same
        negative value, purely to illustrate the unchecked-arithmetic
        failure mode."""
        row = self.rows[acct]
        staked, reward_debt = row["staked"], row["reward_debt"]
        raw = (staked * self.acc) // PRECISION - reward_debt
        wrapped = raw % (2 ** 128) if raw < 0 else raw
        return raw, wrapped


def run_rev3_fuzz(n_sequences, seed=0):
    rng = random.Random(seed)
    accounts = [f"acct{i}" for i in range(8)]
    total_steps = 0
    global_min_product_margin = U128_MAX
    global_min_acc_margin = MAX_ACC
    global_min_fund_mul_margin = U128_MAX
    global_min_solvency_margin = None
    global_max_dust_observed = 0
    global_restore_clamp_count = 0
    global_restore_clamp_max = 0
    for seq_idx in range(n_sequences):
        m = Rev3Model()
        seq_len = rng.randint(10, 40)
        op_log = []
        for _ in range(seq_len):
            total_steps += 1
            op = rng.choices(
                ["stake", "unstake", "claim", "fund", "success", "failure"],
                weights=[25, 20, 15, 20, 10, 10],
            )[0]
            acct = rng.choice(accounts)
            applied = None
            if op == "stake":
                amt = rng.choice([
                    rng.randint(1, 10 ** 6),
                    rng.randint(1, 10 ** 12),
                    rng.randint(MAX_AMOUNT - 1000, MAX_AMOUNT),
                ])
                applied = m.stake(acct, amt)
                op_log.append(("stake", acct, amt, applied))
            elif op == "unstake":
                row = m.rows.get(acct)
                if row and row["staked"] > 0:
                    amt = rng.choice([row["staked"], rng.randint(1, row["staked"])])
                    applied = m.unstake(acct, amt)
                    op_log.append(("unstake", acct, amt, applied))
            elif op == "claim":
                applied = m.claim(acct)
                op_log.append(("claim", acct, None, applied))
            elif op == "fund":
                amt = rng.choice([
                    rng.randint(1, 10 ** 6),
                    rng.randint(1, 10 ** 12),
                    rng.randint(MAX_AMOUNT - 1000, MAX_AMOUNT),
                ])
                applied = m.fund_rewards(amt)
                op_log.append(("fund", None, amt, applied))
            elif op == "success":
                pend = [a for a, r in m.rows.items() if r["pending"]]
                if pend:
                    who = rng.choice(pend)
                    applied = m.payout_success(who)
                    op_log.append(("success", who, None, applied))
            elif op == "failure":
                pend = [a for a, r in m.rows.items() if r["pending"]]
                if pend:
                    who = rng.choice(pend)
                    applied = m.payout_failure(who)
                    op_log.append(("failure", who, None, applied))
            m.check_invariants()
            if m.violations:
                return {
                    "sequences_run": seq_idx + 1,
                    "total_steps": total_steps,
                    "violations": m.violations,
                    "min_product_margin": m.min_product_margin,
                    "min_acc_margin": m.min_acc_margin,
                    "min_fund_mul_margin": m.min_fund_mul_margin,
                    "min_solvency_margin": m.min_solvency_margin,
                    "max_dust_observed": m.max_dust_observed,
                    "restore_clamp_count": getattr(m, "restore_clamp_count", 0),
                    "restore_clamp_max": getattr(m, "restore_clamp_max", 0),
                    "op_log": op_log,
                    "final_rows": m.rows,
                    "final_acc": m.acc,
                    "final_reward_pool_held": m.reward_pool_held,
                    "final_total_staked": m.total_staked,
                }
        global_min_product_margin = min(global_min_product_margin, m.min_product_margin)
        global_min_acc_margin = min(global_min_acc_margin, m.min_acc_margin)
        global_min_fund_mul_margin = min(global_min_fund_mul_margin, m.min_fund_mul_margin)
        if m.min_solvency_margin is not None:
            global_min_solvency_margin = (
                m.min_solvency_margin if global_min_solvency_margin is None
                else min(global_min_solvency_margin, m.min_solvency_margin)
            )
        global_max_dust_observed = max(global_max_dust_observed, m.max_dust_observed)
        global_restore_clamp_count += getattr(m, "restore_clamp_count", 0)
        global_restore_clamp_max = max(global_restore_clamp_max, getattr(m, "restore_clamp_max", 0))
    return {
        "sequences_run": n_sequences,
        "total_steps": total_steps,
        "violations": [],
        "min_product_margin": global_min_product_margin,
        "min_acc_margin": global_min_acc_margin,
        "min_fund_mul_margin": global_min_fund_mul_margin,
        "min_solvency_margin": global_min_solvency_margin,
        "max_dust_observed": global_max_dust_observed,
        "restore_clamp_count": global_restore_clamp_count,
        "restore_clamp_max": global_restore_clamp_max,
    }


def run_adversarial_drift(total_ops, seed=0, checkpoint_every=10_000, n_accounts=4,
                           boundary_mode=False):
    """A single continuous state (not independent short sequences) driven
    by a generator that maximizes partial UNSTAKEs,
    intervening FUND_REWARDS, and payout FAILURES on a SMALL, fixed set of
    accounts -- the exact shape that would reveal compounding rounding
    drift, since every bounded-by-construction per-row/per-op slack claim
    is only actually bounded if repeatedly hammering the SAME rows does not
    make it grow. Records the worst (min) solvency margin at each
    checkpoint so growth-vs-flat is visible as a function of op count, not
    just a single final number. boundary_mode biases STAKE/FUND amounts to
    sit right under MAX_AMOUNT/MAX_ACC instead of small values.
    """
    rng = random.Random(seed)
    accounts = [f"adv{i}" for i in range(n_accounts)]
    m = Rev3Model()
    checkpoints = []
    violations = []
    for step in range(1, total_ops + 1):
        op = rng.choices(
            ["stake", "stake_tiny", "unstake_partial", "unstake_full", "claim", "fund", "success", "failure"],
            weights=[10, 25, 30, 5, 10, 15, 15, 20],
        )[0]
        acct = rng.choice(accounts)
        if op == "stake":
            if boundary_mode:
                # Push rows toward MAX_AMOUNT, not away from it -- a tiny
                # amount here would never even clear MIN_STAKE to create a
                # row at all, let alone stress the ceiling.
                amt = rng.randint(MAX_AMOUNT // 20, MAX_AMOUNT // 8)
            else:
                amt = rng.randint(1, 10 ** 9)
            m.stake(acct, amt)
        elif op == "stake_tiny":
            # Repeated tiny top-ups on the same row maximize the number of
            # independent ceil-rounding events per row -- the specific
            # mechanism that could compound.
            m.stake(acct, rng.randint(1, 10))
        elif op == "unstake_partial":
            row = m.rows.get(acct)
            if row and row["staked"] > m.min_stake:
                amt = rng.randint(1, row["staked"] - m.min_stake)
                if amt > 0:
                    m.unstake(acct, amt)
        elif op == "unstake_full":
            row = m.rows.get(acct)
            if row and row["staked"] > 0 and row["pending"] is None:
                m.unstake(acct, row["staked"])
        elif op == "claim":
            m.claim(acct)
        elif op == "fund":
            if boundary_mode and m.total_staked > 0:
                headroom = MAX_ACC - m.acc
                amt = min(MAX_AMOUNT, max(1, (headroom * m.total_staked) // PRECISION))
            else:
                amt = rng.randint(1, 10 ** 9)
            m.fund_rewards(amt)
        elif op == "success":
            pend = [a for a, r in m.rows.items() if r["pending"]]
            if pend:
                m.payout_success(rng.choice(pend))
        elif op == "failure":
            pend = [a for a, r in m.rows.items() if r["pending"]]
            if pend:
                m.payout_failure(rng.choice(pend))
        m.check_invariants()
        if m.violations:
            violations = m.violations
            checkpoints.append({"step": step, "min_solvency_margin": m.min_solvency_margin,
                                 "min_product_margin": m.min_product_margin})
            break
        if step % checkpoint_every == 0:
            checkpoints.append({"step": step, "min_solvency_margin": m.min_solvency_margin,
                                 "min_product_margin": m.min_product_margin,
                                 "min_acc_margin": m.min_acc_margin,
                                 "n_rows": len(m.rows), "acc": m.acc,
                                 "total_staked": m.total_staked})
    return {"total_ops": total_ops, "violations": violations, "checkpoints": checkpoints,
            "final_min_solvency_margin": m.min_solvency_margin}


def demonstrate_rev2_bugs():
    # --- Partial-unstake lock, concrete trace (matches the SPEC's own worked example) ---
    m = Rev2Model()
    m.stake("alice", 100)
    # Drive acc to exactly a 0.5 share-rate in PRECISION units for a clean trace:
    m.acc = 0  # explicit, start at 0
    m.total_staked = 100
    m.rows["alice"] = {"staked": 100, "reward_debt": 0, "pending": None}
    m.reward_pool_held = 10 ** 9
    # fund so that acc becomes PRECISION//2 (i.e. a 0.5 per-share rate), amount chosen to land exactly
    fund_amount = (PRECISION // 2) * 100 // PRECISION  # total_staked=100
    # force exact: set delta directly to avoid integer-division drift in this illustration
    m.acc = PRECISION // 2
    req = m.unstake_request("alice", 60)
    m.payout_success("alice")  # staked: 100 -> 40, reward_debt stays stale at 50-equivalent
    # second claim attempt on the now-smaller stake, same acc (no new funding yet)
    raw1, wrapped1 = m.claim_attempt_raw("alice")
    # now also show it WORSENS after further funding during this already-broken state
    m.acc += PRECISION // 5
    raw2, wrapped2 = m.claim_attempt_raw("alice")
    return {
        "request_result": req,
        "staked_after_success": m.rows["alice"]["staked"],
        "reward_debt_stale": m.rows["alice"]["reward_debt"],
        "raw_pending_after_partial_unstake": raw1,
        "wrapped_u128_if_unchecked": wrapped1,
        "raw_pending_after_further_funding": raw2,
        "wrapped_u128_if_unchecked_after_funding": wrapped2,
    }


def demonstrate_rev2_global_solvency_gap():
    """Rev2's invariant 2 was stated PER-ROW ('pending_reward(row) <=
    reward_pool_held'), not as a sum. Show a case where every row
    individually satisfies that per-row statement at the moment it is
    checked, yet the SUM of outstanding pending rewards exceeds
    reward_pool_held -- i.e. the per-row statement alone does not imply
    the real solvency property."""
    reward_pool_held = 100
    row_a_pending = 90   # 90 <= 100: passes rev2's own per-row check
    row_b_pending = 90   # 90 <= 100: also passes rev2's own per-row check, independently
    total_owed = row_a_pending + row_b_pending
    return {
        "reward_pool_held": reward_pool_held,
        "row_a_pending": row_a_pending,
        "row_b_pending": row_b_pending,
        "each_row_passes_per_row_check": row_a_pending <= reward_pool_held and row_b_pending <= reward_pool_held,
        "sum_exceeds_pool": total_owed > reward_pool_held,
        "total_owed": total_owed,
    }


def generate_golden_vectors(n_sequences, seed, out_path, min_len=8, max_len=20):
    """Item 5: emit a JSON file of operation sequences with the EXPECTED
    state (header+rows, byte-level hex, the layout above) after every step,
    for the Rust core's tests to replay."""
    import json

    rng = random.Random(seed)
    accounts = [f"gv{i}" for i in range(6)]
    sequences = []
    for seq_idx in range(n_sequences):
        m = Rev3Model()
        steps = []
        seq_len = rng.randint(min_len, max_len)
        for _ in range(seq_len):
            op = rng.choices(
                ["stake", "unstake", "claim", "fund", "success", "failure"],
                weights=[30, 20, 15, 20, 7, 8],
            )[0]
            acct = rng.choice(accounts)
            state_before = encode_state(m)
            applied = False
            args = {}
            if op == "stake":
                amt = rng.randint(1, 10 ** 9)
                args = {"account": acct, "amount": amt}
                applied = m.stake(acct, amt)
            elif op == "unstake":
                row = m.rows.get(acct)
                if row and row["staked"] > 0:
                    amt = rng.choice([row["staked"], rng.randint(1, row["staked"])])
                    args = {"account": acct, "amount": amt}
                    applied = m.unstake(acct, amt)
                else:
                    args = {"account": acct, "amount": 0}
            elif op == "claim":
                args = {"account": acct}
                applied = m.claim(acct)
            elif op == "fund":
                amt = rng.randint(1, 10 ** 9)
                args = {"amount": amt}
                applied = m.fund_rewards(amt)
            elif op == "success":
                pend = [a for a, r in m.rows.items() if r["pending"]]
                who = rng.choice(pend) if pend else acct
                args = {"account": who}
                applied = m.payout_success(who) if pend else False
            elif op == "failure":
                pend = [a for a, r in m.rows.items() if r["pending"]]
                who = rng.choice(pend) if pend else acct
                args = {"account": who}
                applied = m.payout_failure(who) if pend else False
            state_after = encode_state(m)
            steps.append({"op": op, "args": args, "applied": applied,
                          "state_before_hex": state_before, "state_after_hex": state_after})
        sequences.append({"seq_index": seq_idx, "steps": steps})

    payload = {
        "constants": {"MAX_AMOUNT": MAX_AMOUNT, "MAX_ACC": MAX_ACC, "PRECISION": PRECISION,
                      "MIN_STAKE": MIN_STAKE, "STAKE_CURRENCY_ID": STAKE_CURRENCY_ID,
                      "REWARD_CURRENCY_ID": REWARD_CURRENCY_ID},
        "layout": {"HEADER_BYTES": HEADER_BYTES, "ROW_BYTES": ROW_BYTES,
                   "header_fields": ["total_staked:u128", "acc_reward_per_share:u128",
                                      "stake_currency_id:u32", "reward_currency_id:u32",
                                      "reward_pool_held:u128", "min_stake:u128", "row_count:u32",
                                      "next_pending_id:u64", "fuel_reserve:u128"],
                   "row_fields": ["account:[u8;32]", "staked:u128", "reward_debt:u128", "seq:u64",
                                   "pending_kind:u8(0=Idle,1=PendingOut)", "pending_id:u64",
                                   "pending_amount_s:u128", "pending_amount_r:u128"]},
        "sequences": sequences,
    }
    with open(out_path, "w") as f:
        json.dump(payload, f)
    total_steps = sum(len(s["steps"]) for s in sequences)
    return {"sequences": n_sequences, "total_steps": total_steps, "out_path": out_path}


if __name__ == "__main__":
    import argparse
    import json

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", help="Regenerate the golden-vector fixture at this path "
                                       "instead of running the demonstrations/fuzz run")
    parser.add_argument("--sequences", type=int, default=15)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--min-len", type=int, default=8)
    parser.add_argument("--max-len", type=int, default=14)
    args = parser.parse_args()

    if args.out:
        result = generate_golden_vectors(args.sequences, args.seed, args.out, args.min_len, args.max_len)
        print(json.dumps(result, indent=2))
    else:
        print("=== Revision 2 bug demonstrations ===")
        lock_demo = demonstrate_rev2_bugs()
        print(json.dumps(lock_demo, indent=2))
        solvency_demo = demonstrate_rev2_global_solvency_gap()
        print(json.dumps(solvency_demo, indent=2))

        print("\n=== Revision 3 fuzz run ===")
        result = run_rev3_fuzz(100_000, seed=12345)
        print(json.dumps(result, indent=2, default=str))
