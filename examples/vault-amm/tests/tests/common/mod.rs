//! A small deterministic stand for the Vault AMM: it builds the canonical ACMI
//! frames the runtime would, applies accepted outputs, tracks the vault's real
//! ECC balance (including value that left in payouts and prepaid FUEL), and
//! delivers receiver callbacks. It is test scaffolding, not a runtime model.

#![allow(dead_code)]

use std::collections::BTreeMap;

use avm_contract_sdk::{
    decode_message_input, encode_message_output, AccountAddress, Contract, EventIntent,
    MessageOutput, StateUpdate,
};
use avm_vault_amm::{ids, Amm, Config, State, SHELL_CURRENCY_ID};

pub const ADMIN: [u8; 32] = [0x41; 32];
pub const ALICE: [u8; 32] = [0x42; 32];
pub const BOB: [u8; 32] = [0x43; 32];
pub const CAROL: [u8; 32] = [0x44; 32];
pub const MALLORY: [u8; 32] = [0x4d; 32];
pub const C0: u32 = 7;
pub const C1: u32 = 9;
pub const FUEL_RESERVE: u128 = 10_000;
pub const NEVER: u32 = u32::MAX;

pub fn addr(id: u8) -> AccountAddress {
    AccountAddress { dapp_id: [0x11; 32], account_id: [id; 32] }
}
pub const POOL: u8 = 1;

pub fn config(fee_bps: u16, protocol_bps: u16) -> Config {
    Config {
        admin: ADMIN,
        currency: [C0, C1],
        swap_fee_bps: fee_bps,
        protocol_fee_bps: protocol_bps,
        fuel_reserve: FUEL_RESERVE,
    }
}

// ------------------------------------------------------------ wire frames ---

fn put_extras(bytes: &mut Vec<u8>, entries: &[(u32, u128)]) {
    for (id, amount) in entries {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
    }
}

fn write_address(bytes: &mut [u8], offset: usize, address: AccountAddress) {
    bytes[offset..offset + 32].copy_from_slice(&address.dapp_id);
    bytes[offset + 32..offset + 64].copy_from_slice(&address.account_id);
}

pub fn envelope(init: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 288];
    bytes[..4].copy_from_slice(b"ACDP");
    bytes[268..272].copy_from_slice(&(init.len() as u32).to_le_bytes());
    bytes.extend_from_slice(init);
    bytes
}

