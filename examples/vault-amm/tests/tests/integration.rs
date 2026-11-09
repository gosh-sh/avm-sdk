//! The AMM paying out to a real second contract (the Receiver fixture): deploy,
//! bind, deliver, echo, settle; plus the bounce path and the receiver's own
//! negative cases.

mod common;

use std::collections::BTreeMap;

use avm_contract_sdk::{
    decode_message_input, AccountAddress, Contract, MessageOutput, StateUpdate,
};
use avm_vault_amm::ids;
use avm_vault_receiver::{
    last_receipt, received_count, Receiver, ERR_ARGS, ERR_AUTH, ERR_REJECT, STATE_BYTES,
};
use common::*;

const OWNER: [u8; 32] = [0x51; 32];

struct Dest {
    address: AccountAddress,
    state: Vec<u8>,
}

impl Dest {
    fn deploy(id: u8) -> Self {
        let address = addr(id);
        let message = external_message(&envelope(&[]), OWNER, 0, &[], address, true);
        let framed = frame(&message, &[], Some(OWNER), None, address, 10, &BTreeMap::new());
        let output = Receiver::init(decode_message_input(&framed).unwrap());
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
        let message = external_message(body, OWNER, method, &[], self.address, false);
        let framed =
            frame(&message, &self.state, Some(OWNER), None, self.address, 20, &BTreeMap::new());
        let output = Receiver::process_message(decode_message_input(&framed).unwrap());
        self.apply(output)
    }

    fn bind(&mut self, amm: AccountAddress) -> MessageOutput {
        let mut body = amm.dapp_id.to_vec();
        body.extend_from_slice(&amm.account_id);
        self.owner_call(ids::RECEIVER_BIND, &body)
    }

    fn set_reject(&mut self, on: bool) -> MessageOutput {
        self.owner_call(ids::RECEIVER_REJECT, &[u8::from(on)])
    }

    /// Delivers a pool payout exactly as the runtime would.
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
            ids::RECEIVER_RECEIVE,
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
        let output = Receiver::process_message(decode_message_input(&framed).unwrap());
        self.apply(output)
    }
}

fn funded() -> Sim {
    let mut sim = Sim::new(config(30, 2_500));
    sim.seed(ALICE, 1_000_000, 1_000_000);
    accepted(&sim.deposit(ALICE, 50_000, 50_000));
    sim
}

#[test]
fn a_payout_is_delivered_echoed_and_settled_end_to_end() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    let before = sim.ledger(ALICE);
    accepted(&sim.withdraw(ALICE, C0, 4_000, dest.address));
    assert_eq!(sim.ledger(ALICE).0[0], before.0[0] - 4_000);
    let payout = sim.outbox[0].clone();
    assert_eq!(payout.method, ids::RECEIVER_RECEIVE);

    let reply = dest.receive(addr(POOL), &payout);
    accepted(&reply);
    assert_eq!(reply.callback_result_body, payout.body, "the receiver echoes the receipt");
    assert_eq!(received_count(&dest.state), Some(1));
    assert_eq!(last_receipt(&dest.state).unwrap().as_slice(), payout.body.as_slice());

    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert!(sim.outbox.is_empty());
    assert!(sim.pool().pending.is_empty());
    sim.assert_accounting();
}

#[test]
fn a_rejecting_receiver_bounces_the_value_back_to_the_ledger() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    accepted(&dest.set_reject(true));
    let before = sim.ledger(ALICE);
    accepted(&sim.withdraw(ALICE, C1, 2_500, dest.address));
    let payout = sim.outbox[0].clone();

    failure(&dest.receive(addr(POOL), &payout), ERR_REJECT);
    assert_eq!(received_count(&dest.state), Some(0));

    // The runtime returns the attached value with a failure callback.
    accepted(&sim.settle(dest.address, Settle::Bounced(payout.extra.clone())));
    assert_eq!(sim.ledger(ALICE), before, "the holder is made whole");
    assert!(sim.pool().pending.is_empty());
    sim.assert_accounting();

    // Once the receiver relents, the same withdrawal succeeds.
    accepted(&dest.set_reject(false));
    accepted(&sim.withdraw(ALICE, C1, 2_500, dest.address));
    let retry = sim.outbox[0].clone();
    let reply = dest.receive(addr(POOL), &retry);
    accepted(&reply);
    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert_eq!(received_count(&dest.state), Some(1));
    sim.assert_accounting();
}

