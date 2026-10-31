mod common;

use std::collections::BTreeMap;

use avm_contract_sdk::{
    decode_message_input, AccountAddress, Contract, EventIntent, MessageOutput,
};
use avm_vault_amm::{
    ids, math, payout_receipt, Amm, Config, State, ACCOUNT_SLOTS, ADD_LIQUIDITY_ARGS_BYTES,
    CONFIG_BYTES, ERR_ARGS, ERR_AUTH, ERR_BALANCE, ERR_CALLBACK, ERR_CAPACITY, ERR_DEADLINE,
    ERR_INSOLVENT, ERR_NO_LIQUIDITY, ERR_PAUSED, ERR_PENDING, ERR_SLIPPAGE, ERR_STATE, ERR_VALUE,
    ERR_ZERO, MINIMUM_LIQUIDITY, PAYOUT_RECEIPT_BYTES, PENDING_SLOTS, REMOVE_LIQUIDITY_ARGS_BYTES,
    SET_PARAMS_ARGS_BYTES, SHELL_CURRENCY_ID, STATE_BYTES, SWAP_ARGS_BYTES, WITHDRAW_ARGS_BYTES,
};
use common::*;

const MANIFEST: &str = include_str!("../../contracts/amm/avm-contract.toml");

fn sim() -> Sim {
    Sim::new(config(30, 0))
}

/// A pool seeded by Alice with `1_000_000` of each asset.
fn seeded(fee_bps: u16, protocol_bps: u16) -> Sim {
    let mut sim = Sim::new(config(fee_bps, protocol_bps));
    sim.seed(ALICE, 1_000_000, 1_000_000);
    sim
}

fn internal_call(sim: &Sim, source: AccountAddress, method: u64, body: &[u8]) -> MessageOutput {
    let message = internal_message(method, body, &[], source, addr(POOL), None, None, 0);
    let framed = frame(&message, &sim.state, None, Some(source), addr(POOL), sim.time, &sim.vault);
    Amm::process_message(decode_message_input(&framed).unwrap())
}

fn raw_call(sim: &Sim, state: &[u8], actor: [u8; 32], method: u64, body: &[u8]) -> MessageOutput {
    let message = external_message(body, actor, method, &[], addr(POOL), false);
    let framed = frame(&message, state, Some(actor), None, addr(POOL), sim.time, &sim.vault);
    Amm::process_message(decode_message_input(&framed).unwrap())
}

// ------------------------------------------------------------ manifest ---

fn manifest_type_len(name: &str) -> usize {
    let lines: Vec<&str> = MANIFEST.lines().collect();
    let at = lines.iter().position(|line| *line == format!("name = \"{name}\"")).unwrap();
    lines[at + 1].strip_prefix("len = ").unwrap().parse().unwrap()
}

fn manifest_events() -> BTreeMap<String, Vec<String>> {
    let mut events: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut current, mut expect_event, mut expect_field) = (String::new(), false, false);
    for line in MANIFEST.lines() {
        match line {
            "[[abi.events]]" => (expect_event, expect_field) = (true, false),
            "[[abi.events.fields]]" => (expect_event, expect_field) = (false, true),
            line if line.starts_with('[') => (expect_event, expect_field) = (false, false),
            line if line.starts_with("name = ") => {
                let name = line["name = ".len()..].trim_matches('"').to_string();
                if expect_event {
                    events.insert(name.clone(), Vec::new());
                    current = name;
                } else if expect_field {
                    events.get_mut(&current).unwrap().push(name);
                }
                (expect_event, expect_field) = (false, false);
            }
            _ => {}
        }
    }
    events
}

fn event_field_names(event: &EventIntent) -> Vec<String> {
    let bytes = &event.fields_binary;
    let count = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
    let mut position = 4;
    let mut names = Vec::new();
    for _ in 0..count {
        let name_len = usize::from(u16::from_le_bytes([bytes[position], bytes[position + 1]]));
        let value_len = usize::from(u16::from_le_bytes([bytes[position + 3], bytes[position + 4]]));
        position += 5;
        names.push(String::from_utf8(bytes[position..position + name_len].to_vec()).unwrap());
        position += name_len + value_len;
    }
    names
}

#[test]
fn manifest_lengths_match_the_code() {
    assert_eq!(manifest_type_len("state"), STATE_BYTES);
    assert_eq!(manifest_type_len("config"), CONFIG_BYTES);
    assert_eq!(manifest_type_len("add_liquidity_args"), ADD_LIQUIDITY_ARGS_BYTES);
    assert_eq!(manifest_type_len("remove_liquidity_args"), REMOVE_LIQUIDITY_ARGS_BYTES);
    assert_eq!(manifest_type_len("swap_args"), SWAP_ARGS_BYTES);
    assert_eq!(manifest_type_len("withdraw_args"), WITHDRAW_ARGS_BYTES);
    assert_eq!(manifest_type_len("set_params_args"), SET_PARAMS_ARGS_BYTES);
    assert_eq!(manifest_type_len("payout_receipt"), PAYOUT_RECEIPT_BYTES);
    assert_eq!(STATE_BYTES, 4376);
}

#[test]
fn manifest_error_codes_match_the_code() {
    let declared: Vec<u32> = MANIFEST
        .lines()
        .filter_map(|line| line.strip_prefix("status_code = "))
        .map(|code| code.parse().unwrap())
        .collect();
    let used = [
        ERR_ARGS,
        ERR_AUTH,
        ERR_STATE,
        ERR_PAUSED,
        ERR_VALUE,
        ERR_DEADLINE,
        ERR_SLIPPAGE,
        ERR_BALANCE,
        ERR_NO_LIQUIDITY,
        ERR_PENDING,
        ERR_CALLBACK,
        ERR_CAPACITY,
        ERR_INSOLVENT,
        avm_vault_amm::ERR_OVERFLOW,
        avm_vault_amm::ERR_INVARIANT,
        ERR_ZERO,
    ];
    assert_eq!(declared, used);
    assert!(declared.iter().all(|code| *code >= 1000), "codes below 1000 are reserved");
}

