//! The market paying out to a real second contract (the Payee fixture): deploy,
//! bind, deliver, echo, settle; plus the bounce path and the payee's own
//! negative cases.

mod common;

use std::collections::BTreeMap;

use avm_contract_sdk::{
    decode_message_input, AccountAddress, Contract, MessageOutput, StateUpdate,
};
use avm_lending_market::{ids, ERR_CALLBACK};
use avm_lending_payee::{
    last_receipt, received_count, Payee, ERR_ARGS, ERR_AUTH, ERR_REJECT, STATE_BYTES,
};
use common::*;

const PAYEE_OWNER: [u8; 32] = [0x61; 32];

struct Dest {
    address: AccountAddress,
    state: Vec<u8>,
}

impl Dest {
    fn deploy(id: u8) -> Self {
        let address = addr(id);
        let message = external_message(&envelope(&[]), PAYEE_OWNER, 0, &[], address, true);
        let framed = frame(&message, &[], Some(PAYEE_OWNER), None, address, 10, &BTreeMap::new());
        let output = Payee::init(decode_message_input(&framed).unwrap());
        accepted(&output);
        let Some(StateUpdate::FullReplace(state)) = output.state_update else { panic!() };
        assert_eq!(state.len(), STATE_BYTES);
        Self { address, state }
    }

    fn apply(&mut self, output: MessageOutput) -> MessageOutput {
        if output.status_code == 0 {
            let Some(StateUpdate::FullReplace(state)) = &output.state_update else { panic!() };
            self.state = state.clone();
        }
        output
    }

    fn owner_call(&mut self, method: u64, body: &[u8]) -> MessageOutput {
        let message = external_message(body, PAYEE_OWNER, method, &[], self.address, false);
        let framed = frame(
            &message,
            &self.state,
            Some(PAYEE_OWNER),
            None,
            self.address,
            20,
            &BTreeMap::new(),
        );
        let output = Payee::process_message(decode_message_input(&framed).unwrap());
        self.apply(output)
    }

    fn bind(&mut self, market: AccountAddress) -> MessageOutput {
        let mut body = market.dapp_id.to_vec();
        body.extend_from_slice(&market.account_id);
        self.owner_call(ids::PAYEE_BIND, &body)
    }

    fn set_reject(&mut self, on: bool) -> MessageOutput {
        self.owner_call(ids::PAYEE_REJECT, &[u8::from(on)])
    }

    /// Delivers a market payout exactly as the runtime would.
    fn receive(&mut self, source: AccountAddress, payout: &Dispatched) -> MessageOutput {
        self.receive_raw(source, &payout.body, &payout.extra, Some(payout.handler), payout.fuel)
    }

    fn receive_raw(
        &mut self,
        source: AccountAddress,
        body: &[u8],
        extra: &[(u32, u128)],
        handler: Option<u64>,
        fuel: u128,
    ) -> MessageOutput {
        let message = internal_message(
            ids::PAYEE_RECEIVE,
            body,
            extra,
            source,
            self.address,
            None,
            handler,
            fuel,
        );
        let mut balances = BTreeMap::new();
        for (id, amount) in extra {
            balances.insert(*id, *amount);
        }
        let framed = frame(&message, &self.state, None, Some(source), self.address, 20, &balances);
        let output = Payee::process_message(decode_message_input(&framed).unwrap());
        self.apply(output)
    }
}

/// A live market with Bob holding withdrawable funds: 500_000 idle collateral
/// and 100_000 borrowed asset.
fn funded() -> Sim {
    let mut sim = Sim::live();
    accepted(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 500_000));
    accepted(&sim.amount(BOB, ids::BORROW, 100_000));
    sim
}

fn dispatch(sim: &mut Sim, to: AccountAddress, currency: u32, amount: u128) -> Dispatched {
    accepted(&sim.withdraw(BOB, currency, amount, to));
    sim.outbox.last().unwrap().clone()
}

fn held(sim: &Sim, side: usize) -> u128 {
    sim.state().held(side).unwrap()
}