pub fn external_message(
    body: &[u8],
    actor: [u8; 32],
    method: u64,
    extra: &[(u32, u128)],
    destination: AccountAddress,
    deploy: bool,
) -> Vec<u8> {
    let mut bytes = vec![0; 192];
    bytes[0] = 1;
    bytes[1] = if deploy { 3 } else { 1 };
    bytes[2..4].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    bytes[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    bytes[8..16].copy_from_slice(&method.to_le_bytes());
    bytes[16..32].copy_from_slice(&10_000u128.to_le_bytes());
    bytes[32..40].copy_from_slice(&1u64.to_le_bytes());
    bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
    bytes[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
    write_address(&mut bytes, 48, destination);
    bytes[112..144].copy_from_slice(&actor);
    bytes[144..146].copy_from_slice(&64u16.to_le_bytes());
    put_extras(&mut bytes, extra);
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(&[0; 64]);
    bytes
}

/// A canonical internal message. `callback` is `(cause hash, status)` for a
/// native callback; `handler` is the callback method requested by a sender.
#[allow(clippy::too_many_arguments)]
pub fn internal_message(
    method: u64,
    body: &[u8],
    extra: &[(u32, u128)],
    source: AccountAddress,
    destination: AccountAddress,
    callback: Option<([u8; 32], u32)>,
    handler: Option<u64>,
    fuel_fee: u128,
) -> Vec<u8> {
    let mut bytes = vec![0; 176];
    bytes[1] = u8::from(callback.is_some()) | (u8::from(handler.is_some()) << 2);
    bytes[2..4].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    bytes[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    bytes[8..16].copy_from_slice(&method.to_le_bytes());
    bytes[16..32].copy_from_slice(&fuel_fee.to_le_bytes());
    bytes[32..40].copy_from_slice(&5u64.to_le_bytes());
    bytes[40..44].copy_from_slice(&20u32.to_le_bytes());
    write_address(&mut bytes, 48, source);
    write_address(&mut bytes, 112, destination);
    if let Some((hash, status)) = callback {
        bytes.extend_from_slice(&hash);
        bytes.extend_from_slice(&destination.dapp_id);
        bytes.extend_from_slice(&destination.account_id);
        bytes.extend_from_slice(&status.to_le_bytes());
    }
    if let Some(handler) = handler {
        bytes.extend_from_slice(&handler.to_le_bytes());
    }
    put_extras(&mut bytes, extra);
    bytes.extend_from_slice(body);
    bytes
}

/// The ACMI execution frame. `external` is `Some(pubkey)` for a signed call,
/// otherwise the message must be internal and its source becomes the caller.
pub fn frame(
    message: &[u8],
    state: &[u8],
    external: Option<[u8; 32]>,
    source: Option<AccountAddress>,
    destination: AccountAddress,
    time: u32,
    balances: &BTreeMap<u32, u128>,
) -> Vec<u8> {
    let mut bytes = Vec::from(*b"ACMI");
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&time.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&[0x12; 32]);
    bytes.push(u8::from(external.is_some()));
    bytes.extend_from_slice(&[0; 7]);
    match (external, source) {
        (None, Some(source)) => {
            bytes.extend_from_slice(&source.dapp_id);
            bytes.extend_from_slice(&source.account_id);
            bytes.extend_from_slice(&[0; 32]);
        }
        (Some(actor), _) => {
            bytes.extend_from_slice(&[0; 64]);
            bytes.extend_from_slice(&actor);
        }
        (None, None) => panic!("an internal frame needs a source"),
    }
    bytes.extend_from_slice(&destination.dapp_id);
    bytes.extend_from_slice(&destination.account_id);
    bytes.extend_from_slice(&[0x44; 32]);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&(balances.len() as u16).to_le_bytes());
    for (id, amount) in balances {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
    }
    bytes.extend_from_slice(&(state.len() as u32).to_le_bytes());
    bytes.extend_from_slice(state);
    bytes.extend_from_slice(&(message.len() as u32).to_le_bytes());
    bytes.extend_from_slice(message);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes
}

// ------------------------------------------------------------- the stand ---

/// A payout the pool emitted, waiting for a callback.
#[derive(Clone, Debug)]
pub struct Dispatched {
    pub recipient: AccountAddress,
    pub extra: Vec<(u32, u128)>,
    pub body: Vec<u8>,
    pub method: u64,
    pub fuel: u128,
    pub handler: u64,
}

pub enum Settle {
    /// The receiver accepted and echoed this body.
    Delivered(Vec<u8>),
    /// Accepted, but the callback also (wrongly) carries these entries back.
    DeliveredReturning(Vec<u8>, Vec<(u32, u128)>),
    /// Delivery failed; these entries come back with the callback.
    Bounced(Vec<(u32, u128)>),
}

pub struct Sim {
    pub state: Vec<u8>,
    pub vault: BTreeMap<u32, u128>,
    pub time: u32,
    pub outbox: Vec<Dispatched>,
    pub next_hash: u8,
    pub init_events: Vec<EventIntent>,
}

fn encode_checked(output: &MessageOutput) {
    encode_message_output(output).expect("every returned effect set must be canonical");
}

pub fn accepted(output: &MessageOutput) {
    assert_eq!(output.status_code, 0, "expected success");
}

pub fn failure(output: &MessageOutput, code: u32) {
    assert_eq!(output.status_code, code, "wrong error code");
    assert!(output.state_update.is_none(), "a failure must not update state");
    assert!(
        output.outbound_intents.is_empty()
            && output.callback_intents.is_empty()
            && output.event_intents.is_empty(),
        "a failure must not carry effects"
    );
}

impl Sim {
    pub fn new(config: Config) -> Self {
        let message =
            external_message(&envelope(&config.encode()), config.admin, 0, &[], addr(POOL), true);
        let mut vault = BTreeMap::new();
        vault.insert(SHELL_CURRENCY_ID, 1_000_000_000u128);
        let framed =
            frame(&message, &[], Some(config.admin), None, addr(POOL), 10, &BTreeMap::new());
        let output = Amm::init(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        accepted(&output);
        let Some(StateUpdate::FullReplace(state)) = output.state_update else {
            panic!("init must return the full state")
        };
        Self {
            state,
            vault,
            time: 100,
            outbox: Vec::new(),
            next_hash: 1,
            init_events: output.event_intents,
        }
    }

    pub fn pool(&self) -> State {
        State::decode(&self.state).expect("a committed state always decodes")
    }

    pub fn balance(&self, currency: u32) -> u128 {
        self.vault.get(&currency).copied().unwrap_or(0)
    }

    /// `(balance0, balance1, lp)` of a holder's ledger account.
    pub fn ledger(&self, key: [u8; 32]) -> ([u128; 2], u128) {
        self.pool()
            .accounts
            .iter()
            .find(|account| account.key == key)
            .map_or(([0; 2], 0), |account| (account.balance, account.lp))
    }

    pub fn advance(&mut self, seconds: u32) {
        self.time += seconds;
    }

    fn commit(&mut self, output: &MessageOutput, balances: BTreeMap<u32, u128>) {
        let Some(StateUpdate::FullReplace(state)) = &output.state_update else {
            panic!("an accepted call returns the full state")
        };
        self.state = state.clone();
        self.vault = balances;
        for intent in &output.outbound_intents {
            for entry in &intent.extra {
                let held = self.vault.get_mut(&entry.currency_id).expect("vault balance");
                *held = held.checked_sub(entry.amount).expect("payout must be covered");
            }
            // The sender prepays the FUEL reserve when the message is materialized.
            let shell = self.vault.get_mut(&SHELL_CURRENCY_ID).expect("SHELL balance");
            *shell = shell.checked_sub(intent.fuel_fee_shells).expect("FUEL must be covered");
            let handler = output
                .callback_intents
                .iter()
                .find(|callback| callback.outbound_index == 0)
                .map(|callback| callback.handler_method_id)
                .expect("payouts request a callback");
            self.outbox.push(Dispatched {
                recipient: intent.destination,
                extra: intent.extra.iter().map(|e| (e.currency_id, e.amount)).collect(),
                body: intent.body.clone(),
                method: intent.method_id,
                fuel: intent.fuel_fee_shells,
                handler,
            });
        }
        self.vault.retain(|_, amount| *amount != 0);
    }

    /// Sends a signed external call. Accepted calls commit state and value.
    pub fn call(
        &mut self,
        actor: [u8; 32],
        method: u64,
        body: &[u8],
        extra: &[(u32, u128)],
    ) -> MessageOutput {
        let message = external_message(body, actor, method, extra, addr(POOL), false);
        let mut balances = self.vault.clone();
        for (id, amount) in extra {
            *balances.entry(*id).or_insert(0) += amount;
        }
        let framed =
            frame(&message, &self.state, Some(actor), None, addr(POOL), self.time, &balances);
        let output = Amm::process_message(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code == 0 {
            self.commit(&output, balances);
        }
        output
    }

    /// Same as [`Self::call`] but frames the balance as it was *before* the
    /// message's own value arrived.
    pub fn call_with_pre_credit_balance(
        &mut self,
        actor: [u8; 32],
        method: u64,
        body: &[u8],
        extra: &[(u32, u128)],
    ) -> MessageOutput {
        let message = external_message(body, actor, method, extra, addr(POOL), false);
        let framed =
            frame(&message, &self.state, Some(actor), None, addr(POOL), self.time, &self.vault);
        let output = Amm::process_message(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code == 0 {
            let mut balances = self.vault.clone();
            for (id, amount) in extra {
                *balances.entry(*id).or_insert(0) += amount;
            }
            self.commit(&output, balances);
        }
        output
    }

    /// Delivers the receiver's callback for the pending payout to `recipient`.
    pub fn settle(&mut self, recipient: AccountAddress, how: Settle) -> MessageOutput {
        let hash = self.fresh_hash();
        self.settle_with(recipient, how, hash, ids::CALLBACK, recipient)
    }

    pub fn fresh_hash(&mut self) -> [u8; 32] {
        let hash = [self.next_hash; 32];
        self.next_hash += 1;
        hash
    }

    /// A callback with full control over its header, for negative tests.
    pub fn settle_with(
        &mut self,
        recipient: AccountAddress,
        how: Settle,
        hash: [u8; 32],
        method: u64,
        source: AccountAddress,
    ) -> MessageOutput {
        let (status, body, mut returned) = match how {
            Settle::Delivered(body) => (0, body, Vec::new()),
            Settle::DeliveredReturning(body, returned) => (0, body, returned),
            Settle::Bounced(returned) => (1500, Vec::new(), returned),
        };
        returned.sort();
        let _ = recipient;
        let message = internal_message(
            method,
            &body,
            &returned,
            source,
            addr(POOL),
            Some((hash, status)),
            None,
            0,
        );
        let mut balances = self.vault.clone();
        for (id, amount) in &returned {
            *balances.entry(*id).or_insert(0) += amount;
        }
        let framed =
            frame(&message, &self.state, None, Some(source), addr(POOL), self.time, &balances);
        let output = Amm::process_message(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code == 0 {
            self.commit(&output, balances);
            if let Some(index) = self.outbox.iter().position(|d| d.recipient == recipient) {
                self.outbox.remove(index);
            }
        }
        output
    }

    // ------------------------------------------------------ convenience ---

    pub fn deposit(&mut self, actor: [u8; 32], amount0: u128, amount1: u128) -> MessageOutput {
        let mut extra = Vec::new();
        if amount0 != 0 {
            extra.push((C0, amount0));
        }
        if amount1 != 0 {
            extra.push((C1, amount1));
        }
        self.call(actor, ids::DEPOSIT, &[], &extra)
    }

    pub fn add_liquidity(
        &mut self,
        actor: [u8; 32],
        desired: (u128, u128),
        minimum: (u128, u128),
        deadline: u32,
    ) -> MessageOutput {
        let mut body = Vec::new();
        for value in [desired.0, desired.1, minimum.0, minimum.1] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        body.extend_from_slice(&deadline.to_le_bytes());
        self.call(actor, ids::ADD_LIQUIDITY, &body, &[])
    }

    pub fn remove_liquidity(
        &mut self,
        actor: [u8; 32],
        shares: u128,
        minimum: (u128, u128),
        deadline: u32,
    ) -> MessageOutput {
        let mut body = Vec::new();
        for value in [shares, minimum.0, minimum.1] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        body.extend_from_slice(&deadline.to_le_bytes());
        self.call(actor, ids::REMOVE_LIQUIDITY, &body, &[])
    }

    fn swap_body(direction: u8, exact: u128, bound: u128, deadline: u32) -> Vec<u8> {
        let mut body = vec![direction];
        body.extend_from_slice(&exact.to_le_bytes());
        body.extend_from_slice(&bound.to_le_bytes());
        body.extend_from_slice(&deadline.to_le_bytes());
        body
    }

    pub fn swap_exact_in(
        &mut self,
        actor: [u8; 32],
        direction: u8,
        amount_in: u128,
        min_out: u128,
    ) -> MessageOutput {
        let body = Self::swap_body(direction, amount_in, min_out, NEVER);
        self.call(actor, ids::SWAP_EXACT_IN, &body, &[])
    }

    pub fn withdraw(
        &mut self,
        actor: [u8; 32],
        currency: u32,
        amount: u128,
        recipient: AccountAddress,
    ) -> MessageOutput {
        let mut body = Vec::new();
        body.extend_from_slice(&currency.to_le_bytes());
        body.extend_from_slice(&amount.to_le_bytes());
        body.extend_from_slice(&recipient.dapp_id);
        body.extend_from_slice(&recipient.account_id);
        self.call(actor, ids::WITHDRAW, &body, &[])
    }

    pub fn set_params(
        &mut self,
        actor: [u8; 32],
        fee_bps: u16,
        protocol_bps: u16,
        paused: bool,
    ) -> MessageOutput {
        let mut body = Vec::new();
        body.extend_from_slice(&fee_bps.to_le_bytes());
        body.extend_from_slice(&protocol_bps.to_le_bytes());
        body.push(u8::from(paused));
        self.call(actor, ids::SET_PARAMS, &body, &[])
    }

    /// Alice seeds the pool: deposits both assets and adds them as liquidity.
    pub fn seed(&mut self, actor: [u8; 32], amount0: u128, amount1: u128) {
        accepted(&self.deposit(actor, amount0, amount1));
        accepted(&self.add_liquidity(actor, (amount0, amount1), (0, 0), NEVER));
    }

    /// Checks the accounting identities after any sequence of operations.
    pub fn assert_accounting(&self) {
        let pool = self.pool();
        for side in 0..2 {
            let currency = pool.config.currency[side];
            assert_eq!(
                self.balance(currency),
                pool.held(side).unwrap(),
                "vault balance must equal reserve + fees + holder balances for currency {currency}"
            );
        }
        let shares: u128 = pool.accounts.iter().map(|account| account.lp).sum();
        assert_eq!(shares + pool.locked_liquidity(), pool.lp_total, "LP shares are conserved");
        assert_eq!(pool.pending.len(), self.outbox.len(), "pending payouts match the outbox");
    }
}

pub fn product(pool: &State) -> u128 {
    // Test pools stay far below 2^64 per reserve, so the product fits u128.
    pool.reserve[0] * pool.reserve[1]
}