#[test]
fn identifiers_are_unique_nonzero_and_cover_every_event() {
    let manifest = manifest_events();
    let mut seen = std::collections::BTreeSet::new();
    for (name, id) in ids::EVENTS {
        assert!(manifest.contains_key(*name), "{name} missing from the manifest");
        assert!(*id != 0 && seen.insert(*id), "{name} has a duplicate or zero class");
    }
    assert_eq!(ids::EVENTS.len(), manifest.len());
    for id in [
        ids::DEPLOY,
        ids::DEPOSIT,
        ids::ADD_LIQUIDITY,
        ids::REMOVE_LIQUIDITY,
        ids::SWAP_EXACT_IN,
        ids::WITHDRAW,
        ids::CALLBACK,
        ids::COLLECT_FEES,
        ids::SET_PARAMS,
        ids::TOPUP,
        ids::RECEIVER_BIND,
        ids::RECEIVER_REJECT,
        ids::RECEIVER_RECEIVE,
    ] {
        assert!(id != 0 && seen.insert(id), "duplicate or zero method id");
    }
}

#[test]
fn every_emitted_event_matches_its_manifest_declaration() {
    let manifest = manifest_events();
    let mut sim = Sim::new(config(30, 5_000));
    let mut events: Vec<EventIntent> = sim.init_events.clone();
    let mut run = |output: MessageOutput| {
        accepted(&output);
        events.extend(output.event_intents);
    };
    run(sim.deposit(ALICE, 2_000_000, 2_000_000));
    run(sim.add_liquidity(ALICE, (1_000_000, 1_000_000), (0, 0), NEVER));
    run(sim.deposit(BOB, 50_000, 0));
    run(sim.swap_exact_in(BOB, 0, 10_000, 0));
    run(sim.remove_liquidity(ALICE, 1_000, (0, 0), NEVER));
    run(sim.call(ADMIN, ids::COLLECT_FEES, &[], &[]));
    run(sim.set_params(ADMIN, 25, 1_000, false));
    run(sim.call(MALLORY, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 5)]));
    run(sim.withdraw(ALICE, C0, 100, addr(5)));
    run(sim.settle(addr(5), Settle::Delivered(payout_receipt(C0, 0, 100))));
    run(sim.withdraw(ALICE, C1, 100, addr(6)));
    run(sim.settle(addr(6), Settle::Bounced(vec![(C1, 100)])));
    let mut emitted = std::collections::BTreeSet::new();
    for event in &events {
        let declared = manifest.get(&event.event_name).expect("event is declared");
        assert_eq!(&event_field_names(event), declared, "{}", event.event_name);
        let class = ids::EVENTS.iter().find(|(name, _)| *name == event.event_name).unwrap().1;
        assert_eq!(event.event_class, class);
        emitted.insert(event.event_name.clone());
    }
    assert_eq!(emitted.len(), manifest.len(), "the script must exercise every event");
}

// ----------------------------------------------------- init and config ---

#[test]
fn config_validation_rejects_every_malformed_form() {
    let good = config(30, 5_000);
    assert_eq!(Config::parse(&good.encode()), Some(good));
    type Mutation = Box<dyn Fn(&mut Config)>;
    let mutations: Vec<Mutation> = vec![
        Box::new(|c| c.admin = [0; 32]),
        Box::new(|c| c.currency = [9, 9]),
        Box::new(|c| c.currency = [9, 7]),
        Box::new(|c| c.currency = [SHELL_CURRENCY_ID, 9]),
        Box::new(|c| c.currency = [1, SHELL_CURRENCY_ID]),
        Box::new(|c| c.swap_fee_bps = 1_001),
        Box::new(|c| c.protocol_fee_bps = 5_001),
        Box::new(|c| c.fuel_reserve = 0),
    ];
    for mutate in mutations {
        let mut bad = good;
        mutate(&mut bad);
        assert_eq!(Config::parse(&bad.encode()), None);
    }
    let mut padded = good.encode();
    padded[45] = 1;
    assert_eq!(Config::parse(&padded), None);
    assert_eq!(Config::parse(&padded[..63]), None);
    assert_eq!(Config::parse(&[padded.clone(), vec![0]].concat()), None);
}

fn try_init(
    actor: [u8; 32],
    cfg: &Config,
    init: Option<&[u8]>,
    extra: &[(u32, u128)],
) -> MessageOutput {
    let init = init.map_or_else(|| cfg.encode(), <[u8]>::to_vec);
    let message = external_message(&envelope(&init), actor, 0, extra, addr(POOL), true);
    let framed = frame(&message, &[], Some(actor), None, addr(POOL), 10, &BTreeMap::new());
    Amm::init(decode_message_input(&framed).unwrap())
}

#[test]
fn init_is_admin_only_valueless_and_validated() {
    let cfg = config(30, 0);
    accepted(&try_init(ADMIN, &cfg, None, &[]));
    failure(&try_init(MALLORY, &cfg, None, &[]), ERR_AUTH);
    failure(&try_init(ADMIN, &cfg, None, &[(C0, 1)]), ERR_VALUE);
    failure(&try_init(ADMIN, &cfg, Some(&[0; 63]), &[]), ERR_ARGS);
    let mut bad = cfg;
    bad.currency = [SHELL_CURRENCY_ID, C1];
    failure(&try_init(ADMIN, &bad, None, &[]), ERR_ARGS);
    // An ordinary call against an uninitialized account is refused.
    let sim = sim();
    failure(&raw_call(&sim, &[], ADMIN, ids::DEPOSIT, &[]), ERR_STATE);
}

