//! The treasury paying out to a real second contract (the Payee fixture): deploy,
//! bind, deliver, echo, settle; plus the bounce path and the payee's own
//! negative cases.

mod common;

use std::collections::BTreeMap;

use avm_contract_sdk::{
    decode_message_input, AccountAddress, Contract, MessageOutput, StateUpdate,
};
use avm_multisig_treasury::ids;
use avm_treasury_payee::{
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

    fn bind(&mut self, amm: AccountAddress) -> MessageOutput {
        let mut body = amm.dapp_id.to_vec();
        body.extend_from_slice(&amm.account_id);
        self.owner_call(ids::PAYEE_BIND, &body)
    }

    fn set_reject(&mut self, on: bool) -> MessageOutput {
        self.owner_call(ids::PAYEE_REJECT, &[u8::from(on)])
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

fn funded() -> Sim {
    Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C])
}

/// Runs a transfer proposal up to (and including) its execution.
fn dispatch(sim: &mut Sim, to: AccountAddress, currency: u32, amount: u128) -> Dispatched {
    let id = sim.transfer(OWNER_A, to, currency, amount);
    accepted(&sim.approve(OWNER_B, id));
    sim.advance(DELAY);
    accepted(&sim.execute(OWNER_C, id));
    sim.outbox.last().unwrap().clone()
}

#[test]
fn a_transfer_is_delivered_echoed_and_settled_end_to_end() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    let payout = dispatch(&mut sim, dest.address, C7, 4_000);
    assert_eq!(payout.method, ids::PAYEE_RECEIVE);

    let reply = dest.receive(addr(POOL), &payout);
    accepted(&reply);
    assert_eq!(reply.callback_result_body, payout.body, "the payee echoes the receipt");
    assert_eq!(received_count(&dest.state), Some(1));
    assert_eq!(last_receipt(&dest.state).unwrap().as_slice(), payout.body.as_slice());

    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert!(sim.outbox.is_empty() && sim.state().pending.is_empty());
    assert_eq!(sim.state().held(C7), 996_000);
    sim.assert_accounting();
}

#[test]
fn a_rejecting_payee_bounces_the_value_back_into_the_treasury() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    accepted(&dest.set_reject(true));
    let payout = dispatch(&mut sim, dest.address, C9, 2_500);

    failure(&dest.receive(addr(POOL), &payout), ERR_REJECT);
    assert_eq!(received_count(&dest.state), Some(0));
    accepted(&sim.settle(dest.address, Settle::Bounced(payout.extra.clone())));
    assert_eq!(sim.state().held(C9), 1_000_000, "nothing was lost");
    sim.assert_accounting();

    // Once the payee relents, the same approved proposal succeeds.
    accepted(&dest.set_reject(false));
    let id = sim.state().proposals[0].id;
    accepted(&sim.execute(OWNER_A, id));
    let retry = sim.outbox.last().unwrap().clone();
    let reply = dest.receive(addr(POOL), &retry);
    accepted(&reply);
    accepted(&sim.settle(dest.address, Settle::Delivered(reply.callback_result_body)));
    assert_eq!(received_count(&dest.state), Some(1));
    assert_eq!(sim.state().held(C9), 997_500);
    assert!(sim.state().proposals.is_empty());
}

#[test]
fn successive_transfers_use_distinct_receipts_and_replays_are_refused() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    let mut payouts = Vec::new();
    for amount in [100u128, 250, 900] {
        let payout = dispatch(&mut sim, dest.address, C7, amount);
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
    let payout = dispatch(&mut sim, dest.address, C7, 700);
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
    for extra in [vec![(C7, 699)], vec![(C9, 700)]] {
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

#[test]
fn a_payee_that_echoes_the_wrong_receipt_cannot_settle_the_transfer() {
    let mut sim = funded();
    let mut dest = Dest::deploy(10);
    accepted(&dest.bind(addr(POOL)));
    let payout = dispatch(&mut sim, dest.address, C7, 1_234);
    let mut forged = payout.body.clone();
    forged[16] ^= 1;
    assert_ne!(sim.settle(dest.address, Settle::Delivered(forged)).status_code, 0);
    assert_eq!(sim.state().pending.len(), 1, "the transfer stays pending");
    accepted(&sim.settle(dest.address, Settle::Delivered(payout.body)));
    assert!(sim.state().pending.is_empty());
    sim.assert_accounting();
}