#[test]
fn a_withdrawal_is_delivered_echoed_and_settled_end_to_end() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    let held_before = held(&sim, 0);
    let payout = dispatch(&mut sim, dest.address, COL, 4_000);
    assert_eq!(payout.method, ids::PAYEE_RECEIVE);
    assert_eq!(payout.fuel, FUEL);
    assert_eq!(sim.account(BOB).balance[0], 496_000, "debited before the payout leaves");

    let reply = dest.receive(addr(POOL), &payout);
    accepted(&reply);
    assert_eq!(reply.callback_result_body, payout.body, "the payee echoes the receipt");
    assert_eq!(received_count(&dest.state), Some(1));
    assert_eq!(last_receipt(&dest.state).unwrap().as_slice(), payout.body.as_slice());

    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert!(sim.outbox.is_empty() && sim.state().pending.is_empty());
    assert_eq!(held(&sim, 0), held_before - 4_000);
    assert_eq!(sim.events, ["lend.action"]);
    sim.assert_accounting();
}

#[test]
fn a_rejecting_payee_bounces_the_value_back_into_the_ledger() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    accepted(&dest.set_reject(true));
    let held_before = held(&sim, 1);
    let payout = dispatch(&mut sim, dest.address, DEBT, 60_000);
    assert_eq!(sim.account(BOB).balance[1], 40_000);

    failure(&dest.receive(addr(POOL), &payout), ERR_REJECT);
    assert_eq!(received_count(&dest.state), Some(0));
    accepted(&sim.settle(dest.address, Settle::Bounced(payout.extra.clone())));
    assert_eq!(sim.account(BOB).balance[1], 100_000, "nothing was lost");
    assert_eq!(held(&sim, 1), held_before);
    sim.assert_accounting();

    // Once the payee relents, the same withdrawal succeeds.
    accepted(&dest.set_reject(false));
    let retry = dispatch(&mut sim, dest.address, DEBT, 60_000);
    let reply = dest.receive(addr(POOL), &retry);
    accepted(&reply);
    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert_eq!(held(&sim, 1), held_before - 60_000);
    sim.assert_accounting();
}

#[test]
fn a_bounce_does_not_hide_a_position_that_lost_its_account_slot() {
    // The withdrawing account is empty after the debit; the pending entry keeps it alive.
    let mut sim = Sim::new();
    accepted(&sim.call(
        ADMIN,
        ids::TOPUP,
        &[],
        &[(avm_lending_market::SHELL_CURRENCY_ID, 100_000)],
    ));
    accepted(&sim.deposit(OUTSIDER, &[(COL, 70)]));
    let payout = dispatch_from(&mut sim, OUTSIDER, addr(30), COL, 70);
    assert_eq!(sim.state().accounts.len(), 1, "kept for the refund");
    accepted(&sim.settle(addr(30), Settle::Bounced(payout.extra.clone())));
    assert_eq!(sim.account(OUTSIDER).balance[0], 70);
    // A delivered payout finally lets the empty account go.
    let payout = dispatch_from(&mut sim, OUTSIDER, addr(30), COL, 70);
    accepted(&sim.settle(addr(30), Settle::Delivered(payout.body)));
    assert!(sim.state().accounts.is_empty());
}

fn dispatch_from(
    sim: &mut Sim,
    actor: [u8; 32],
    to: AccountAddress,
    currency: u32,
    amount: u128,
) -> Dispatched {
    accepted(&sim.withdraw(actor, currency, amount, to));
    sim.outbox.last().unwrap().clone()
}

#[test]
fn callbacks_are_checked_against_the_pending_payout() {
    let mut sim = funded();
    let to = addr(10);
    let payout = dispatch(&mut sim, to, COL, 700);
    let hash = sim.fresh_hash();
    let ok = |sim: &mut Sim, how: Settle, hash: [u8; 32], method: u64, source| {
        sim.settle_with(to, how, hash, method, source)
    };
    // Unknown sender and wrong method (a zero hash is rejected by the SDK decoder).
    failure(&ok(&mut sim, Settle::Delivered(vec![]), hash, ids::CALLBACK, addr(99)), ERR_CALLBACK);
    failure(&ok(&mut sim, Settle::Delivered(vec![]), hash, ids::SUPPLY, to), ERR_CALLBACK);
    // Success must not carry principal; a bounce must carry exactly the principal.
    let principal = payout.extra.clone();
    failure(
        &ok(&mut sim, Settle::DeliveredReturning(vec![], principal), hash, ids::CALLBACK, to),
        ERR_CALLBACK,
    );
    failure(&ok(&mut sim, Settle::Bounced(vec![]), hash, ids::CALLBACK, to), ERR_CALLBACK);
    failure(
        &ok(&mut sim, Settle::Bounced(vec![(COL, 699)]), hash, ids::CALLBACK, to),
        ERR_CALLBACK,
    );
    failure(
        &ok(&mut sim, Settle::Bounced(vec![(DEBT, 700)]), hash, ids::CALLBACK, to),
        ERR_CALLBACK,
    );
    assert_eq!(sim.state().pending.len(), 1, "still pending after every rejected callback");
    // The right callback settles; replaying it afterwards finds nothing pending.
    accepted(&ok(&mut sim, Settle::Delivered(vec![]), hash, ids::CALLBACK, to));
    failure(&ok(&mut sim, Settle::Delivered(vec![]), hash, ids::CALLBACK, to), ERR_CALLBACK);
    sim.assert_accounting();
}