#[test]
fn state_layout_roundtrips_and_is_fixed_size() {
    let mut sim = seeded(30, 5_000);
    assert_eq!(sim.state.len(), STATE_BYTES);
    accepted(&sim.deposit(BOB, 10_000, 0));
    accepted(&sim.swap_exact_in(BOB, 0, 5_000, 0));
    accepted(&sim.deposit(ALICE, 5, 0));
    accepted(&sim.withdraw(ALICE, C0, 1, addr(5)));
    for state in [sim.state.clone(), sim.pool().encode()] {
        assert_eq!(State::decode(&state).unwrap().encode(), state);
    }
    assert_eq!(sim.pool().encode().len(), STATE_BYTES);
}

// ------------------------------------------------------------- deposits ---

#[test]
fn deposits_credit_the_callers_ledger_and_the_vault() {
    let mut sim = sim();
    accepted(&sim.deposit(ALICE, 100, 200));
    assert_eq!(sim.ledger(ALICE), ([100, 200], 0));
    assert_eq!((sim.balance(C0), sim.balance(C1)), (100, 200));
    accepted(&sim.deposit(ALICE, 0, 5));
    assert_eq!(sim.ledger(ALICE), ([100, 205], 0));
    sim.assert_accounting();
}

#[test]
fn deposits_reject_unexpected_value_and_callers() {
    let mut sim = sim();
    failure(&sim.call(ALICE, ids::DEPOSIT, &[], &[]), ERR_VALUE);
    failure(&sim.call(ALICE, ids::DEPOSIT, &[], &[(SHELL_CURRENCY_ID, 5)]), ERR_VALUE);
    failure(&sim.call(ALICE, ids::DEPOSIT, &[], &[(8, 5)]), ERR_VALUE);
    failure(&sim.call(ALICE, ids::DEPOSIT, &[], &[(C0, 0)]), ERR_VALUE);
    failure(&sim.call(ALICE, ids::DEPOSIT, &[], &[(SHELL_CURRENCY_ID, 1), (C0, 5)]), ERR_VALUE);
    failure(&sim.call(ALICE, ids::DEPOSIT, &[1], &[(C0, 5)]), ERR_ARGS);
    failure(&sim.call(ALICE, 0x1234, &[], &[]), ERR_ARGS);
    failure(&internal_call(&sim, addr(5), ids::DEPOSIT, &[]), ERR_AUTH);
    assert_eq!(sim.pool().accounts.len(), 0);
    sim.assert_accounting();
}

#[test]
fn the_ledger_has_a_fixed_number_of_holders_and_frees_slots() {
    let mut sim = sim();
    let holder = |index: usize| [0x60 + index as u8; 32];
    for index in 0..ACCOUNT_SLOTS {
        accepted(&sim.deposit(holder(index), 10, 0));
    }
    failure(&sim.deposit(holder(ACCOUNT_SLOTS), 10, 0), ERR_CAPACITY);
    // A holder that withdraws everything frees its slot once the payout settles.
    accepted(&sim.withdraw(holder(0), C0, 10, addr(5)));
    failure(&sim.deposit(holder(ACCOUNT_SLOTS), 10, 0), ERR_CAPACITY);
    accepted(&sim.settle(addr(5), Settle::Delivered(payout_receipt(C0, 0, 10))));
    accepted(&sim.deposit(holder(ACCOUNT_SLOTS), 10, 0));
    assert_eq!(sim.pool().accounts.len(), ACCOUNT_SLOTS);
    sim.assert_accounting();
}

// ------------------------------------------------------------ liquidity ---

#[test]
fn first_liquidity_mints_the_root_and_locks_the_minimum() {
    let sim = seeded(30, 0);
    let pool = sim.pool();
    assert_eq!(pool.reserve, [1_000_000, 1_000_000]);
    assert_eq!(pool.lp_total, 1_000_000);
    assert_eq!(sim.ledger(ALICE), ([0, 0], 999_000));
    assert_eq!(pool.locked_liquidity(), MINIMUM_LIQUIDITY);
    sim.assert_accounting();
    // Asymmetric seed: sqrt(5e5 * 2e6) = 1e6.
    let mut sim = Sim::new(config(30, 0));
    sim.seed(BOB, 500_000, 2_000_000);
    assert_eq!(sim.ledger(BOB).1, 999_000);
}

#[test]
fn dust_and_bounds_are_enforced_on_the_first_deposit() {
    let mut sim = sim();
    accepted(&sim.deposit(ALICE, 5_000, 5_000));
    failure(&sim.add_liquidity(ALICE, (1_000, 1_000), (0, 0), NEVER), ERR_ZERO);
    failure(&sim.add_liquidity(ALICE, (0, 1_000), (0, 0), NEVER), ERR_ARGS);
    failure(&sim.add_liquidity(ALICE, (6_000, 5_000), (0, 0), NEVER), ERR_BALANCE);
    failure(&sim.add_liquidity(ALICE, (5_000, 5_000), (5_001, 0), NEVER), ERR_SLIPPAGE);
    failure(&sim.add_liquidity(ALICE, (5_000, 5_000), (0, 0), 99), ERR_DEADLINE);
    failure(&sim.add_liquidity(BOB, (5_000, 5_000), (0, 0), NEVER), ERR_BALANCE);
    accepted(&sim.add_liquidity(ALICE, (1_001, 1_001), (0, 0), NEVER));
    assert_eq!(sim.ledger(ALICE).1, 1);
    sim.assert_accounting();
}