#[test]
fn successive_payouts_carry_increasing_ids_and_replays_are_refused() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    let mut bodies = Vec::new();
    for amount in [100u128, 250, 900] {
        accepted(&sim.withdraw(ALICE, C0, amount, dest.address));
        let payout = sim.outbox[0].clone();
        let reply = dest.receive(addr(POOL), &payout);
        accepted(&reply);
        accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
        bodies.push(payout);
    }
    assert_eq!(received_count(&dest.state), Some(3));
    let ids_seen: Vec<u64> =
        bodies.iter().map(|p| u64::from_le_bytes(p.body[8..16].try_into().unwrap())).collect();
    assert!(ids_seen.windows(2).all(|w| w[0] < w[1]), "{ids_seen:?}");
    // Replaying the first payout (a stale id) is refused and changes nothing.
    let snapshot = dest.state.clone();
    failure(&dest.receive(addr(POOL), &bodies[0]), ERR_ARGS);
    assert_eq!(dest.state, snapshot);
    sim.assert_accounting();
}

#[test]
fn the_receiver_enforces_who_may_call_it_and_what_it_accepts() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);

    // Unbound receivers accept nothing.
    accepted(&sim.withdraw(ALICE, C0, 700, dest.address));
    let payout = sim.outbox[0].clone();
    failure(&dest.receive(addr(POOL), &payout), ERR_AUTH);

    accepted(&dest.bind(addr(POOL)));
    // Binding is once only, and only the owner may configure.
    failure(&dest.bind(addr(POOL)), ERR_ARGS);
    let message = external_message(&[1], MALLORY, ids::RECEIVER_REJECT, &[], dest.address, false);
    let framed =
        frame(&message, &dest.state, Some(MALLORY), None, dest.address, 20, &BTreeMap::new());
    failure(&Receiver::process_message(decode_message_input(&framed).unwrap()), ERR_AUTH);

    // A different sender is refused even with a perfect body.
    failure(&dest.receive(addr(77), &payout), ERR_AUTH);
    // No callback request: refused.
    failure(
        &dest.receive_raw(addr(POOL), &payout.body, &payout.extra, None, payout.fuel),
        ERR_AUTH,
    );
    // Value that disagrees with the receipt: refused.
    failure(
        &dest.receive_raw(
            addr(POOL),
            &payout.body,
            &[(C0, 699)],
            Some(payout.handler),
            payout.fuel,
        ),
        ERR_ARGS,
    );
    failure(
        &dest.receive_raw(
            addr(POOL),
            &payout.body,
            &[(C1, 700)],
            Some(payout.handler),
            payout.fuel,
        ),
        ERR_ARGS,
    );
    // Malformed receipts.
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

    // The good delivery still works after all those refusals.
    let reply = dest.receive(addr(POOL), &payout);
    accepted(&reply);
    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert_eq!(received_count(&dest.state), Some(1));
}

#[test]
fn a_receiver_that_echoes_the_wrong_receipt_cannot_settle_the_payout() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    accepted(&sim.withdraw(ALICE, C0, 1_234, dest.address));
    let payout = sim.outbox[0].clone();
    let mut forged = payout.body.clone();
    forged[16] ^= 1;
    let output = sim.settle(dest.address, Settle::Delivered(forged));
    assert_ne!(output.status_code, 0);
    assert_eq!(sim.pool().pending.len(), 1, "the payout stays pending");
    // The genuine echo still settles it.
    accepted(&sim.settle(dest.address, Settle::Delivered(payout.body)));
    assert!(sim.pool().pending.is_empty());
    sim.assert_accounting();
}