#[test]
fn a_reused_cause_hash_cannot_settle_a_later_payout() {
    let mut sim = funded();
    let to = addr(10);
    dispatch(&mut sim, to, COL, 10);
    let hash = sim.fresh_hash();
    accepted(&sim.settle_with(to, Settle::Delivered(vec![]), hash, ids::CALLBACK, to));
    dispatch(&mut sim, to, COL, 20);
    failure(&sim.settle_with(to, Settle::Delivered(vec![]), hash, ids::CALLBACK, to), ERR_CALLBACK);
    assert_eq!(sim.state().pending.len(), 1);
}

#[test]
fn a_forged_echo_cannot_hold_a_slot_hostage() {
    // The authenticated status proves delivery; the payee-controlled echo is not trusted.
    let mut sim = funded();
    let payout = dispatch(&mut sim, addr(10), COL, 1_234);
    let mut forged = payout.body.clone();
    forged[16] ^= 1;
    accepted(&sim.settle(addr(10), Settle::Delivered(forged)));
    assert!(sim.state().pending.is_empty());
    sim.assert_accounting();
}

#[test]
fn successive_withdrawals_use_distinct_receipts_and_replays_are_refused() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    let mut payouts = Vec::new();
    for amount in [100u128, 250, 900] {
        let payout = dispatch(&mut sim, dest.address, COL, amount);
        let reply = dest.receive(addr(POOL), &payout);
        accepted(&reply);
        accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
        payouts.push(payout);
    }
    assert_eq!(received_count(&dest.state), Some(3));
    let ids_seen: Vec<u64> =
        payouts.iter().map(|p| u64::from_le_bytes(p.body[8..16].try_into().unwrap())).collect();
    assert!(ids_seen.windows(2).all(|w| w[0] < w[1]), "{ids_seen:?}");
    // Redelivering the latest payout is a replay and changes nothing.
    let snapshot = dest.state.clone();
    failure(&dest.receive(addr(POOL), &payouts[2]), ERR_ARGS);
    assert_eq!(dest.state, snapshot);
    sim.assert_accounting();
}

#[test]
fn the_payee_enforces_who_may_call_it_and_what_it_accepts() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    let payout = dispatch(&mut sim, dest.address, COL, 700);
    // Unbound payees accept nothing.
    failure(&dest.receive(addr(POOL), &payout), ERR_AUTH);
    accepted(&dest.bind(addr(POOL)));
    failure(&dest.bind(addr(POOL)), ERR_ARGS);
    let message = external_message(&[1], OUTSIDER, ids::PAYEE_REJECT, &[], dest.address, false);
    let framed =
        frame(&message, &dest.state, Some(OUTSIDER), None, dest.address, 20, &BTreeMap::new());
    failure(&Payee::process_message(decode_message_input(&framed).unwrap()), ERR_AUTH);
    // Wrong sender, missing callback request, mismatching value, malformed receipts.
    failure(&dest.receive(addr(77), &payout), ERR_AUTH);
    failure(
        &dest.receive_raw(addr(POOL), &payout.body, &payout.extra, None, payout.fuel),
        ERR_AUTH,
    );
    for extra in [vec![(COL, 699)], vec![(DEBT, 700)]] {
        failure(
            &dest.receive_raw(addr(POOL), &payout.body, &extra, Some(payout.handler), payout.fuel),
            ERR_ARGS,
        );
    }
    let mut bad_magic = payout.body.clone();
    bad_magic[0] = b'X';
    failure(
        &dest.receive_raw(addr(POOL), &bad_magic, &payout.extra, Some(payout.handler), payout.fuel),
        ERR_ARGS,
    );
    failure(
        &dest.receive_raw(
            addr(POOL),
            &payout.body[..31],
            &payout.extra,
            Some(payout.handler),
            payout.fuel,
        ),
        ERR_ARGS,
    );
    assert_eq!(received_count(&dest.state), Some(0));
    let reply = dest.receive(addr(POOL), &payout);
    accepted(&reply);
    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert_eq!(received_count(&dest.state), Some(1));
}