#[test]
fn later_deposits_take_the_pool_ratio_and_keep_the_excess() {
    let mut sim = seeded(30, 0);
    accepted(&sim.deposit(BOB, 500_000, 700_000));
    accepted(&sim.add_liquidity(BOB, (500_000, 700_000), (500_000, 500_000), NEVER));
    assert_eq!(sim.ledger(BOB), ([0, 200_000], 500_000));
    assert_eq!(sim.pool().reserve, [1_500_000, 1_500_000]);
    assert_eq!(sim.pool().lp_total, 1_500_000);
    // When the second asset limits, the first one is trimmed instead.
    accepted(&sim.deposit(CAROL, 900_000, 100_000));
    accepted(&sim.add_liquidity(CAROL, (900_000, 100_000), (100_000, 100_000), NEVER));
    assert_eq!(sim.ledger(CAROL), ([800_000, 0], 100_000));
    sim.assert_accounting();
}

#[test]
fn later_deposits_respect_slippage_bounds() {
    let mut sim = seeded(30, 0);
    accepted(&sim.deposit(BOB, 500_000, 700_000));
    failure(&sim.add_liquidity(BOB, (500_000, 700_000), (0, 500_001), NEVER), ERR_SLIPPAGE);
    failure(&sim.add_liquidity(BOB, (500_000, 700_000), (500_001, 0), NEVER), ERR_SLIPPAGE);
}

#[test]
fn removing_liquidity_pays_out_pro_rata_but_never_the_locked_minimum() {
    let mut sim = seeded(30, 0);
    failure(&sim.remove_liquidity(ALICE, 0, (0, 0), NEVER), ERR_ARGS);
    failure(&sim.remove_liquidity(ALICE, 999_001, (0, 0), NEVER), ERR_BALANCE);
    failure(&sim.remove_liquidity(BOB, 1, (0, 0), NEVER), ERR_BALANCE);
    failure(&sim.remove_liquidity(ALICE, 1_000, (1_001, 0), NEVER), ERR_SLIPPAGE);
    failure(&sim.remove_liquidity(ALICE, 1_000, (0, 0), 50), ERR_DEADLINE);
    accepted(&sim.remove_liquidity(ALICE, 999_000, (999_000, 999_000), NEVER));
    assert_eq!(sim.ledger(ALICE), ([999_000, 999_000], 0));
    let pool = sim.pool();
    assert_eq!((pool.reserve, pool.lp_total), ([1_000, 1_000], MINIMUM_LIQUIDITY));
    sim.assert_accounting();
}

#[test]
fn the_pool_stays_usable_after_everyone_exits() {
    let mut sim = seeded(30, 0);
    accepted(&sim.remove_liquidity(ALICE, 999_000, (0, 0), NEVER));
    accepted(&sim.deposit(BOB, 1_000, 1_000));
    accepted(&sim.add_liquidity(BOB, (1_000, 1_000), (0, 0), NEVER));
    assert_eq!(sim.ledger(BOB).1, 1_000);
    accepted(&sim.deposit(CAROL, 5_000, 0));
    accepted(&sim.swap_exact_in(CAROL, 0, 100, 1));
    sim.assert_accounting();
}

#[test]
fn removing_dust_that_rounds_to_zero_is_rejected() {
    let mut sim = Sim::new(config(30, 0));
    sim.seed(ALICE, 2_000_000, 1_001_000);
    // 1 share of ~1.4M supply is worth 1.41 units of c0 but 0.7 of c1 -> floors to zero.
    failure(&sim.remove_liquidity(ALICE, 1, (0, 0), NEVER), ERR_ZERO);
}

// ---------------------------------------------------------------- swaps ---

#[test]
fn exact_input_swaps_match_the_independent_oracle() {
    let mut sim = seeded(30, 0);
    accepted(&sim.deposit(BOB, 10_000, 0));
    failure(&sim.swap_exact_in(BOB, 0, 10_000, 9_872), ERR_SLIPPAGE);
    accepted(&sim.swap_exact_in(BOB, 0, 10_000, 9_871));
    assert_eq!(sim.ledger(BOB), ([0, 9_871], 0));
    assert_eq!(sim.pool().reserve, [1_010_000, 990_129]);
    sim.assert_accounting();
    // And back in the other direction.
    accepted(&sim.swap_exact_in(BOB, 1, 9_871, 1));
    let pool = sim.pool();
    assert_eq!(sim.ledger(BOB).0[1], 0);
    // 10_000 in, two fees paid, so strictly less than 10_000 comes back.
    assert!(sim.ledger(BOB).0[0] > 9_800 && sim.ledger(BOB).0[0] < 10_000);
    assert_eq!(pool.reserve[1], 990_129 + 9_871);
    sim.assert_accounting();
}

#[test]
fn the_protocol_fee_is_skimmed_from_the_swap_fee_only() {
    let mut sim = seeded(30, 5_000);
    accepted(&sim.deposit(BOB, 10_000, 0));
    accepted(&sim.swap_exact_in(BOB, 0, 10_000, 9_871));
    let pool = sim.pool();
    // fee = 30, protocol cut = 15: 15 stays in the reserve for LPs.
    assert_eq!(pool.reserve, [1_009_985, 990_129]);
    assert_eq!(pool.fees, [15, 0]);
    sim.assert_accounting();
    failure(&sim.call(MALLORY, ids::COLLECT_FEES, &[], &[]), ERR_AUTH);
    accepted(&sim.call(ADMIN, ids::COLLECT_FEES, &[], &[]));
    assert_eq!(sim.ledger(ADMIN).0, [15, 0]);
    assert_eq!(sim.pool().fees, [0, 0]);
    failure(&sim.call(ADMIN, ids::COLLECT_FEES, &[], &[]), ERR_ZERO);
    sim.assert_accounting();
    // Collected fees are ordinary balances the admin can withdraw.
    accepted(&sim.withdraw(ADMIN, C0, 15, addr(7)));
    sim.assert_accounting();
}

