#!/usr/bin/env python3
"""Small-constant exhaustive check. Usage: python3 tools/exhaustive_check.py [depth]
(default depth 7; depth 8 reproduces the figures cited in docs/SPEC.md).

Overrides staking_rewards_model's module-level constants with tiny values
(limits reachable within a short sequence), then DFS-enumerates EVERY
operation sequence up to a bounded length over a bounded argument set,
asserting all invariants after every step. States are memoized by
(state_signature, remaining_depth) so identical subtrees are not
re-explored -- this does not skip checking any sequence's invariants (every
node is checked once, on first visit), it only avoids re-deriving the same
guarantee from the same state twice.
"""
import copy
import sys

import staking_rewards_model as mdl

# ---- tiny, reachable constants (overriding the module's real ones) ----
mdl.PRECISION = 2 ** 3          # 8
mdl.MIN_STAKE = 2
mdl.MAX_AMOUNT = 10
mdl.MAX_ACC = 16
mdl.U128_MAX = 2 ** 128 - 1     # overflow margin is irrelevant at this scale; keep real ceiling
mdl.ROUNDING_TOLERANCE = 1

ACCOUNTS = ["a0", "a1"]
AMOUNTS = [1, 2, 3]  # small, but spans below/at/above MIN_STAKE=2


def state_signature(m):
    rows_sig = tuple(sorted(
        (a, r["staked"], r["reward_debt"], r["seq"],
         None if r["pending"] is None else (r["pending"]["amount_s"], r["pending"]["amount_r"]))
        for a, r in m.rows.items()
    ))
    return (m.total_staked, m.acc, m.reward_pool_held, rows_sig)


def clone(m):
    # NOTE: Rev3Model.__init__'s `min_stake=MIN_STAKE` default is bound at
    # class-DEFINITION time (a standard Python gotcha), so it still captures
    # the module's ORIGINAL MIN_STAKE even after `mdl.MIN_STAKE = 2` above --
    # that override only affects NEW references to the module global looked
    # up at call time, not an already-bound default parameter. Passing
    # min_stake explicitly (from the PARENT's own current value, not the
    # module constant) is what actually propagates it correctly.
    m2 = mdl.Rev3Model(min_stake=m.min_stake)
    m2.total_staked = m.total_staked
    m2.acc = m.acc
    m2.reward_pool_held = m.reward_pool_held
    m2.next_pending_id = m.next_pending_id
    m2.rows = copy.deepcopy(m.rows)
    m2.cum_stake_in = m.cum_stake_in
    m2.cum_stake_out = m.cum_stake_out
    m2.cum_fund_in = m.cum_fund_in
    m2.cum_reward_out = m.cum_reward_out
    m2.min_product_margin = m.min_product_margin
    m2.min_acc_margin = m.min_acc_margin
    m2.min_fund_mul_margin = m.min_fund_mul_margin
    m2.min_solvency_margin = m.min_solvency_margin
    m2.max_dust_observed = m.max_dust_observed
    m2.restore_clamp_count = getattr(m, "restore_clamp_count", 0)
    m2.restore_clamp_max = getattr(m, "restore_clamp_max", 0)
    m2.violations = []
    return m2


def all_ops(m):
    """Every (op, args) combination worth trying from this state."""
    ops = []
    for acct in ACCOUNTS:
        for amt in AMOUNTS:
            ops.append(("stake", acct, amt))
    for acct in ACCOUNTS:
        row = m.rows.get(acct)
        if row:
            for amt in AMOUNTS:
                ops.append(("unstake", acct, amt))
            ops.append(("unstake", acct, row["staked"]))  # exact full unstake, always tried
    for acct in ACCOUNTS:
        ops.append(("claim", acct, None))
    for amt in AMOUNTS:
        ops.append(("fund", None, amt))
    for acct in ACCOUNTS:
        ops.append(("success", acct, None))
        ops.append(("failure", acct, None))
    return ops


def apply_op(m, op):
    kind, acct, amt = op
    if kind == "stake":
        return m.stake(acct, amt)
    if kind == "unstake":
        return m.unstake(acct, amt)
    if kind == "claim":
        return m.claim(acct)
    if kind == "fund":
        return m.fund_rewards(amt)
    if kind == "success":
        return m.payout_success(acct)
    if kind == "failure":
        return m.payout_failure(acct)
    raise ValueError(kind)


def run_exhaustive(max_depth, report_every=200_000):
    memo = set()
    sequences_checked = 0
    nodes_visited = 0
    counterexample = [None]
    clamp_triggers = [0]
    clamp_max_seen = [0]

    sys.setrecursionlimit(10_000)

    def dfs(m, depth_remaining, path):
        nonlocal sequences_checked, nodes_visited
        if counterexample[0] is not None:
            return
        sig = state_signature(m)
        key = (sig, depth_remaining)
        if key in memo:
            return
        memo.add(key)
        nodes_visited += 1
        if nodes_visited % report_every == 0:
            print(f"...{nodes_visited} nodes visited, {sequences_checked} leaf sequences, "
                  f"memo size {len(memo)}", flush=True)
        if depth_remaining == 0:
            sequences_checked += 1
            return
        parent_clamp_count = getattr(m, "restore_clamp_count", 0)
        for op in all_ops(m):
            child = clone(m)
            applied = apply_op(child, op)
            child.check_invariants()
            # Counts payout_failure's saturating_sub clamp triggers (the
            # staker-favoring edge case, see the model's own comment on it)
            # across this exhaustive run specifically.
            if child.restore_clamp_count > parent_clamp_count:
                clamp_triggers[0] += 1
                clamp_max_seen[0] = max(clamp_max_seen[0], child.restore_clamp_max)
            if child.violations:
                counterexample[0] = {"path": path + [(op, applied)], "violations": child.violations,
                                      "state": {"total_staked": child.total_staked, "acc": child.acc,
                                                "reward_pool_held": child.reward_pool_held,
                                                "rows": child.rows}}
                return
            dfs(child, depth_remaining - 1, path + [(op, applied)])
            if counterexample[0] is not None:
                return

    root = mdl.Rev3Model(min_stake=mdl.MIN_STAKE)
    dfs(root, max_depth, [])
    return {
        "max_depth": max_depth,
        "nodes_visited": nodes_visited,
        "leaf_sequences": sequences_checked,
        "memo_size": len(memo),
        "counterexample": counterexample[0],
        "restore_clamp_triggers": clamp_triggers[0],
        "restore_clamp_max_seen": clamp_max_seen[0],
    }


if __name__ == "__main__":
    depth = int(sys.argv[1]) if len(sys.argv) > 1 else 7
    result = run_exhaustive(depth)
    print("max_depth:", result["max_depth"])
    print("nodes_visited:", result["nodes_visited"])
    print("leaf_sequences:", result["leaf_sequences"])
    print("memo_size:", result["memo_size"])
    print("counterexample:", result["counterexample"])
    print("restore_clamp_triggers:", result["restore_clamp_triggers"])
    print("restore_clamp_max_seen:", result["restore_clamp_max_seen"])