#[test]
fn swaps_validate_direction_funds_and_pool_state() {
    let mut sim = sim();
    accepted(&sim.deposit(BOB, 1_000, 0));
    failure(&sim.swap_exact_in(BOB, 0, 100, 0), ERR_NO_LIQUIDITY);
    sim.seed(ALICE, 1_000_000, 1_000_000);
    failure(&sim.swap_exact_in(BOB, 2, 100, 0), ERR_ARGS);
    failure(&sim.swap_exact_in(BOB, 0, 0, 0), ERR_ARGS);
    failure(&sim.swap_exact_in(BOB, 1, 100, 0), ERR_BALANCE);
    failure(&sim.swap_exact_in(BOB, 0, 1_001, 0), ERR_BALANCE);
    failure(&sim.swap_exact_in(CAROL, 0, 100, 0), ERR_BALANCE);
    failure(&sim.swap_exact_in(BOB, 0, 1, 0), ERR_ZERO);
    let late = {
        let mut body = vec![0u8];
        body.extend_from_slice(&100u128.to_le_bytes());
        body.extend_from_slice(&0u128.to_le_bytes());
        body.extend_from_slice(&99u32.to_le_bytes());
        sim.call(BOB, ids::SWAP_EXACT_IN, &body, &[])
    };
    failure(&late, ERR_DEADLINE);
    failure(&sim.call(BOB, ids::SWAP_EXACT_IN, &[0; 36], &[]), ERR_ARGS);
    failure(&sim.call(BOB, ids::SWAP_EXACT_IN, &[0; 37], &[(C0, 1)]), ERR_VALUE);
    sim.assert_accounting();
}

#[test]
fn trades_never_lower_the_invariant() {
    let mut sim = seeded(30, 3_000);
    accepted(&sim.deposit(BOB, 5_000_000, 5_000_000));
    let mut k = product(&sim.pool());
    for (index, amount) in [1u128, 2, 3, 7, 999, 1_000, 12_345, 100_000, 400_000].iter().enumerate()
    {
        let direction = (index % 2) as u8;
        let output = sim.swap_exact_in(BOB, direction, *amount, 0);
        if output.status_code == 0 {
            let next = product(&sim.pool());
            assert!(next >= k, "k fell from {k} to {next} on swap #{index}");
            k = next;
        }
        sim.assert_accounting();
    }
}

#[test]
fn fee_changes_apply_to_later_swaps_only() {
    let mut sim = seeded(30, 0);
    accepted(&sim.deposit(BOB, 20_000, 0));
    accepted(&sim.set_params(ADMIN, 0, 0, false));
    accepted(&sim.swap_exact_in(BOB, 0, 10_000, 0));
    // No fee: floor(1e6 * 10_000 / 1_010_000) = 9_900.
    assert_eq!(sim.ledger(BOB).0[1], 9_900);
}

// ------------------------------------------------------------- payouts ---

fn alice_with_funds() -> Sim {
    let mut sim = seeded(30, 0);
    accepted(&sim.remove_liquidity(ALICE, 999_000, (0, 0), NEVER));
    sim
}

#[test]
fn a_withdrawal_debits_the_holder_and_emits_a_tracked_payout() {
    let mut sim = alice_with_funds();
    let shell_before = sim.balance(SHELL_CURRENCY_ID);
    let output = sim.withdraw(ALICE, C0, 500_000, addr(5));
    accepted(&output);
    let [payout] = &output.outbound_intents[..] else { panic!("one payout") };
    assert_eq!(payout.destination, addr(5));
    assert_eq!(payout.fuel_fee_shells, FUEL_RESERVE);
    assert_eq!(payout.method_id, ids::RECEIVER_RECEIVE);
    assert_eq!(payout.body, payout_receipt(C0, 0, 500_000));
    assert_eq!(payout.extra.len(), 1);
    assert_eq!((payout.extra[0].currency_id, payout.extra[0].amount), (C0, 500_000));
    let [callback] = &output.callback_intents[..] else { panic!("one callback request") };
    assert_eq!((callback.outbound_index, callback.handler_method_id), (0, ids::CALLBACK));
    assert_eq!(sim.ledger(ALICE).0, [499_000, 999_000]);
    assert_eq!(sim.balance(SHELL_CURRENCY_ID), shell_before - FUEL_RESERVE);
    let pool = sim.pool();
    assert_eq!(pool.pending.len(), 1);
    assert_eq!(pool.in_flight(0).unwrap(), 500_000);
    assert_eq!(pool.liability(0).unwrap(), pool.held(0).unwrap() + 500_000);
    sim.assert_accounting();
}

#[test]
fn withdrawals_validate_their_arguments() {
    let mut sim = alice_with_funds();
    failure(&sim.withdraw(ALICE, 99, 1, addr(5)), ERR_ARGS);
    failure(&sim.withdraw(ALICE, SHELL_CURRENCY_ID, 1, addr(5)), ERR_ARGS);
    failure(&sim.withdraw(ALICE, C0, 0, addr(5)), ERR_ARGS);
    failure(&sim.withdraw(ALICE, C0, 1, addr(POOL)), ERR_ARGS);
    let zero = AccountAddress { dapp_id: [0; 32], account_id: [0; 32] };
    failure(&sim.withdraw(ALICE, C0, 1, zero), ERR_ARGS);
    failure(
        &sim.withdraw(ALICE, C0, 1, AccountAddress { dapp_id: [0x11; 32], account_id: [0; 32] }),
        ERR_ARGS,
    );
    failure(&sim.withdraw(ALICE, C0, 999_001, addr(5)), ERR_BALANCE);
    failure(&sim.withdraw(BOB, C0, 1, addr(5)), ERR_BALANCE);
    failure(&sim.call(ALICE, ids::WITHDRAW, &[0; 83], &[]), ERR_ARGS);
    failure(&sim.call(ALICE, ids::WITHDRAW, &[0; 84], &[(C0, 1)]), ERR_VALUE);
    sim.assert_accounting();
}

#[test]
fn the_vault_must_hold_the_fuel_reserve_for_each_payout() {
    let mut sim = alice_with_funds();
    sim.vault.insert(SHELL_CURRENCY_ID, FUEL_RESERVE - 1);
    failure(&sim.withdraw(ALICE, C0, 1, addr(5)), ERR_INSOLVENT);
    sim.vault.insert(SHELL_CURRENCY_ID, FUEL_RESERVE);
    accepted(&sim.withdraw(ALICE, C0, 1, addr(5)));
    assert_eq!(sim.balance(SHELL_CURRENCY_ID), 0);
    // Anyone can top the reserve back up with SHELL.
    accepted(&sim.call(MALLORY, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 50_000)]));
    accepted(&sim.withdraw(ALICE, C0, 1, addr(6)));
}

#[test]
fn each_recipient_has_one_payout_in_flight_and_slots_are_bounded() {
    let mut sim = alice_with_funds();
    accepted(&sim.withdraw(ALICE, C0, 10, addr(5)));
    failure(&sim.withdraw(ALICE, C0, 10, addr(5)), ERR_PENDING);
    failure(&sim.withdraw(BOB, C1, 10, addr(5)), ERR_PENDING);
    for index in 1..PENDING_SLOTS {
        accepted(&sim.withdraw(ALICE, C1, 10, addr(5 + index as u8)));
    }
    failure(&sim.withdraw(ALICE, C1, 10, addr(99)), ERR_CAPACITY);
    // Settling one frees its slot and the recipient.
    accepted(&sim.settle(addr(5), Settle::Delivered(payout_receipt(C0, 0, 10))));
    accepted(&sim.withdraw(ALICE, C0, 10, addr(5)));
    sim.assert_accounting();
}

#[test]
fn a_delivered_payout_settles_without_touching_balances() {
    let mut sim = alice_with_funds();
    accepted(&sim.withdraw(ALICE, C0, 500_000, addr(5)));
    let held = sim.pool().held(0).unwrap();
    let vault = sim.balance(C0);
    accepted(&sim.settle(addr(5), Settle::Delivered(payout_receipt(C0, 0, 500_000))));
    let pool = sim.pool();
    assert!(pool.pending.is_empty());
    assert_eq!(pool.receipt_pos, 1);
    assert_eq!(pool.held(0).unwrap(), held);
    assert_eq!(sim.balance(C0), vault);
    assert_eq!(pool.liability(0).unwrap(), pool.held(0).unwrap());
    sim.assert_accounting();
}

#[test]
fn a_bounced_payout_returns_exactly_to_the_holders_ledger() {
    let mut sim = alice_with_funds();
    accepted(&sim.withdraw(ALICE, C0, 999_000, addr(5)));
    accepted(&sim.withdraw(ALICE, C1, 999_000, addr(6)));
    assert_eq!(sim.ledger(ALICE).0, [0, 0]);
    // The account survives while payouts are in flight, so a refund always has a home.
    assert_eq!(sim.pool().accounts.len(), 1);
    accepted(
        &sim.settle(addr(5), Settle::Bounced(vec![(SHELL_CURRENCY_ID, 4_000), (C0, 999_000)])),
    );
    assert_eq!(sim.ledger(ALICE).0, [999_000, 0]);
    accepted(&sim.settle(addr(6), Settle::Delivered(payout_receipt(C1, 1, 999_000))));
    assert_eq!(sim.ledger(ALICE).0, [999_000, 0]);
    sim.assert_accounting();
    assert_eq!(sim.pool().withdraw_seq, 2);
}

#[test]
fn forged_or_inexact_callbacks_are_rejected_and_the_payout_stays_pending() {
    let mut sim = alice_with_funds();
    accepted(&sim.withdraw(ALICE, C0, 1_000, addr(5)));
    let receipt = payout_receipt(C0, 0, 1_000);
    let h = sim.fresh_hash();
    let before = sim.state.clone();
    let vault = sim.vault.clone();
    let forged = |sim: &mut Sim, how: Settle, method: u64, source: AccountAddress| {
        sim.settle_with(addr(5), how, h, method, source)
    };
    // Delivered, but the echoed receipt is wrong or missing.
    let wrong = payout_receipt(C0, 0, 999);
    failure(&forged(&mut sim, Settle::Delivered(wrong), ids::CALLBACK, addr(5)), ERR_CALLBACK);
    failure(&forged(&mut sim, Settle::Delivered(vec![]), ids::CALLBACK, addr(5)), ERR_CALLBACK);
    // Delivered, yet principal came back with it.
    let returning = Settle::DeliveredReturning(receipt.clone(), vec![(C0, 1_000)]);
    failure(&forged(&mut sim, returning, ids::CALLBACK, addr(5)), ERR_CALLBACK);
    let foreign = Settle::DeliveredReturning(receipt.clone(), vec![(C1, 5)]);
    failure(&forged(&mut sim, foreign, ids::CALLBACK, addr(5)), ERR_CALLBACK);
    // Nothing is pending for this source, or the handler method is wrong.
    let ok = Settle::Delivered(receipt.clone());
    failure(&forged(&mut sim, ok, ids::CALLBACK, addr(6)), ERR_CALLBACK);
    let ok = Settle::Delivered(receipt.clone());
    failure(&forged(&mut sim, ok, ids::DEPOSIT, addr(5)), ERR_CALLBACK);
    // Bounced with the wrong principal: short, long, wrong currency, extra currency, nothing.
    for returned in [
        vec![(C0, 999)],
        vec![(C0, 1_001)],
        vec![(C1, 1_000)],
        vec![(C0, 1_000), (C1, 1)],
        vec![],
        vec![(SHELL_CURRENCY_ID, 4_000)],
    ] {
        failure(&forged(&mut sim, Settle::Bounced(returned), ids::CALLBACK, addr(5)), ERR_CALLBACK);
    }
    assert_eq!(sim.state, before, "rejected callbacks leave the state untouched");
    assert_eq!(sim.vault, vault);
    assert_eq!(sim.pool().pending.len(), 1);
    // The genuine receipt still settles.
    accepted(&forged(&mut sim, Settle::Delivered(receipt), ids::CALLBACK, addr(5)));
    assert!(sim.pool().pending.is_empty());
}

#[test]
fn a_cause_hash_can_only_be_used_once_while_it_is_remembered() {
    let mut sim = alice_with_funds();
    accepted(&sim.withdraw(ALICE, C0, 10, addr(5)));
    let hash = sim.fresh_hash();
    accepted(&sim.settle_with(
        addr(5),
        Settle::Delivered(payout_receipt(C0, 0, 10)),
        hash,
        ids::CALLBACK,
        addr(5),
    ));
    accepted(&sim.withdraw(ALICE, C0, 10, addr(5)));
    failure(
        &sim.settle_with(
            addr(5),
            Settle::Delivered(payout_receipt(C0, 1, 10)),
            hash,
            ids::CALLBACK,
            addr(5),
        ),
        ERR_CALLBACK,
    );
    let fresh = sim.fresh_hash();
    accepted(&sim.settle_with(
        addr(5),
        Settle::Delivered(payout_receipt(C0, 1, 10)),
        fresh,
        ids::CALLBACK,
        addr(5),
    ));
}

#[test]
fn the_receipt_ring_wraps_without_losing_consistency() {
    let mut sim = alice_with_funds();
    for round in 0..40u64 {
        accepted(&sim.withdraw(ALICE, C0, 1, addr(5)));
        accepted(&sim.settle(addr(5), Settle::Delivered(payout_receipt(C0, round, 1))));
        sim.assert_accounting();
    }
    assert_eq!(sim.pool().receipt_pos, 40);
    assert_eq!(sim.pool().withdraw_seq, 40);
}

#[test]
fn only_callbacks_may_arrive_from_other_accounts() {
    let mut sim = alice_with_funds();
    for method in [ids::DEPOSIT, ids::WITHDRAW, ids::SWAP_EXACT_IN, ids::COLLECT_FEES, ids::CALLBACK]
    {
        failure(&internal_call(&sim, addr(5), method, &[]), ERR_AUTH);
    }
    accepted(&sim.withdraw(ALICE, C0, 1, addr(5)));
    // A plain internal message from the recipient is not a callback.
    failure(&internal_call(&sim, addr(5), ids::CALLBACK, &[]), ERR_AUTH);
}

// ------------------------------------------------------ admin and pause ---

#[test]
fn pausing_blocks_new_activity_but_never_exits() {
    let mut sim = alice_with_funds();
    accepted(&sim.deposit(BOB, 10_000, 10_000));
    accepted(&sim.add_liquidity(BOB, (1_000, 1_000), (0, 0), NEVER));
    failure(&sim.set_params(MALLORY, 30, 0, true), ERR_AUTH);
    accepted(&sim.set_params(ADMIN, 30, 0, true));
    failure(&sim.deposit(BOB, 1, 0), ERR_PAUSED);
    failure(&sim.add_liquidity(BOB, (1_000, 1_000), (0, 0), NEVER), ERR_PAUSED);
    failure(&sim.swap_exact_in(BOB, 0, 100, 0), ERR_PAUSED);
    accepted(&sim.remove_liquidity(BOB, 1, (0, 0), NEVER));
    accepted(&sim.withdraw(ALICE, C0, 100, addr(5)));
    accepted(&sim.settle(addr(5), Settle::Bounced(vec![(C0, 100)])));
    accepted(&sim.call(ALICE, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 1)]));
    accepted(&sim.set_params(ADMIN, 30, 0, false));
    accepted(&sim.deposit(BOB, 1, 0));
    sim.assert_accounting();
}

#[test]
fn parameters_are_admin_only_and_bounded() {
    let mut sim = sim();
    failure(&sim.set_params(ALICE, 30, 0, false), ERR_AUTH);
    failure(&sim.set_params(ADMIN, 1_001, 0, false), ERR_ARGS);
    failure(&sim.set_params(ADMIN, 30, 5_001, false), ERR_ARGS);
    failure(&sim.call(ADMIN, ids::SET_PARAMS, &[30, 0, 0, 0, 2], &[]), ERR_ARGS);
    failure(&sim.call(ADMIN, ids::SET_PARAMS, &[30, 0, 0, 0], &[]), ERR_ARGS);
    accepted(&sim.set_params(ADMIN, 1_000, 5_000, false));
    let pool = sim.pool();
    assert_eq!((pool.config.swap_fee_bps, pool.config.protocol_fee_bps), (1_000, 5_000));
    assert_eq!(pool.config.admin, ADMIN, "the admin identity is immutable");
}

#[test]
fn topup_only_takes_shell_and_leaves_the_state_alone() {
    let mut sim = seeded(30, 0);
    let before = sim.state.clone();
    accepted(&sim.call(MALLORY, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 77)]));
    assert_eq!(sim.state, before);
    failure(&sim.call(MALLORY, ids::TOPUP, &[], &[(C0, 5)]), ERR_VALUE);
    failure(&sim.call(MALLORY, ids::TOPUP, &[], &[]), ERR_VALUE);
    failure(&sim.call(MALLORY, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 0)]), ERR_VALUE);
}

// ---------------------------------------------- state canonicality, trips ---

const OFF_FLAGS: usize = 64;
const OFF_RESERVES: usize = 72;
const OFF_LP_TOTAL: usize = 136;
const OFF_RESERVED: usize = 152;
const OFF_SEQ: usize = 200;
const OFF_RECEIPT_POS: usize = 208;
const OFF_RECEIPTS: usize = 216;
const OFF_ACCOUNTS: usize = 728;
const OFF_PENDING: usize = 3_288;

#[test]
fn the_decoder_rejects_every_non_canonical_state() {
    let mut sim = alice_with_funds();
    accepted(&sim.deposit(BOB, 5, 5));
    accepted(&sim.withdraw(ALICE, C0, 10, addr(5)));
    accepted(&sim.withdraw(ALICE, C1, 10, addr(6)));
    accepted(&sim.settle(addr(5), Settle::Delivered(payout_receipt(C0, 0, 10))));
    let good = sim.state.clone();
    assert!(State::decode(&good).is_ok());
    assert_eq!(OFF_PENDING, OFF_ACCOUNTS + 32 * 80);
    let set = |offset: usize, bytes: &[u8]| {
        let mut state = good.clone();
        state[offset..offset + bytes.len()].copy_from_slice(bytes);
        state
    };
    let mutations: Vec<(&str, Vec<u8>)> = vec![
        ("pause flag out of range", set(OFF_FLAGS, &[2])),
        ("padding after flags", set(OFF_FLAGS + 1, &[1])),
        ("reserve zeroed while live", set(OFF_RESERVES, &[0; 16])),
        ("LP supply off by one", set(OFF_LP_TOTAL, &1_001u128.to_le_bytes())),
        ("LP supply below the locked minimum", set(OFF_LP_TOTAL, &1u128.to_le_bytes())),
        ("reserved bytes (first)", set(OFF_RESERVED, &[1])),
        ("reserved bytes (last)", set(OFF_RESERVED + 39, &[1])),
        ("receipt count without a receipt", set(OFF_RECEIPT_POS, &2u64.to_le_bytes())),
        ("receipt slot written but empty", set(OFF_RECEIPTS, &[0; 32])),
        ("stray receipt", set(OFF_RECEIPTS + 5 * 32, &[9; 32])),
        ("duplicate holder key", {
            let key: Vec<u8> = good[OFF_ACCOUNTS..OFF_ACCOUNTS + 32].to_vec();
            set(OFF_ACCOUNTS + 80, &key)
        }),
        ("hole before a used account slot", set(OFF_ACCOUNTS, &[0; 32])),
        ("pending id beyond the sequence", set(OFF_SEQ, &1u64.to_le_bytes())),
        ("pending slot with unknown currency", set(OFF_PENDING + 104, &55u32.to_le_bytes())),
        ("pending slot with zero amount", set(OFF_PENDING + 112, &[0; 16])),
        ("pending slot padding", set(OFF_PENDING + 1, &[1])),
        ("pending flag out of range", set(OFF_PENDING, &[2])),
    ];
    for (name, state) in mutations {
        assert!(State::decode(&state).is_err(), "decoder accepted: {name}");
        let output = raw_call(&sim, &state, ALICE, ids::DEPOSIT, &[]);
        failure(&output, ERR_STATE);
    }
    let mut short = good.clone();
    short.pop();
    assert!(State::decode(&short).is_err());
}

#[test]
fn an_underfunded_vault_refuses_to_operate() {
    let mut sim = seeded(30, 0);
    accepted(&sim.deposit(BOB, 100, 0));
    sim.vault.insert(C0, sim.balance(C0) - 10);
    failure(&sim.deposit(BOB, 1, 0), ERR_INSOLVENT);
    failure(&sim.swap_exact_in(BOB, 0, 10, 0), ERR_INSOLVENT);
    failure(&sim.call(ALICE, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 1)]), ERR_INSOLVENT);
}

#[test]
fn the_balance_check_works_before_or_after_the_transfer_is_credited() {
    let mut sim = sim();
    accepted(&sim.call_with_pre_credit_balance(
        ALICE,
        ids::DEPOSIT,
        &[],
        &[(C0, 500_000), (C1, 700_000)],
    ));
    sim.assert_accounting();
    accepted(&sim.call_with_pre_credit_balance(ALICE, ids::DEPOSIT, &[], &[(C0, 1)]));
    let mut body = Vec::new();
    for value in [500_001u128, 700_000, 0, 0] {
        body.extend_from_slice(&value.to_le_bytes());
    }
    body.extend_from_slice(&NEVER.to_le_bytes());
    accepted(&sim.call_with_pre_credit_balance(ALICE, ids::ADD_LIQUIDITY, &body, &[]));
    sim.assert_accounting();
}

#[test]
fn a_round_trip_never_returns_more_than_it_started_with() {
    let mut sim = seeded(30, 0);
    accepted(&sim.deposit(BOB, 1_000_000, 0));
    for amount in [40u128, 333, 1_001, 5_000, 77_777, 250_000] {
        let start = sim.ledger(BOB).0;
        accepted(&sim.swap_exact_in(BOB, 0, amount, 0));
        let proceeds = sim.ledger(BOB).0[1] - start[1];
        accepted(&sim.swap_exact_in(BOB, 1, proceeds, 0));
        let end = sim.ledger(BOB).0;
        assert!(end[0] < start[0], "round trip of {amount} gained value: {start:?} -> {end:?}");
        assert_eq!(end[1], start[1]);
        sim.assert_accounting();
    }
}

#[test]
fn math_and_contract_agree_on_a_published_example() {
    // The README's worked example, derived independently of the contract code.
    assert_eq!(math::swap_out(1_000_000, 1_000_000, 10_000, 30), Ok(9_871));
    assert_eq!(math::swap_in(1_000_000, 1_000_000, 9_000, 30), Ok(9_110));
    assert_eq!(math::sqrt_product(1_000_000, 1_000_000) - MINIMUM_LIQUIDITY, 999_000);
}
